//! An Entity-Component-System
//! This is designed with a data-oriented approach in mind.
//! Users of the system can choose how the component data
//! for any component type they register with the world
//! is stored in memory.
//!
//! #Examples
//!
//! ```
//! use engine::ecs::*;
//!
//! #[derive(Eq, PartialEq, Debug, Clone)]
//! struct Counter(pub usize);
//!
//! struct Increment(usize);
//!
//! impl Increment {
//!     fn new() -> Increment { Increment(1) }
//! }
//!
//! unsafe impl ComponentEdit for Increment {
//!    type Item = Counter;
//!    fn is_commutative(&self) -> bool { true }
//!    fn combine_with(self, other: Increment) -> Increment {
//!        Increment(self.0 + other.0)
//!    }
//!
//!    fn apply(&self, counter: &mut Counter) { 
//!        counter.0 += self.0;
//!    }
//! }
//!
//! impl Editable for Counter {
//!     type Edit = Increment;
//! }
//!
//! struct CounterSystem;
//! impl System for CounterSystem {
//!     fn process(&mut self, handle: &WorldHandle) {
//!         for e in 
//!             handle.query()
//!             .with_component::<Counter>() {
//!             handle.submit_change(e, Increment::new())
//!         }
//!     }
//! }
//!
//! let mut world = 
//!     WorldBuilder::new()
//!     .with_component_mapper(VecMapper::<Counter>::new())
//!     .with_system(CounterSystem)
//!     .build();
//! let e = world.entity_manager().next_entity();
//! world.get_mapper_mut::<Counter>().set(e, Counter(0));
//! world.process_systems();
//! let new_count = world.get_mapper::<Counter>().get(e).unwrap();
//! assert_eq!(*new_count, Counter(1));
//! ```
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::ops::Index;

pub use self::vec_mapper::VecMapper;
pub use self::world::{EntityQuery, World, WorldBuilder, WorldHandle};
pub use self::entity::{Entity, EntityManager};

pub mod vec_mapper;
pub mod world;
pub mod entity;

/// A rust component type is any static type that can be cloned.
pub trait Component: Any + Clone {}

impl<T: Any + Clone> Component for T {}

/// A component which can be edited.
pub trait Editable: Component {
    type Edit: ComponentEdit<Item=Self> + Any;
}

/// An edit that can be applied to a component.
///
/// This is the only way changes can be made to component data.
/// they are grouped into two categories:
/// 
/// - Commutative, which can be combined in arbitrary order and yield the same result.
///
/// - Non-commutative, which cannot be combined.
///
/// It is the responsibility of the implementor of this trait to ensure that
/// the combine_with function behaves as expected and that only commutative
/// edits are identified as such.
pub unsafe trait ComponentEdit {
    /// The type of component this edit operates on.
    type Item: Editable;
    /// Combines two commutative edits together.
    /// This function will only be called with two edits which are commutative.
    ///
    /// `a.combine_with(b)` and `b.combine_with(a)` must yield the same result.
    fn combine_with(self, other: Self) -> Self;

    /// Whether this edit is commutative, or can be applied in arbitrary order
    /// with other commutative edits and yield the same result.
    fn is_commutative(&self) -> bool;

    /// Apply this edit to a component.
    fn apply(&self, &mut Self::Item);
}

/// Processes entities that contain specific components.
pub trait System {
    /// Does arbitrary work with the world.
    fn process(&mut self, &WorldHandle);
}

/// Used to initialize new entities with a specific set of components.
pub trait Prototype {
    fn initialize(&self, &mut ComponentMappers, Entity);
}

/// Maps entities to component data.
pub trait ComponentMapper {
    type Component: Component;
    
    /// Get an immutable reference to an entity's component data.
    fn get(&self, e: Entity) -> Option<&Self::Component>;
    /// Get a mutable reference to an entity's component data.
    fn get_mut(&mut self, e: Entity) -> Option<&mut Self::Component>;
    /// Sets the component data for a specific entity.
    fn set(&mut self, e: Entity, c: Self::Component);
    /// Stop managing component data for this entity.
    fn remove(&mut self, e: Entity);
    /// Get a vector of all the entities this manages.
    fn entities(&self) -> Vec<Entity>;
}

