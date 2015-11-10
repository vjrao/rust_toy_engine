//! This is an entity component system designed for parallelism.
//! Every component type has data stored in its own array.
//!
//! To process, we copy all arrays and wrap each one in a mutex.
//! The old arrays become "read-only" and the new array is locked.
//! Each processor reads data from the read-only, applies changes,
//! and commits its changes.
//!
//! Processors are specified to run in arbitrary order, which means 
//! it is the burden of the user to avoid race conditions.
//! In the future, processors may be registered with a priority
//! to make this easier.

use std::any::{Any, TypeId};
use std::borrow::Cow;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard};

use self::component_map::*;
use self::entity::{Entity, EntityManager};

mod component_map;
mod entity;

/// A Component type must be safe to bitwise copy.
pub trait Component: Any + Copy { 
    fn id() -> ComponentId where Self: Sized {
        ComponentId(TypeId::of::<Self>())
    }
}

/// A component type's unique id.
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
pub struct ComponentId(TypeId);


/// Wraps two `Cow`s: one for reading, which will never be copied,
/// and one for writing, which is wrapped in a mutex.
#[derive(Clone)]
struct CowHandle<'a, T: Clone + 'a> {
    read: Cow<'a, T>,
    write: Arc<Mutex<Cow<'a, T>>>
}

impl<'a, T: Clone + 'a> CowHandle<'a, T> {
    fn new(val: &'a T) -> Self {
        let borrowed = Cow::Borrowed(val);
        // would preferably do this in a way that doesn't
        // incur the cost of creating a mutex every time.
        CowHandle {
            read: borrowed.clone(),
            write: Arc::new(Mutex::new(borrowed)),
        }
    }

    fn get_read(&self) -> &T {
        & *self.read
    }

    fn lock_write(&self) -> MutexGuard<Cow<'a, T>> {
        match self.write.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Consumes itself, returning Some with any new data.
    /// None means no changes were made or that this handle is not unique.
    fn into_inner(self) -> Option<T> {
        let mutex = match Arc::try_unwrap(self.write) {
            Ok(val) => val,
            _ => return None,
        };
        let cow = match mutex.into_inner() {
            Ok(val) => val,
            Err(poisoned) => poisoned.into_inner()
        };

        match cow {
            Cow::Owned(data) => Some(data),
            _ => None
        }
    }
}

/// Maps entities to arbitrary data.
/// Just a type alias to HashMap right now. Later, will improve.
#[derive(Clone)]
pub struct DataMap<T> {
    data: HashMap<Entity, T>,
}

impl<T: Clone> DataMap<T> {
    fn new() -> Self {
        DataMap {
            data: HashMap::new()
        }
    }
    fn set(&mut self, e: Entity, data: T) {
        self.data.insert(e, data);
    }

    fn get(&self, e: Entity) -> Option<&T> {
        self.data.get(&e)
    }
    // iterator functions.
}

/// Operations that all data maps must have. These are untyped.
trait DataOps {
    /// prunes dead entities away.
    fn prune(&mut self, &EntityManager);

    // functions for getting/setting binary data
    // will probably want to have some kind of unsafe API/trait where
    // you can register data processing plugins.
}

impl<T: Clone + Any> DataOps for DataMap<T> {
    fn prune(&mut self, em: &EntityManager) {
        // simple implementation for now.
        // may want to move this to a no-alloc version later.
        // when i change how DataMap itself functions, we'll see.
        let mut to_remove = Vec::new();
        for entity in self.data.keys() {
            if !em.is_alive(*entity) {
                to_remove.push(*entity);
            }
        }

        for entity in to_remove {
            self.data.remove(&entity);
        }
    }

}

trait Same<T> {}
impl<A> Same<A> for A {}

/// A handle for writing. Used in conjunction with DataArray.
#[repr(C)]
struct WriteHandle<'a, T: 'a + Clone> {
    guard: MutexGuard<'a, Cow<'a, T>>
}

impl<'a, T: 'a + Clone> Deref for WriteHandle<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        & **self.guard
    }
}

impl<'a, T: 'a + Clone> DerefMut for WriteHandle<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.to_mut()
    }
}

/// A processor does some work every tick.
pub trait Processor {
    fn process<T: WorldHandle + Clone>(&mut self, world_handle: T) where Self: Sized;
}

impl<A: Processor, B: Processor> Processor for (A, B) {
    fn process<T: WorldHandle + Clone>(&mut self, world_handle: T) {
        self.0.process(world_handle.clone());
        self.1.process(world_handle);
    }
}

pub trait WorldHandle {
}


/// The concrete implementation of a `World`. Parameterised
/// by the traits in the component_map module.
pub struct World<T, P: Processor> {
    entity_manager: EntityManager,
    components: T,
    // one or more processors.
    processors: P,
}

/// The WorldHandle allows access to operations on components
/// and entities.
/// This is produced by the world and is how the bulk
/// of the work is done.
#[derive(Clone)]
struct ConcreteWorldHandle<'a, T: 'a> {
    entity_manager: CowHandle<'a, EntityManager>,
    components: T,
}

impl<'a, T: SyncComponentMap> WorldHandle for ConcreteWorldHandle<'a, T> {

}

impl<T: ComponentMap, P: Processor> World<T, P>
where for <'a> T: MakeCowMap<'a> {
    pub fn process(&mut self){
        use std::mem;
        let new_values = {
            let handle = ConcreteWorldHandle {
                entity_manager: CowHandle::new(&self.entity_manager),
                components: self.components.make_cow_map(),
            };

            self.processors.process(handle.clone());

            handle.components.into_owned_map()
        };

        self.components.update_from(new_values);
    }
}
#[cfg(test)]
mod tests {
}