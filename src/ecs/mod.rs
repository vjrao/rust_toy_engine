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
//!     fn process(&mut self, entities: &[Entity], mappers: &mut ComponentMappers) {
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
//! let new_count = world.get_mapper::<Counter>().unwrap().get(e).unwrap();
//! assert_eq!(*new_count, Counter(1));
//! ```
use std::any::{Any, TypeId};

pub use self::iter::*;
pub use self::mapper::{ComponentMapper, ComponentMappers};
pub use self::vec_mapper::VecMapper;
pub use self::world::{World, WorldBuilder, EntityManager};

pub mod iter;
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
pub trait Component: Any {
    type Filter;
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
        fn process(&mut self, mappers: &mut ComponentMappers) {
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
        let e = world.next_entity();

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
            .with_system(MovementSystem)
            .build();

        let has_both = world.next_entity();
        let has_pos_only = world.next_entity();
        let has_vel_only = world.next_entity();

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
        world.get_mapper_mut::<Position>().unwrap().set(before, Position(6, 9));
        world.get_mapper_mut::<Velocity>().unwrap().set(before, Velocity(0, 0));
        world.get_mapper_mut::<Position>().unwrap().set(after, Position(6, 9));
        world.get_mapper_mut::<Velocity>().unwrap().set(after, Velocity(0, 0));

        world.destroy_entity(before);
        world.process_systems();
        world.destroy_entity(after);

        assert_eq!(world.get_mapper::<Position>().unwrap().get(before), None);
        assert_eq!(world.get_mapper::<Velocity>().unwrap().get(before), None);
        assert_eq!(world.get_mapper::<Position>().unwrap().get(after), None);
        assert_eq!(world.get_mapper::<Velocity>().unwrap().get(after), None);
    }

    #[test]
    fn prototype_initialization() {
        struct PositionProto(i32, i32);
        impl Prototype for PositionProto {
            fn initialize(&self, e: Entity, mappers: &mut ComponentMappers) {
                mappers.get_mapper_mut().unwrap().set(e, Position(self.0, self.1));
            }
        }

        let mut world = 
            WorldBuilder::new()
            .with_component_mapper(VecMapper::<Position>::new())
            .build();

        let origin_proto = PositionProto(0, 0);
        let e = world.next_entity_prototyped(&origin_proto);

        assert_eq!(*world.get_mapper::<Position>().unwrap().get(e).unwrap(), Position(0, 0));
    }
}