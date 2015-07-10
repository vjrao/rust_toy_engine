use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::mem;
use std::ops::{Deref, DerefMut};
use std::raw::TraitObject;

pub use self::vec_mapper::VecMapper;

mod vec_mapper;

/// An unique entity.
/// Entities each have a unique id which serves
/// as a weak pointer to the entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Entity {
    id: usize,
}

/// Manages creation and destruction of entities.
pub struct EntityManager {
    next: Entity,
    entities: HashSet<Entity>,
}

impl EntityManager {
    /// Creates a new `EntityManager`.
    pub fn new() -> EntityManager {
        EntityManager { 
            next: Entity { id: 0 },
            entities: HashSet::new(),
        }
    }
    /// Creates an entity.
    pub fn next(&mut self) -> Entity {
        while self.is_alive(self.next) {
            self.next.id += 1;
        }
        self.entities.insert(self.next);
        self.next
    }

    /// Creates a vector of n entities.
    pub fn next_n(&mut self, n: usize) -> Vec<Entity> {
        let mut entities = Vec::with_capacity(n);
        for _ in 0..n {
            entities.push(self.next());
        }
        entities
    }

    /// Whether an entity is currently "alive", or exists.
    pub fn is_alive(&self, e: Entity) -> bool {
        self.entities.contains(&e)
    }

    /// Destroy an entity.
    pub fn destroy(&mut self, e: Entity) {
        self.entities.remove(&e);
    }

}

#[derive(Eq, PartialEq, Hash)]
///A unique id for every component.
pub struct ComponentId(TypeId);

/// A rust component type is any static type.
pub trait Component: Any { 
    /// Get the unique `ComponentId` for this type.
    fn id() -> ComponentId {
        ComponentId(TypeId::of::<Self>())
    }
}
impl<T: Any> Component for T {}

/// Maps entities to component data.
pub trait ComponentMapper {
    type Component: Component;
    
    /// Sets the component data for a specific entity.
    fn set(&mut self, e: Entity, c: Self::Component);
    /// Get an immutable reference to an entity's component data.
    fn get(&self, e: Entity) -> Option<&Self::Component>;
    /// Get a mutable reference to an entity's component data.
    fn get_mut(&mut self, e: Entity) -> Option<&mut Self::Component>;
    /// Stop managing component data for this entity.
    fn remove(&mut self, e: Entity);
    /// Get a list of all entities this manages.
    fn entities(&self) -> Vec<Entity>;
    /// Prune dead entities from this mapper.
    fn prune(&mut self, em: &EntityManager);
}

// Contains conduits for `ComponentMapper` methods which are not generic.
trait ComponentMapperExt: 'static {
    fn remove(&mut self, e: Entity);
    fn entities(&self) -> Vec<Entity>;
    fn prune(&mut self, em: &EntityManager);
}

impl<T, M> ComponentMapperExt for M
where T: Component, M: ComponentMapper<Component=T> + 'static {
    fn remove(&mut self, e: Entity) { M::remove(self, e) }
    fn entities(&self) -> Vec<Entity> { M::entities(self) }
    fn prune(&mut self, em: &EntityManager) { M::prune(self, em) }
}

/// used to maintain ownership of the mapper so it destructs properly,
/// while storing a `TraitObject` for runtime polymorphism.
struct MapperHandle {
    obj: TraitObject,
    handle: Box<ComponentMapperExt>,
}

impl MapperHandle {
    /// Create a `MapperHandle` from a mapper.
    fn from_mapper<T, M>(mapper: M) -> MapperHandle
    where T: Component, M: ComponentMapper<Component=T> + 'static {
        let mapper = Box::new(mapper);
        let obj: TraitObject = unsafe {
            mem::transmute(&*mapper as &ComponentMapper<Component=T>)
        };
        MapperHandle {
            obj: obj,
            handle: mapper as Box<ComponentMapperExt>
        }
    }
}

/// Stores all component mappers.
pub struct ComponentMappers(HashMap<ComponentId, MapperHandle>);