impl<T: Component> Index<Entity> for ComponentMapper<Component=T> {
    type Output = T;

    /// Indexes into the component mapper.
    /// This should only be used when an entity is known to have
    /// the component.
    fn index<'a>(&'a self, e: Entity) -> &'a T {
        let opt = self.get(e);
        debug_assert!(opt.is_some());
        opt.unwrap()
    }
}

struct MapperHandle {
    mapper: Box<Any> // Box<Box<ComponentMapper<Component=C>>
}

impl MapperHandle {
    // create a Box<Any> that aliases a Box<Box<ComponentMapper<Component=C>>
    fn from_mapper<C, M: Any>(mapper: M) -> Self 
    where C: Component, M: ComponentMapper<Component=C> + 'static {
        let mapper = Box::new(mapper) as Box<ComponentMapper<Component=C>>;
        MapperHandle {
            mapper: Box::new(mapper) as Box<Any>
        }
    }
}

/// Stores all component mappers.
pub struct ComponentMappers(HashMap<TypeId, MapperHandle>);

impl ComponentMappers {
    fn new() -> Self {
        ComponentMappers(HashMap::new())
    }

    /// insert a component mapper into 
    pub fn insert<T, M: Any>(&mut self, mapper: M)
    where T: Component, M: ComponentMapper<Component=T> + 'static {
        self.0.insert(TypeId::of::<T>(), MapperHandle::from_mapper(mapper));
    }

    /// Get an immutable reference to the component mapper for this type.
    pub fn get_mapper< 'b, 'a: 'b, T: Component>(&'a self)
                                -> &'b ComponentMapper<Component=T> {
        let mapper: Option<&Box<ComponentMapper<Component=T>>>
        = self.0.get(&TypeId::of::<T>()).and_then(|h|
            h.mapper.downcast_ref::<Box<ComponentMapper<Component=T>>>()
        );
        debug_assert!(mapper.is_some());
        &**mapper.unwrap()
    }

    /// Get a mutable reference to the component mapper for this type.
    pub fn get_mapper_mut<'b, 'a: 'b, T: Component>(&'a mut self)
                                -> &'b mut ComponentMapper<Component=T> {
        let mapper: Option<&mut Box<ComponentMapper<Component=T>>>
        = self.0.get_mut(&TypeId::of::<T>()).and_then(|h|
            h.mapper.downcast_mut::<Box<ComponentMapper<Component=T>>>()
        );
        debug_assert!(mapper.is_some());
        &mut **mapper.unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEW_ENTITIES: usize = 1000;

    #[derive(Clone)]
    struct Position {
        x: i32,
        y: i32,
    }

    struct Translate {
        x: i32,
        y: i32,
    }

    struct PositionCreator;
    impl System for PositionCreator {
        fn process(&mut self, handle: &WorldHandle) {
            // create 1000 new entities.
            handle.next_entities(NEW_ENTITIES).with_component(Position { x: 0, y: 0 });
        }
    }

    struct PositionTranslator;
    impl System for PositionTranslator {
        fn process(&mut self, handle: &WorldHandle) {
            for e in handle.query().with_component::<Position>() {
                handle.submit_change(e, Translate { x: 1, y: 1 });
            }
        }
    }

    unsafe impl ComponentEdit for Translate {
        type Item = Position;

        fn is_commutative(&self) -> bool { true }
        fn combine_with(self, other: Translate) -> Translate {
            Translate {
                x: self.x + other.x,
                y: self.y + other.y,
            }
        }
        fn apply(&self, pos: &mut Position) {
            pos.x += self.x;
            pos.y += self.y;
        }
    }

    impl Editable for Position {
        type Edit = Translate; // only translations are allowed.
    }

    #[test]
    pub fn basic_test() {
        let mut world = WorldBuilder::new()
            .with_component_mapper(VecMapper::<Position>::new())
            .with_system(PositionCreator)
            .with_system(PositionTranslator)
            .build();

        for _ in 0..2 { world.process_systems() }
    }
}