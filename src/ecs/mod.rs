//! An Entity-Component-System
//! This is designed with a data-oriented approach in mind.
//! Users of the system can choose how the component data
//! for any component type they register with the world
//! is stored in memory.
//!
//! #Examples
//!

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub use self::entity::*;
pub use self::vec_manager::VecManager;
pub use self::component_managers::{ComponentManagers, ManagerLookup};

use self::query::*;

mod entity;
mod component_managers;
mod query;
mod vec_manager;

pub trait Component: Any + Clone { }

/// Different messages component managers can receive to update the component data they're storing.
pub enum Delta<T: Component> {
    Destroyed(Entity),
    Set(Entity, T),
}

/// Does work on every entity with a set of components.
pub trait Processor<M: ManagerLookup> {
    // Takes a handle to the world and does some work.
    fn process<'a>(&mut self, WorldHandle<'a, M>);
}

/// Stores component data.
pub trait ComponentManager<T: Component> {
    /// Create an vector of all pairs of entities and Component data.
    fn component_data<'a>(&'a self) -> Vec<(Entity, &'a T)>;

    /// Get a reference to the component data for an entity.
    fn get(&self, Entity) -> Option<&T>;

    /// Set the component data for a specific entity.
    fn set(&mut self, Entity, T);

    /// remove the component data for a specific entity.
    fn remove(&mut self, Entity) -> Option<T>;
}

pub struct World<M: ManagerLookup> {
    db: ComponentDB<M>,
    entity_manager: EntityManager,
    processors: Vec<Box<Processor<M>>>
}

impl<M: ManagerLookup> World<M> {
    /// Create a new `World`.
    fn new(managers: M) -> Self {
        World {
            db: ComponentDB {
                entries: HashMap::new(),
                managers: managers
            },
            entity_manager: EntityManager::new(),
            processors: Vec::new(),
        }
    }

    /// Adds a processor to this world.
    fn add_processor<P: Processor<M> + 'static>(&mut self, processor: P) {
        self.processors.push(Box::new(processor));
    }

    /// Processes all processors.
    fn process(&mut self) {
        let mut new_db = self.db.clone();

        {
            let world = WorldHandle {
                old_db: &self.db,
                new_db: Arc::new(Mutex::new(&mut new_db)),
                entity_manager: &self.entity_manager,
            };

            // TODO: parallelize this.
            for processor in &mut self.processors {
                processor.process(world.clone())
            }
        }

        self.db = new_db;
    }
}

/// Passed to systems, allows for operations on the world
/// such as querying for entities and submitting edits.
#[derive(Clone)]
pub struct WorldHandle<'a, M: 'a + ManagerLookup> {
    old_db: &'a ComponentDB<M>,
    new_db: Arc<Mutex<&'a mut ComponentDB<M>>>,
    entity_manager: &'a EntityManager,
}

impl<'a, M: 'a + ManagerLookup> WorldHandle<'a, M> {
    pub fn query(&self) -> Query<'a, M, ()> {
        Query {
            db: self.old_db,
            entity_manager: self.entity_manager,
            query: ()
        }
    }
}

pub struct Query<'a, M: 'a, Q> {
    db: &'a ComponentDB<M>,
    entity_manager: &'a EntityManager,
    query: Q,
}

impl<'a, M: 'a + ManagerLookup, Q: QuerySpec> Query<'a, M, Q> {
    fn with_component<C: Component>(self) -> Query<'a, M, Result<SpecializedQuerySpec<Q, C>, Q>> {
        Query {
            db: self.db,
            entity_manager: self.entity_manager,
            query: self.query.with_component::<C>()
        }
    }
}

impl<'a, M: 'a + ManagerLookup, Q: QuerySpec> IntoIterator for Query<'a, M, Q> {
    type IntoIter = ColdQuery<'a, Q, M>;
    type Item = ColdQueryItem<'a, Q, M>;

    fn into_iter(self) -> ColdQuery<'a, Q, M> {
        self.db.query(self.query, self.entity_manager)
    }
}

#[derive(Clone)]
pub struct ComponentDB<M> {
    entries: HashMap<Vec<usize>, usize>,
    managers: M
}

impl<M: ManagerLookup> ComponentDB<M> {
    // look up the spec, increment count, fetch query that exists.
    fn query<'a, Q: QuerySpec>(&'a self, spec: Q, em: &EntityManager) -> ColdQuery<'a, Q, M> {
        let _unique_parts = spec.depths(&self.managers).sort();

        // for now, just return a cold query.
        ColdQuery::new(spec, &self.managers, em)
        // incrementing count requires interior mutability
    }

    // apply a delta to a mapper 
    fn _apply_delta<T>(&mut self, delta: Delta<T>) where T: Component {
        let m = self.managers.lookup_mut::<T>().expect("no mapper found for component");
        match delta {
            Delta::Destroyed(e) => { m.remove(e); }
            Delta::Set(e, data) => { m.set(e, data); },
        }
        // TODO: store the delta so queries can access it later
    }

    // perform final actions for the update.
    fn _finalize_update(&mut self) {
        // cache queries that need caching

        // have cached queries access all the deltas they want.
    }
}

#[cfg(test)]
mod tests {
}