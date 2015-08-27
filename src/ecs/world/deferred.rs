use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::mpsc::Sender;

use ::ecs::{
    Component,
    ComponentEdit,
    ComponentMapper,
    ComponentMappers,
    Editable,
    Entity,
    EntityManager,
    Prototype,
};

type EntityInitializer = Box<Fn(&mut ComponentMappers, Entity)>;

// messages pertaining to deferred processing
pub enum Message {
    // create n entities with this initializer.
    Next(usize, EntityInitializer),
    // an edit to be applied to an entity.
    // Editable, Vec<(Entity, Editable::Edit)>
    Edit(TypeId, Box<Any>, Box<Fn() -> Box<EditHolder>>),
    // destroy an entity.
    Destroy(Entity),
    // prunes dead entities using this function.
    Prune(TypeId, Box<Fn(&mut ComponentMappers, &EntityManager)>),
}

/// Used to initialize entities created in a deferred manner with components.
pub struct EntityCreationGuard {
    message_tx: Sender<Message>,
    initializer: Option<EntityInitializer>,
    n: usize,
}


// If there are many chained calls to the member functions,
// the amount of virtual function calls could lead to poor performance
// TODO: redesign with unboxed closures?
impl EntityCreationGuard {
    /// When the entity is created, attach this component data to it.
    pub fn with_component<C: Component>(self, c: C) -> Self {
        let mut s = self;
        if let Some(init) = s.initializer.take() {
            s.initializer = Some(Box::new(move |mappers, e| {
                init(mappers, e);
                mappers.get_mapper_mut::<C>().set(e, c.clone());
            }))
        } else {
            s.initializer = Some(Box::new(move |mappers, e| {
                mappers.get_mapper_mut::<C>().set(e, c.clone());
            }))
        }
        s
    }

    /// When the entity is created, initialize it with this prototype.
    pub fn prototype<P: Prototype + 'static>(self, p: P) -> Self {
        let mut s = self;
        if let Some(init) = s.initializer.take() {
            s.initializer = Some(Box::new(move |mappers, e| {
                init(mappers, e);
                p.initialize(mappers, e);
            }))
        } else {
            s.initializer = Some(Box::new(move |mappers, e| {
                p.initialize(mappers, e);
            }))
        }
        s
    }
}

impl Drop for EntityCreationGuard {
    fn drop(&mut self) {
        if self.initializer.is_none() { return; }
        let initializer = self.initializer.take();
        self.message_tx.send(Message::Next(self.n, initializer.unwrap())).unwrap();
    }
}

// a type-independent trait for TypedEditHolder that works well with
// the channels for deferred processing.
pub trait EditHolder {
    fn push(&mut self, Box<Any>);
    fn apply(&self, &mut ComponentMappers, &EntityManager);
}

// utility struct for holding component edits
struct TypedEditHolder<T: Editable> {
    map: HashMap<Entity, ComponentEdits<T>>
}

impl<T: Editable> TypedEditHolder<T> {
    fn new() -> Self {
        TypedEditHolder {
            map: HashMap::new()
        }
    }
}

impl<T: Editable> EditHolder for TypedEditHolder<T> {
    fn push(&mut self, edits: Box<Any>) {
        let edits: Vec<(Entity, T::Edit)> = *edits.downcast().unwrap();
        for (e, edit) in edits {
            let entry = self.map.entry(e).or_insert(ComponentEdits::new());
            entry.push(edit);
        }
    }

    fn apply(&self, mappers: &mut ComponentMappers, em: &EntityManager) {
        let mapper = mappers.get_mapper_mut::<T>();
        for (entity, edits) in &self.map {
            if !em.is_alive(*entity) { continue }
            let cdata = mapper.get_mut(*entity);
            if cdata.is_none() { continue };
            let cdata = cdata.unwrap();
            edits.apply(cdata);
        }
    }
}

// for storing component edits.
pub struct ComponentEdits<T: Editable> {
    nc: Option<T::Edit>, // non-commutative
    c: Option<T::Edit>, // commutative
}

impl<T> ComponentEdits<T>
where T: Editable {
    fn new() -> Self {
        ComponentEdits {
            nc: None,
            c: None,
        }
    }

    fn push(&mut self, edit: T::Edit) {
        if edit.is_commutative() {
            self.c = match self.c.take() {
                Some(e) => Some(e.combine_with(edit)),
                None => Some(edit)
            }
        } else {
            self.nc = match self.nc.take() {
                // TODO: log replacement here.
                Some(_) => Some(edit),
                None => Some(edit),
            }
        }
    }

    fn apply(&self, item: &mut T) {
        match self.nc {
            Some(ref e) => e.apply(item),
            None => {}
        }
        match self.c {
            Some(ref e) => e.apply(item),
            None => {}
        }
    }
}

/// A handle to the world state that is passed to systems which are processing.
/// wraps the entity manager and component mappers.
#[derive(Clone)]
pub struct WorldHandle<'a> {
    em: &'a EntityManager,
    component_mappers: &'a ComponentMappers,
    message_tx: Sender<Message>
}

impl<'a> WorldHandle<'a> {
    /// Whether an entity is currently "alive", or exists.
    pub fn is_alive(&self, e: Entity) -> bool {
        self.em.is_alive(e)
    }

