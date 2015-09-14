use std::any::TypeId;
use std::marker::PhantomData;
use std::mem;

use super::{Component, ComponentManager};

/// Something that can look up managers.
///
/// In practice, this is almost always going to be a `ComponentManagers`,
/// but this trait exists as a way to to facilitate recursive generic type
/// declarations.
pub trait ManagerLookup: Clone {
    /// Returns a unique depth of this component type or `None`.
    /// While this seems relatively useless at first glance,
    /// it can be used to give each component a unique id in the world.
    fn depth_of<T: Component>(&self) -> Result<usize, usize>
    where Self: Sized; 

    /// Try to get a reference to a manager for T.
    fn lookup<T: Component>(&self) -> Option<&ComponentManager<T>>
    where Self: Sized;

    /// Try to get a mutable reference to a manager for T.
    fn lookup_mut<T: Component>(&mut self) -> Option<&mut ComponentManager<T>>
    where Self: Sized;

    /// Adds a manager on to this manager lookup
    fn and<M, T>(self, manager: M) -> ComponentManagers<Self, M, T>
    where Self: Sized, M: ComponentManager<T>, T: Component {
        ComponentManagers {
            nested: self,
            top_level: manager,
            _marker: PhantomData,
        }
    }
}

impl ManagerLookup for () {
    fn depth_of<T: Component>(&self) -> Result<usize, usize> { Err(0) }
    fn lookup<T: Component>(&self) -> Option<&ComponentManager<T>> {
        None
    }

    fn lookup_mut<T: Component>(&mut self) -> Option<&mut ComponentManager<T>> {
        None
    }
}

/// A conglomeration of component managers. Very fast to query.
#[derive(Clone)]
pub struct ComponentManagers<L, M, C> {
    nested: L,
    top_level: M,
    _marker: PhantomData<C>
}

impl<L, M, C> ComponentManagers<L, M, C>
where L: ManagerLookup, M: ComponentManager<C> + Clone, C: Component {
    pub fn new(manager: M) -> ComponentManagers<(), M, C> {
        ().and(manager)
    }
}

impl<L, M, C> ManagerLookup for ComponentManagers<L, M, C>
where L: ManagerLookup, M: ComponentManager<C> + Clone, C: Component {
    fn depth_of<T: Component>(&self) -> Result<usize, usize> {
        match self.nested.depth_of::<T>() {
            Ok(depth) => Ok(depth),
            Err(acc) => {
                if TypeId::of::<T>() == TypeId::of::<C>() { Ok(acc + 1) }
                else { Err(acc + 1) }
            }
        }
    }

    fn lookup<T: Component>(&self) -> Option<&ComponentManager<T>> {
        if TypeId::of::<T>() == TypeId::of::<C>() {
            Some(unsafe {
                mem::transmute(&self.top_level as &ComponentManager<C>)
            })
        } else {
            self.nested.lookup()
        }
    }

    fn lookup_mut<T: Component>(&mut self) -> Option<&mut ComponentManager<T>> {
        if TypeId::of::<T>() == TypeId::of::<C>() {
            Some(unsafe {
                mem::transmute(&mut self.top_level as &mut ComponentManager<C>)
            })
        } else {
            self.nested.lookup_mut()
        }
    }
}