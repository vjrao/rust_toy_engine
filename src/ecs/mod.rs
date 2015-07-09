use std::collections::{HashSet, HashMap};
use std::any::{TypeId, Any};
use std::raw::TraitObject;
use std::mem;

pub use self::vec_mapper::VecMapper;

mod vec_mapper;

/// An unique entity.
/// Entities each have a unique id which serves
/// as a weak pointer to the entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
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

/// A rust component type is any static type.
pub trait Component: Any { }
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
    /// Prune dead entities from this manager.
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

///used to maintain ownership of the mapper so it destructs properly.
struct MapperHandle {
    obj: TraitObject,
    handle: Box<ComponentMapperExt>,
}

impl MapperHandle {
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

/// The world ties together entities, components, and systems.
pub struct World {
    pub entity_manager: EntityManager,
    component_mappers: HashMap<TypeId, MapperHandle>
}
impl World {
    /// Get an immutable reference to the component mapper for this type.
    pub fn get_mapper<T: Component>(&self)
                                -> Option<&ComponentMapper<Component=T>> {
        match self.component_mappers.get(&TypeId::of::<T>()) {
            Some(data) => {
                Some( unsafe { mem::transmute(data.obj) } )
            }
            None => None
        }
    }

    /// Get a mutable reference to the component mapper for this type
    pub fn get_mapper_mut<T: Component>(&mut self)
                                -> Option<&mut ComponentMapper<Component=T>> {
        match self.component_mappers.get(&TypeId::of::<T>()) {
            Some(data) => {
                Some( unsafe { mem::transmute(data.obj) } )
            }
            None => None
        }
    }
}

/// Factory for `World`. 
pub struct WorldBuilder {
    component_mappers: HashMap<TypeId, MapperHandle>
}

impl WorldBuilder {
    /// Create a new `WorldBuilder`
    pub fn new() -> WorldBuilder {
        WorldBuilder { component_mappers: HashMap::new() }
    }

    /// Add a component mapper to this world.
    pub fn with_component_mapper<T, M>(self, mapper: M) -> WorldBuilder
    where T: Component, M: ComponentMapper<Component=T> + Any {
        let mut s = self;
        s.component_mappers.insert(
            TypeId::of::<M::Component>(),
            MapperHandle::from_mapper(mapper)
        );
        s
    }

    /// Builds a world from self.
    pub fn build(self) -> World {
        World {
            entity_manager: EntityManager::new(),
            component_mappers: self.component_mappers
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Eq, PartialEq)]
    struct Position(i32, i32);
    struct Velocity(i32, i32);
    
    #[test]
    fn build_world() {
        let world = WorldBuilder::new().build();
    }

    #[test]
    fn get_and_set_value() {
        let mut world = 
            WorldBuilder::new()
            .with_component_mapper(VecMapper::<Position>::new())
            .build());
        let e = world.entity_manager.next();

        {
            let mut pos_mapper = world.get_mapper_mut::<Position>().unwrap();
            pos_mapper.set(e, Position(6, 9))
        }

        let pos_mapper = world.get_mapper::<Position>().unwrap();
        let pos = pos_mapper.get(e).unwrap();
        assert_eq!(*pos, Position(6, 9))
    }

}