impl ComponentMappers {
    /// Get an immutable reference to the component mapper for this type.
    pub fn get_mapper<T: Component>(&self)
                                -> Option<&ComponentMapper<Component=T>> {
        match self.0.get(&T::id()) {
            Some(h) => {
                Some( unsafe { mem::transmute(h.obj) } )
            }
            None => None
        }
    }

    /// Get a mutable reference to the component mapper for this type.
    pub fn get_mapper_mut<T: Component>(&mut self)
                                -> Option<&mut ComponentMapper<Component=T>> {
        match self.0.get(&T::id()) {
            Some(h) => {
                Some( unsafe { mem::transmute(h.obj) } )
            }
            None => None
        }
    }

    fn get_handle(&self, id: &ComponentId) -> Option<&ComponentMapperExt> {
        match self.0.get(id) {
            Some(h) => Some(&*h.handle),
            None => None
        }
    }
}

/// Processes entities that contain specific components.
pub trait System {
    /// Do arbitrary work on the entities supplied to it.
    /// These entities will all implement the traits
    /// this system was registered with.
    fn process(&mut self, &[Entity], &mut ComponentMappers);
}

struct SysEntry(Box<System>, Vec<ComponentId>);

// Helper function for system processing.
// Returns a vector of all values in v which are duplicate at least n times.
fn duplicate_n_times<T: Ord + Eq + Clone>(v: Vec<T>, n: usize) -> Vec<T> {
    if v.len() == 0 { return v }
    else if n == 0 { return Vec::new() }

    let mut v = v;
    let mut duplicates = Vec::new();

    v.sort_by(|a, b| a.cmp(b));

    let mut i = 1;
    let mut prev_val = &v[0];
    let mut count = 1;
    while i < v.len() {
        let cur_val = &v[i];
        if *prev_val == *cur_val {
            count += 1;
            if count == n { duplicates.push(cur_val.clone()) } 
        } else {
            prev_val = cur_val;
            count = 1;
        }

        i += 1;
    }

    duplicates
}

/// The world ties together entities, components, and systems.
pub struct World {
    pub entity_manager: EntityManager,
    component_mappers: ComponentMappers,
    systems: Vec<SysEntry>,
}

impl World {
    /// Process all the systems in this world in arbitrary order.
    pub fn process_systems(&mut self) {
        //let mut systems = Vec::new();
        for sys in &mut self.systems {
            // Find entities which contain requisite component types.

            let mut entities = Vec::new();
            // the number of components this system depends on.
            let num_components = sys.1.len();
            for id in &sys.1 {
                entities.append(
                    &mut self.component_mappers.get_handle(id).unwrap().entities()
                );
            }
            //get duplicates next to each other,
            entities = duplicate_n_times(entities, num_components);
            sys.0.process(&entities, &mut self.component_mappers);
        }
    }
}

impl Deref for World {
    type Target = ComponentMappers;
    fn deref(&self) -> &ComponentMappers {
        &self.component_mappers
    }
}

impl DerefMut for World {
    fn deref_mut(&mut self) -> &mut ComponentMappers {
        &mut self.component_mappers
    }
}

/// Factory for `World`. 
pub struct WorldBuilder {
    component_mappers: ComponentMappers,
    systems: Vec<SysEntry>,
}

impl WorldBuilder {
    /// Create a new `WorldBuilder`.
    pub fn new() -> WorldBuilder {
        WorldBuilder { 
            component_mappers: ComponentMappers(HashMap::new()),
            systems: Vec::new() 
        }
    }

    /// Add a component mapper to this world.
    pub fn with_component_mapper<T, M>(self, mapper: M) -> WorldBuilder
    where T: Component, M: ComponentMapper<Component=T> + Any {
        let mut s = self;
        s.component_mappers.0.insert(
            M::Component::id(),
            MapperHandle::from_mapper(mapper)
        );
        s
    }

