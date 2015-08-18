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
//! #[derive(Eq, PartialEq, Debug)]
//! struct Counter(pub u32);
//! struct CounterSystem;
//! impl System for CounterSystem {
//!     fn process(&mut self, mappers: &ComponentMappers, em: &EntityManager) {
//!         let entities: Vec<Entity> = 
//!             mappers.query()
//!             .with_component::<Counter>()
//!             .into_iter().collect();
//!         let count_mapper = mappers.get_mapper_mut::<Counter>().unwrap();
//!         for e in entities {
//!            count_mapper.get_mut(*e).unwrap().0 += 1;
//!         }
//!     }
//! }
//!
//! let mut world = 
//!     WorldBuilder::new()
//!     .with_component_mapper(VecMapper::<Counter>::new())
//!     .with_system(CounterSystem)
//!     .build();
//! let e = world.next_entity();
//! world.get_mapper_mut::<Counter>().unwrap().set(e, Counter(0));
//! world.process_systems();
//! let new_count = world.get_mapper::<Counter>()[e].unwrap();
//! assert_eq!(*new_count, Counter(1));
//! ```
use std::any::Any;

pub use self::mapper::{ComponentMapper, ComponentMappers};
pub use self::vec_mapper::VecMapper;
pub use self::world::{World, WorldBuilder, EntityManager};

pub mod mapper;
pub mod vec_mapper;
pub mod world;

/// An unique entity.
/// Entities each have a unique id which serves
/// as a weak pointer to the entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Entity {
    id: usize,
}

/// A rust component type is any static type.
pub trait Component: Any {}

impl<T: Any> Component for T {}

/// A component which can be filtered by its properties.
pub trait Filterable: Component {
    type Filter;
    /// Whether this component is accepted by this filter.
    fn is_accepted_by(&self, filter: &Self::Filter) -> bool;
}

/// A component which can be edited.
pub trait Editable: Component {
    type Edit: ComponentEdit;
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
/// the combine_with function behaves as expected.
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
    fn process(&mut self, &ComponentMappers, &EntityManager);
}

/// Used to initialize new entities with a specific set of components.
pub trait Prototype {
    fn initialize(&self, Entity, &mut ComponentMappers);
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
        fn process(&mut self, mappers: &ComponentMappers, em: &EntityManager) {
            // calculate new positions
            let mut new_positions = Vec::new();
            {
                let pos_mapper = mappers.get_mapper::<Position>();
                let vel_mapper = mappers.get_mapper::<Velocity>();
                for e in mappers.query()
                .with_component::<Position>()
                .with_component::<Velocity>() {
                    let pos = pos_mapper[e];
                    let vel = vel_mapper[e];

                    let new_pos = Position(pos.0 + vel.0, pos.1 + vel.1);
                    new_positions.push((e, new_pos));
                }
            }

            // set new positions.
            let mut pos_mapper = mappers.get_mapper_mut::<Position>();
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
        let e = world.next_entity();

        {
            let mut pos_mapper = world.get_mapper_mut::<Position>();
            pos_mapper.set(e, Position(6, 9))
        }

        let pos_mapper = world.get_mapper::<Position>();
        assert_eq!(*pos_mapper[e], Position(6, 9));
    }

    #[test]
    fn test_simple_system() {
        let mut world = 
            WorldBuilder::new()
            .with_component_mapper(VecMapper::<Position>::new())
            .with_component_mapper(VecMapper::<Velocity>::new())
            .with_system(MovementSystem)
            .build();

        let has_both = world.next_entity();
        let has_pos_only = world.next_entity();
        let has_vel_only = world.next_entity();

        {
            let mut pos_mapper = world.get_mapper_mut::<Position>();
            pos_mapper.set(has_both, Position(0, 0));
            pos_mapper.set(has_pos_only, Position(0, 0))
        }
        {
            let mut vel_mapper = world.get_mapper_mut::<Velocity>();
            vel_mapper.set(has_both, Velocity(3, 4));
            vel_mapper.set(has_vel_only, Velocity(0, 0));
        }

        world.process_systems();

        let pos_mapper = world.get_mapper::<Position>();
        let vel_mapper = world.get_mapper::<Velocity>();

        assert_eq!(*pos_mapper[has_both], Position(3, 4));
        assert_eq!(*vel_mapper[has_both], Velocity(3, 4));

        assert_eq!(*pos_mapper[has_pos_only], Position(0, 0));
        assert_eq!(vel_mapper.get(has_pos_only), None);

        assert_eq!(pos_mapper.get(has_vel_only), None);
        assert_eq!(*vel_mapper[has_vel_only], Velocity(0, 0));
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

    #[test]
    fn destroy_entity() {
        let mut world = 
            WorldBuilder::new()
            .with_component_mapper(VecMapper::<Position>::new())
            .with_component_mapper(VecMapper::<Velocity>::new())
            .with_system(MovementSystem)
            .build();

        let before = world.next_entity();
        let after = world.next_entity();
        world.get_mapper_mut::<Position>().set(before, Position(6, 9));
        world.get_mapper_mut::<Velocity>().set(before, Velocity(0, 0));
        world.get_mapper_mut::<Position>().set(after, Position(6, 9));
        world.get_mapper_mut::<Velocity>().set(after, Velocity(0, 0));

        world.destroy_entity(before);
        world.process_systems();
        world.destroy_entity(after);

        assert_eq!(world.get_mapper::<Position>().get(before), None);
        assert_eq!(world.get_mapper::<Velocity>().get(before), None);
        assert_eq!(world.get_mapper::<Position>().get(after), None);
        assert_eq!(world.get_mapper::<Velocity>().get(after), None);
    }

    #[test]
    fn prototype_initialization() {
        struct PositionProto(i32, i32);
        impl Prototype for PositionProto {
            fn initialize(&self, e: Entity, mappers: &mut ComponentMappers) {
                mappers.get_mapper_mut().set(e, Position(self.0, self.1));
            }
        }

        let mut world = 
            WorldBuilder::new()
            .with_component_mapper(VecMapper::<Position>::new())
            .build();

        let origin_proto = PositionProto(0, 0);
        let e = world.next_entity_prototyped(&origin_proto);

        assert_eq!(*world.get_mapper::<Position>()[e], Position(0, 0));
    }
}