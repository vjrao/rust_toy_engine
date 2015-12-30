//! This module contains component-related types and traits.
//!
//!
//! Included are some type-level component lists.
//! this disgusting type system abuse culminates in various types of mixed-type component lists.
//! These aren't really to be used externally, but instead provide some level of convenience
//! and abstraction over the components which worlds operate over.
//!
//! These lists will most likely have O(1) access to members, at least when compiled with optimizations.
//! If `TypeId::of` ever becomes a const fn, they definitely will.

use memory::Vector;

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::mem;
use std::sync::{Mutex, MutexGuard};

use super::entity::{self, Entity, EntityManager};
use super::world::WorldAllocator;

/// Components are arbitrary data associated with entities.
pub trait Component: Any + Send + Sync {}

impl<T: Any + Send + Sync> Component for T {}

/// Maps entities to component data. This must be supplied with an entity manager
/// to be made usable. 
pub struct ComponentMap<T: Component> {
    indices: Vector<Option<usize>, WorldAllocator>,
    data: Vector<Entry<T>, WorldAllocator>,
    
    // fifo deque / linked-list of free 
    // indices.
    first_free: Option<usize>,
    last_free: Option<usize>,
}

enum Entry<T> {
    Full(FullEntry<T>),
    Empty(Option<usize>),
}

impl<T> Entry<T> {
    // Attempts to take the data out of this entry.
    // Replaces this entry with an empty one with no next index.
    fn take(&mut self) -> Option<FullEntry<T>> {
        use std::mem;
        
        let entry = Entry::Empty(None);
        
        match mem::replace(self, entry) {
            Entry::Full(full) => Some(full),
            Entry::Empty(next) => {
                *self = Entry::Empty(next);
                None
            }
        }
    }
}

struct FullEntry<T> {
    data: T,
}

impl<T: Component> ComponentMap<T> {
    // Pop the first value from the free list.
    // None if there isn't one.
    fn freelist_pop(&mut self) -> Option<usize> {
        let next = self.first_free.take();
        if let &Some(ref next_index) = &next {
            match &self.data[*next_index] {
                &Entry::Empty(ref next) => {
                    self.first_free = next.clone();
                }
                // nodes whose indices are in the freelist should
                // really be empty
                _ => unreachable!(),
            }
        } else {
            self.last_free = None;
        }
        
        next
    }
    
    // Pushes a free index onto the free list.
    // Does not alter the data entry for that index.
    fn freelist_push(&mut self, index: usize) {
        let last_free = self.last_free.take();
        if let &Some(ref last_index) = &last_free {
            // pushing onto non-empty list
            self.data[*last_index] = Entry::Empty(Some(index));
        } else {
            // pushing onto empty list.
            self.first_free = Some(index);
        }
        
        self.last_free = Some(index);
    }
    
    pub fn new(alloc: WorldAllocator) -> Self {
        ComponentMap {
            indices: Vector::with_alloc(alloc),
            data: Vector::with_alloc(alloc),
            first_free: None,
            last_free: None,
        }
    }
    
    /// Supply this ComponentMap with an EntityManager to get a usable handle.
    pub fn supply<'a>(&'a mut self, entity_manager: &'a EntityManager) -> MapHandle<'a ,T> {
        // ensure capacity for indices so unchecked indexing always works.
        let len = entity_manager.size_hint();
        self.indices.reserve(len);
        while self.indices.len() < len {
            self.indices.push(None);
        }
        
        MapHandle {
            map: self,
            entity_manager: entity_manager,
        }
    }
}

pub struct MapHandle<'a, T: 'a + Component> {
    map: &'a mut ComponentMap<T>,
    entity_manager: &'a EntityManager, 
}

