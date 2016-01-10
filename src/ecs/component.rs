//! This module contains component-related types and traits.
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

use super::entity::{self, Entity, EntityManager, index_of};
use super::world::WorldAllocator;
use super::{COMPONENT_ALIGN, LARGE_SIZE};

// Since space is crucial to these structures, we will use
// the two most significant bits of the index to signify whether
// there is an entry for this component. We only allow entities
// to have 8KB of component data, so 2^14 bits will easily suffice.
// entity index is unused.
const MAX_OFFSET: u16 = (1 << 15) - 1;
const CLEARED: u16 = ::std::u16::MAX;

/// Components are arbitrary data associated with entities.
pub trait Component: Any + Send + Sync {}
impl<T: Any + Send + Sync> Component for T {}

// any checks for component types which cannot be done at compile-time
// will be done here.
fn assert_component_compliance<T: Component>() {
    use std::mem;
    
    assert!(mem::align_of::<T>() <= COMPONENT_ALIGN, 
        "Component alignment too large. Maximum supported is {} bytes.", COMPONENT_ALIGN);
        
    assert!(mem::size_of::<T>() <= LARGE_SIZE, 
        "Component size too large. Maximum supported is {} bytes.", LARGE_SIZE);
}

/// Maps entities to the offset of this component's data
/// in their entry in the master table.
pub struct ComponentOffsetTable<T: Component> {
    offsets: Vector<u16, WorldAllocator>,
    _marker: PhantomData<T>,
}

impl<T: Component> ComponentOffsetTable<T> {    
    pub fn new(alloc: WorldAllocator) -> Self {
        ComponentOffsetTable {
            offsets: Vector::with_alloc(alloc),
            _marker: PhantomData,
        }
    }
    
    /// Find the offset of the component this manages offsets for relative to the
    /// data of the entity this manages.
    pub fn offset_of(&self, entity: Entity) -> Option<u16> {
        // if the vector isn't long enough yet, that's a sure sign that
        // we haven't got an index for this entity.
        self.offsets.get(index_of(entity) as usize).and_then(|offset| {
            if *offset == CLEARED {
                None
            } else {
                Some(*offset)
            }
        })
    }
    
    /// Store the given offset for the entity given.
    pub fn set(&mut self, entity: Entity, offset: u16) {
        let idx = index_of(entity) as usize;
        // there shouldn't be any offsets that use this many bits.
        debug_assert!(idx <= MAX_OFFSET as usize);
        
        self.ensure_capacity(idx);
        self.offsets[idx] = offset;
    }
    
    /// Remove the offset for the entity given.
    pub fn remove(&mut self, entity: Entity) {
        let idx = index_of(entity) as usize;
        
        // don't bother extending the capacity if we don't even have
        // entries that far out.
        if let Some(off) = self.offsets.get_mut(idx) {
            *off = CLEARED;
        }
    }
    
    fn ensure_capacity(&mut self, size: usize) {
        let len = self.offsets.len();
        if len >= size { return }
        
        self.offsets.reserve(size - len);
        while self.offsets.len() < size {
            self.offsets.push(CLEARED);
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
pub struct Empty(PhantomData<()>);

#[doc(hidden)]
pub fn make_empty() -> Empty {
    Empty(PhantomData)
}

/// A list of components where each entry is a zero-sized PhantomData.
/// This is used to signify which components a world will manage, without yet constructing
/// the offset tables themselves.
pub trait PhantomComponents {
    type Components: Components;
	/// Push another component type onto this list. This additionally verifies
	/// that no duplicates are pushed.
	fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Self> where Self: Sized;
	fn has<T: Component>(&self) -> bool;
    fn into_components(self, alloc: WorldAllocator) -> Self::Components;
}

impl PhantomComponents for Empty {
    type Components = Empty;
    
	fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Empty> {
        assert_component_compliance::<T>();
		ListEntry {
			val: PhantomData,
			parent: self,
		}
	}
	
	fn has<T: Component>(&self) -> bool { false }
    fn into_components(self, _: WorldAllocator) -> Empty { make_empty() }
}

impl<C: Component, P: PhantomComponents> PhantomComponents for ListEntry<PhantomData<C>, P> {
    type Components = ListEntry<ComponentOffsetTable<C>, P::Components>;
    
	fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Self> {
		if self.has::<T>() { panic!("Added type to components list twice.") }
        assert_component_compliance::<T>();
		
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
            val: ComponentOffsetTable::new(alloc),
            parent: self.parent.into_components(alloc),
        }
    }
}

/// A list where each entry is an offset table for that component.
pub trait Components {
	/// Get a reference to the offset table for the given component type.
    /// This panics if the component type is not present in this list.
	fn get<T: Component>(&self) -> &ComponentOffsetTable<T>;
    
    /// Get a mutable reference to the offset table for the given component type.
    /// This panics if the component type is not present in this list.
	fn get_mut<T: Component>(&mut self) -> &mut ComponentOffsetTable<T>;
}

impl Components for Empty {
	fn get<T: Component>(&self) -> &ComponentOffsetTable<T> {
        panic!("No such component in tables list.");
    }
    
    fn get_mut<T: Component>(&mut self) -> &mut ComponentOffsetTable<T> {
        panic!("No such component in tables list.");
    }
}

impl<C: Component, P: Components> Components for ListEntry<ComponentOffsetTable<C>, P> {
	fn get<T: Component>(&self) -> &ComponentOffsetTable<T> {
        // TypeIds should have resolved to concrete values by the optimization phase.
        // My guess is that every monomorphization of this function will be inlined into 
        // a large if-else tree, which will then be optimized down to a single return or panic.
		if TypeId::of::<T>() == TypeId::of::<C>() {
			unsafe { 
				mem::transmute(&self.val)
			}
		} else {
			self.parent.get()
		}
	}
    
    fn get_mut<T: Component>(&mut self) -> &mut ComponentOffsetTable<T> {
		if TypeId::of::<T>() == TypeId::of::<C>() {
			unsafe { 
				mem::transmute(&mut self.val)
			}
		} else {
			self.parent.get_mut()
		}
	}
}


#[cfg(test)]
mod tests {    
    use super::*;
    
    struct A;
    struct B;
    struct C;
    struct D;
    
    #[test]
    #[should_panic]
    fn push_twice() {
        let _ = make_empty().push::<A>().push::<A>();
    }
}