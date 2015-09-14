//! An Entity-Component-System
//! This is designed with a data-oriented approach in mind.
//! Users of the system can choose how the component data
//! for any component type they register with the world
//! is stored in memory.
//!
//! #Examples
//!
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::mem;

use self::component_managers::{ComponentManagers, ManagerLookup};
use self::entity::{Entity, EntityManager};

mod entity;
mod component_managers;

pub trait Component: Any + Clone {
}

/// Different messages component managers can receive to update the component data they're storing.
pub enum Delta<T: Component> {
    Destroyed(Entity),
    Set(Entity, T),
}

/// Does work on every entity with a set of components.
pub trait Processor {
    // Takes a component manager for the type of component this produces.
    fn process(&mut self, WorldHandle);
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
    _db: ComponentDB<M>
}


/// Passed to systems, allows for operations on the world
/// such as querying for entities and submitting edits.
pub struct WorldHandle;

#[derive(Clone)]
pub struct ComponentDB<M> {
    entries: HashMap<Vec<usize>, usize>,
    managers: M
}

impl<M: ManagerLookup> ComponentDB<M> {
    // look up the spec, increment count, fetch query that exists.
    fn query<'a, Q: QuerySpec>(&'a self, spec: Q) -> ColdQuery<'a, Q, M> {
        let _unique_parts = spec.depths(&self.managers).sort();

        // for now, just return a cold query.
        ColdQuery::new(spec, &self.managers)
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