impl<'a, T: 'a + Component> MapHandle<'a, T> {
    /// Set the data stored for `e` to the value stored in `data`.
    pub fn set(&mut self, e: Entity, data: T) {
        if !self.entity_manager.is_alive(e) { return; }
        
        let entity_index = entity::index_of(e);
        let entry = Entry::Full(FullEntry {
            data: data,
        });
        
        if let Some(index) = self.map.indices[entity_index].clone() {
            self.map.data[index] = entry;
        } else if let Some(index) = self.map.freelist_pop() {
            self.map.data[index] = entry;
            self.map.indices[entity_index] = Some(index);
        } else {
            self.map.data.push(entry);
            self.map.indices[entity_index] = Some(self.map.data.len() - 1);
        }
    }
    
    /// Try to get a reference to the data stored for `e`.
    /// Returns None if there isn't any.
    pub fn get(&self, e: Entity) -> Option<&T> {
        if !self.entity_manager.is_alive(e) { return None; }
        
        if let &Some(ref index) = &self.map.indices[entity::index_of(e)] {
            match &self.map.data[*index] {
                &Entry::Full(ref full) => Some(&full.data),
                // if we're storing an index for an entity, the entry is guaranteed to be full.
                _ => unreachable!(), 
            }
        } else {
            None
        }
    }
    
    /// Try to get a mutable reference to the data stored for `e`.
    /// Returns None if there isn't any.
    pub fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        if !self.entity_manager.is_alive(e) { return None; }
        
        if let &mut Some(ref index) = &mut self.map.indices[entity::index_of(e)] {
            match &mut self.map.data[*index] {
                &mut Entry::Full(ref mut full) => Some(&mut full.data),
                // if we're storing an index for an entity, the entry is still guaranteed to be full.
                _ => unreachable!(),
            }
        } else {
            None
        }
    }
    
    /// Removes the data stored for `e`. Returns the old data
    /// if it existed.
    pub fn remove(&mut self, e: Entity) -> Option<T> {
        if !self.entity_manager.is_alive(e) { return None; }
        let entity_index = entity::index_of(e);
        if let Some(index) = self.map.indices[entity_index].clone() {
            self.map.freelist_push(index);
            self.map.data[index].take().map(|entry| entry.data)
        } else {
            None
        }
    }
}

/////////////////////////////////////////////////////
///// Type-level component lists ////////////////////
/////////////////////////////////////////////////////

/// Non-empty case of a list entry.
pub struct ListEntry<T: Any, P> {
	val: T,
	parent: P,
}

/// Empty case of a list entry.
// marker is so outsiders can't create one.
pub struct Empty(PhantomData<()>);

#[doc(hidden)]
pub fn make_empty() -> Empty {
    Empty(PhantomData)
}

/// A list of components where each entry is a zero-sized PhantomData.
/// This is used to signify which components a world will manage, without yet constructing
/// the maps themselves.
pub trait PhantomComponentMaps {
    type Components: ComponentMaps;
	/// Push another component type onto this list. This additionally verifies
	/// that no duplicates are pushed.
	fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Self> where Self: Sized;
	fn has<T: Component>(&self) -> bool;
    fn into_components(self, alloc: WorldAllocator) -> Self::Components;
}

impl PhantomComponentMaps for Empty {
    type Components = Empty;
    
	fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Empty> {
		ListEntry {
			val: PhantomData,
			parent: self,
		}
	}
	
	fn has<T: Component>(&self) -> bool { false }
    fn into_components(self, _: WorldAllocator) -> Empty { make_empty() }
}

impl<C: Component, P: PhantomComponentMaps> PhantomComponentMaps for ListEntry<PhantomData<C>, P> {
    type Components = ListEntry<Mutex<ComponentMap<C>>, P::Components>;
    
	fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Self> {
		if self.has::<T>() { panic!("Added type to components list twice.") }
		
		ListEntry {
			val: PhantomData,
			parent: self,
		}
	}
	
	fn has<T: Component>(&self) -> bool {
		(TypeId::of::<T>() == TypeId::of::<C>()) || self.parent.has::<T>()
	}
    
    fn into_components(self, alloc: WorldAllocator) -> Self::Components {
        ListEntry {
            val: Mutex::new(ComponentMap::new(alloc)),
            parent: self.parent.into_components(alloc),
        }
    }
}

/// A list where each entry is a `ComponentMap` with mutually exclusive
/// access. 
pub trait ComponentMaps {
	/// Get mutually exclusive access to the supplied component's map.
	/// Panics if this list has no entry for `T`.
	fn lock<T: Component>(&self) -> MutexGuard<ComponentMap<T>>;
}

impl ComponentMaps for Empty {
	fn lock<T: Component>(&self) -> MutexGuard<ComponentMap<T>> {
		panic!("No component of given type");
	}
}

impl<C: Component, P: ComponentMaps> ComponentMaps for ListEntry<Mutex<ComponentMap<C>>, P> {
	fn lock<T: Component>(&self) -> MutexGuard<ComponentMap<T>> {
		if TypeId::of::<T>() == TypeId::of::<C>() {
			unsafe { 
				mem::transmute::<
                    MutexGuard<ComponentMap<C>>,
                    MutexGuard<ComponentMap<T>>
                >(self.val.lock().unwrap())
			}
		} else {
			self.parent.lock()
		}
	}
}


#[cfg(test)]
mod tests {    
    use super::{ComponentMap};
}