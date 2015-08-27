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
    EntityQuery,
    Prototype,
};

type EntityInitializer = Box<Fn(&mut ComponentMappers, Entity)>;

// messages pertaining to deferred processing
pub enum Message {
    // create n entities with this initializer.
    Next(usize, EntityInitializer),
    // an edit to be applied to an entity.
    // Editable, Entity, Box<Editable::Edit>
    Edit(TypeId, Entity, Box<Any>, Box<Fn() -> Box<EditHolder>>),
    // destroy an entity.
    Destroy(Entity),
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
    pub fn prototype<C: Component, P: Prototype + 'static>(self, p: P) -> Self {
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
    fn push(&mut self, Entity, Box<Any>);
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
    fn push(&mut self, entity: Entity, edit: Box<Any>) {
        let edit: Box<T::Edit> = edit.downcast().unwrap(); 
        let entry = self.map.entry(entity).or_insert(ComponentEdits::new());
        entry.push(*edit);
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

    pub fn submit_change<T>(&self, e: Entity, edit: T::Edit)
    where T: Editable {
        self.message_tx.send(
            Message::Edit(
                TypeId::of::<T>(),
                e,
                Box::new(edit),
                Box::new(|| Box::new(TypedEditHolder::<T>::new()) as Box<EditHolder>)
            )
        ).unwrap();
    }

    // Creates a query
    pub fn query(&'a self) -> EntityQuery<'a> {
        EntityQuery {
            mappers: self.component_mappers,
            em: self.em,
            num_components: 0,
            candidates: Vec::new(),
            disallowed: Vec::new()
        }
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