    /// Add a system to this world.
    pub fn with_system<S>(self, sys: S, components: Vec<ComponentId>) -> WorldBuilder
    where S: System + 'static {
        let mut s = self;
        s.systems.push(SysEntry(Box::new(sys), components));
        s
    }

    /// Builds a world from self.
    pub fn build(self) -> World {
        World {
            entity_manager: EntityManager::new(),
            component_mappers: self.component_mappers,
            systems: self.systems
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct Position(pub i32, pub i32);
    #[derive(Debug, Eq, PartialEq)]
    struct Velocity(pub i32, pub i32);

    struct MovementSystem;
    impl System for MovementSystem {
        fn process(&mut self, entities: &[Entity], mappers: &mut ComponentMappers) {
            // calculate new positions
            let mut new_positions = Vec::new();
            {
                let pos_mapper = mappers.get_mapper::<Position>().unwrap();
                let vel_mapper = mappers.get_mapper::<Velocity>().unwrap();
                for e in entities {
                    let pos = pos_mapper.get(*e).unwrap();
                    let vel = vel_mapper.get(*e).unwrap();

                    let new_pos = Position(pos.0 + vel.0, pos.1 + vel.1);
                    new_positions.push((*e, new_pos));
                }
            }

            // set new positions.
            let mut pos_mapper = mappers.get_mapper_mut::<Position>().unwrap();
            for (e, pos) in new_positions {
                pos_mapper.set(e, pos);
            }
        }
    }
    #[test]
    #[allow(unused_variables)]
    fn build_world() {
        let world = WorldBuilder::new().build();
    }

    #[test]
    fn get_and_set_value() {
        let mut world = 
            WorldBuilder::new()
            .with_component_mapper(VecMapper::<Position>::new())
            .build();
        let e = world.entity_manager.next();

        {
            let mut pos_mapper = world.get_mapper_mut::<Position>().unwrap();
            pos_mapper.set(e, Position(6, 9))
        }

        let pos_mapper = world.get_mapper::<Position>().unwrap();
        let pos = pos_mapper.get(e).unwrap();
        assert_eq!(*pos, Position(6, 9));
    }

    #[test]
    fn test_simple_system() {
        let mut world = 
            WorldBuilder::new()
            .with_component_mapper(VecMapper::<Position>::new())
            .with_component_mapper(VecMapper::<Velocity>::new())
            .with_system(MovementSystem, vec![Position::id(), Velocity::id()])
            .build();

        let has_both = world.entity_manager.next();
        let has_pos_only = world.entity_manager.next();
        let has_vel_only = world.entity_manager.next();

        {
            let mut pos_mapper = world.get_mapper_mut::<Position>().unwrap();
            pos_mapper.set(has_both, Position(0, 0));
            pos_mapper.set(has_pos_only, Position(0, 0))
        }
        {
            let mut vel_mapper = world.get_mapper_mut::<Velocity>().unwrap();
            vel_mapper.set(has_both, Velocity(3, 4));
            vel_mapper.set(has_vel_only, Velocity(0, 0));
        }

        world.process_systems();

        let pos_mapper = world.get_mapper::<Position>().unwrap();
        let vel_mapper = world.get_mapper::<Velocity>().unwrap();

        assert_eq!(*pos_mapper.get(has_both).unwrap(), Position(3, 4));
        assert_eq!(*vel_mapper.get(has_both).unwrap(), Velocity(3, 4));

        assert_eq!(*pos_mapper.get(has_pos_only).unwrap(), Position(0, 0));
        assert_eq!(vel_mapper.get(has_pos_only), None);

        assert_eq!(pos_mapper.get(has_vel_only), None);
        assert_eq!(*vel_mapper.get(has_vel_only).unwrap(), Velocity(0, 0));
    }

    #[test]
    #[should_panic]
    fn get_nonexistent_mapper() {
        let world = WorldBuilder::new().build();
        world.get_mapper::<Position>().expect("Unable to unwrap mapper.");
    }

    #[test]
    #[should_panic]
    fn get_nonexistent_mapper_mut() {
        let mut world = WorldBuilder::new().build();
        world.get_mapper_mut::<Position>().expect("Unable to unwrap mut mapper.");
    }
}