    /// Destroy an entity.
    pub fn destroy_entity(&self, e: Entity) {
        self.message_tx.send(Message::Destroy(e)).unwrap();
    }

    /// Create an entity in a deferred manner.
    pub fn next_entity(&self) -> EntityCreationGuard {
        self.next_entities(1)
    }

    /// Create some entities in a deferred manner.
    pub fn next_entities(&self, n: usize) -> EntityCreationGuard {
        EntityCreationGuard {
            message_tx: self.message_tx.clone(),
            initializer: None,
            n: n
        }
    }

    pub fn submit_change<E, T>(&self, e: Entity, edit: E)
    where E: ComponentEdit<Item=T> + Any, T: Editable {
        self.message_tx.send(
            Message::Edit(
                TypeId::of::<T>(),
                Box::new(vec![(e, edit)]),
                Box::new(|| Box::new(TypedEditHolder::<T>::new()) as Box<EditHolder>)
            )
        ).unwrap();
    }

    pub fn submit_changes<E, D, I, T, S>(&self, f: D, i: I)
    where E: ComponentEdit<Item=T> + Any, D: Fn(S) -> (Entity, E), I: Iterator<Item=S>,
    T: Editable<Edit=E> {
        let edits: Vec<_> = i.map(|e| f(e)).collect();
        self.message_tx.send(
            Message::Edit(
                TypeId::of::<T>(),
                Box::new(edits),
                Box::new(|| Box::new(TypedEditHolder::<T>::new()) as Box<EditHolder>)
            )
        ).unwrap()
    }

    // Creates a query
    pub fn query(&'a self) -> EntityQuery<'a> {
        EntityQuery {
            mappers: self.component_mappers,
            em: self.em,
            num_components: 0,
            candidates: Vec::new(),
            disallowed: Vec::new(),
            message_tx: self.message_tx.clone(),
        }
    }

}

/// A builder for entity queries.
pub struct EntityQuery<'a> {
    mappers: &'a ComponentMappers,
    em: &'a EntityManager,
    num_components: usize,
    candidates: Vec<Vec<Entity>>,
    disallowed: Vec<Vec<Entity>>,
    message_tx: Sender<Message>,
}

// when a component mapper's ratio of Dead:Alive
// is over this threshold, a prune message is sent.
const PRUNE_THRESHOLD: f32 = 0.25;

impl<'a> EntityQuery<'a> {

    // filters vector and returns it with the ratio of dead to alive
    fn filter_alive(&self, entities: Vec<Entity>) -> (Vec<Entity>, f32) {
        let total = entities.len() as f32;
        let entities: Vec<Entity> = entities.into_iter().filter(|e| 
            self.em.is_alive(*e)
        ).collect();
        let ratio = 1f32 - (entities.len() as f32 / total);
        (entities, ratio)
    }

    fn attempt_prune<C: Component>(&self, ratio: f32) {
        if ratio > PRUNE_THRESHOLD {
            self.message_tx.send(Message::Prune(
                TypeId::of::<C>(), Box::new(|cm, em| {
                    let m = cm.get_mapper_mut::<C>();
                    for e in m.entities().iter().filter(|e| !em.is_alive(**e)) {
                        m.remove(*e);
                    }
            }))).unwrap();
        }
    }

    /// Causes this query to only return entities with a component of type `C`.
    pub fn with_component<C>(mut self) -> Self
    where C: Component {
        let mapper = self.mappers.get_mapper::<C>();

        let entities = mapper.entities();
        
        let (entities, ratio) = self.filter_alive(entities);
        self.attempt_prune::<C>(ratio);

        self.candidates.push(entities);
        self.num_components += 1;

        self
    }

    /// Causes this query to only return entities without a component of type `C`.
    pub fn without_component<C>(mut self) -> Self
    where C: Component {
        let mapper = self.mappers.get_mapper::<C>();

        let entities = mapper.entities();

        let (entities, ratio) = self.filter_alive(entities);
        self.attempt_prune::<C>(ratio);

        self.disallowed.push(entities);
        self
    }
}

/// The implementation of IntoIterator for EntityQuery executes
/// the query on the mappers.
impl<'a> IntoIterator for EntityQuery<'a> {
    type Item = Entity;
    type IntoIter = <Vec<Entity> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        let mut map = HashMap::new();

        // find the intersection between all the candidate lists
        for e in self.candidates.iter().flat_map(|v| v.iter()) {
            let i = map.entry(e).or_insert(0usize);
            *i += 1;
        }

        // but ensure that no disallowed entities will be included.
        for e in self.disallowed.iter().flat_map(|v| v.iter()) {
            map.insert(e, 0);
        }

        // collect all entities in the intersection
        map.into_iter().filter_map(|(k, v)| {
            if v == self.candidates.len(){ Some(*k) }
            else { None }
        }).collect::<Vec<_>>().into_iter()
    }
}

impl<'a> Deref for WorldHandle<'a> {
    type Target = ComponentMappers;
    fn deref(&self) -> &ComponentMappers {
        self.component_mappers
    }

}

// constructor function.
pub fn world_handle<'a>(em: &'a EntityManager,
                    cm: &'a ComponentMappers,
                    tx: Sender<Message>) -> WorldHandle<'a> {
    WorldHandle {
        em: em,
        component_mappers: cm,
        message_tx: tx,
    }
}