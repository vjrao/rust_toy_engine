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
use std::cell::{RefCell, Ref, RefMut};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex, MutexGuard};

use self::entity::{Entity, EntityManager};

mod entity;

/// A Component type must be safe to bitwise copy.
pub trait Component: Any + Copy { 
    fn id() -> ComponentId where Self: Sized {
        ComponentId(TypeId::of::<Self>())
    }
}

/// A component type's unique id.
#[derive(PartialEq, Eq, Hash)]
pub struct ComponentId(TypeId);


/// Wraps two `Cow`s: one for reading, which will never be copied,
/// and one for writing, which is wrapped in a mutex.
#[derive(Clone)]
struct CowHandle<'a, T: Clone + 'a> {
    read: Cow<'a, T>,
    write: Arc<Mutex<Cow<'a, T>>>
}

/// Maps entities to arbitrary data.
/// Just a type alias to HashMap right now. Later, will improve.
#[derive(Clone)]
struct DataMap<T> {
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

    fn make_handle(&self) -> CowHandle<Self> {
        let borrowed = Cow::Borrowed(self);
        // would preferably do this in a way that doesn't
        // incur the cost of creating a mutex every time.
        CowHandle {
            read: borrowed.clone(),
            write: Arc::new(Mutex::new(borrowed)),
        }
    }
    // iterator functions.
}

/// Operations that all data maps must have. These are untyped.
trait DataOps {
    /// prunes dead entities away.
    fn prune(&mut self, &EntityManager);

    /// Make a `CowData` from this
    fn make_cow(&self) -> CowData;

    /// Update from a given `CowData`. Guaranteed to be the same
    /// type as this produces, behind reflection.
    fn update_from_cow<'a>(&'a mut self, CowData<'a>);

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

    fn make_cow<'a>(&'a self) -> CowData<'a> {
        use std::mem;

        let static_self: &'static Self = unsafe { mem::transmute(self) };
        let static_cow: CowHandle<'static, DataMap<T>> = static_self.make_handle();
        CowData {
            any: Box::new(static_cow),
            _marker: PhantomData,
        }
    }

    fn update_from_cow<'a>(&'a mut self, cow: CowData<'a>) {
        let handle: Box<CowHandle<'static, Self>> = cow.any.downcast().unwrap();
        let mutex = match Arc::try_unwrap(handle.write) {
            Ok(val) => val,
            Err(_) => panic!("Failed to unwrap arc."),
        };

        let cow = match mutex.into_inner() {
            Ok(val) => val,
            Err(poisoned) => poisoned.into_inner()
        };

        // only update if the data got changed.
        if let Cow::Owned(data) = cow {
            *self = data
        }
    }
}

/// Wraps an untyped reference to a CowHandle for a specific data map.
struct CowData<'a> {
    // unsafe code is looking inevitable here...
    any: Box<Any>,
    _marker: PhantomData<&'a Any>,
}

/// All the data associated with a specific Component type.
/// All of its fields are Rcs that refer to the same underlying object.
struct ComponentData {
    any: Option<Rc<Any>>,
    data_ops: Weak<DataOps>,
}

impl DataOps for ComponentData {
    // all the expects that occur in these functions should never trigger.

    fn prune(&mut self, em: &EntityManager) {
        self.use_data_ops(|data_ops| data_ops.prune(em));
    }

    fn make_cow(&self) -> CowData {
        let data_ops = self.data_ops.upgrade().expect("Failed to upgrade");
        let cow = data_ops.make_cow();
        CowData {
            any: cow.any,
            _marker: PhantomData,
        }
    }

    fn update_from_cow<'a>(&'a mut self, cow: CowData<'a>) {
        self.use_data_ops(|data_ops| data_ops.update_from_cow(cow));
    }
}

impl ComponentData {
    // all the expects that occur in these functions should never trigger.

    fn new<T: Component>() -> Self {
        let data = Rc::new(DataMap::<T>::new());
        ComponentData {
            any: Some(data.clone()),
            data_ops: Rc::downgrade(&(data as Rc<DataOps>)),
        }
    }

    /// Try to get a reference to the data as a concrete type.
    fn get_as<T: Component>(&self) -> Option<&DataMap<T>> {
        let any = self.any.as_ref().expect("Component data is None.");
        if any.is::<T>() { 
            Some(any.downcast_ref().expect("Failed to downcast component data."))
        } else {
            None
        }
    }

    /// Try to get a mutable reference to the data as a concrete type.
    fn get_mut<T: Component>(&mut self) -> Option<&mut DataMap<T>> {
        let mut_ref = self.any.as_mut().expect("Component data is None.");
        if mut_ref.is::<T>() {
            let any = Rc::get_mut(mut_ref).expect("More than one strong ref to component data.");
            Some(any.downcast_mut().expect("Failed to downcast component data."))
        } else {
            None
        }
    }

    /// Use the data ops in some way.
    fn use_data_ops<F>(&mut self, f: F) where F: FnOnce(&mut DataOps) {
        // I haven't done too much research into the circumstances
        // in which llvm will perform devirtualisation.
        // However, I wouldn't be surprised if those circumstances
        // entail never reassigning trait objects.
        // If so, this function destroys those guarantees.

        // upgrade the data ops first, so we don't drop them both.
        let mut up = self.data_ops.upgrade().expect("Failed to upgrade.");
        // Now take out the strong ref to any, downgrade it, and destroy it.
        let any = self.any.take().expect("Component data is None.");
        let temp_down = Rc::downgrade(&any);
        drop(any);

        // only strong ref is the upgraded DataOps now.
        let data_ops = Rc::get_mut(&mut up).expect("More than one strong ref");
        f(data_ops);

        self.any = Some(temp_down.upgrade().expect("Failed to upgrade."))

        // the hoops I jump through to use safe code...
    }
}

// TODO (rphmeier): Make this generic over a recursive-variadic
// that stores the component types. This will yield a variety
// of improvements:
// - No reflection for single-component arrays (big win)
// - Enables work in serialization
pub struct World {
    entity_manager: EntityManager,
    data_maps: HashMap<ComponentId, ComponentData>,
}

impl World {
    fn process(&mut self) {
        // make a world handle. this involves making a CowHandle
        // for each DataMap, and a CowHandle for the EntityManager.
        let em = CowHandle {
            read: Cow::Borrowed(&self.entity_manager),
            write: Arc::new(Mutex::new(Cow::Borrowed(&self.entity_manager))),
        };

        let mut cow_data = HashMap::new();
        for (id, data) in self.data_maps.iter() 

        let wh = WorldHandle {
            entity_manager: em,
        };

        {
            // run threaded processors here
        }

        // run sync processors

        // write out the 
    }
}

#[derive(Clone)]
pub struct WorldHandle<'a> {
    entity_manager: CowHandle<'a, EntityManager>,
    data_maps: HashMap<ComponentId, CowData>,
}

#[cfg(test)]
mod tests {
}