//! Disgusting type system abuse culminating in various types of mixed-type component lists.
//! These aren't really to be used externally, but instead provide some level of convenience
//! and abstraction over the components which worlds operate over.
//!
//! These lists will most likely have O(1) access to members, at least when compiled with optimizations.
//! If `TypeId::of` ever becomes a const fn, they definitely will be.

use std::any::{Any, TypeId};
use std::marker::PhantomData;
use std::mem;
use std::sync::{Mutex, MutexGuard};

use super::component_map::ComponentMap;
use super::world::MainAllocator;

trait Component: Any {}

/// Non-empty case of a list entry.
pub struct Entry<T: Any, P> {
	val: T,
	parent: P,
}

/// Empty case of a list entry.
pub struct Empty;

/// A list of components where each entry is a zero-sized PhantomData.
/// This is used to signify which components a world will manage, without yet constructing
/// the maps themselves.
pub trait PhantomComponents {
	/// Push another component type onto this list. This additionally verifies
	/// that no duplicates are pushed.
	fn push<T: Component>(self) -> Entry<PhantomData<T>, Self> where Self: Sized;
	fn has<T: Component>(&self) -> bool;
}

impl PhantomComponents for Empty {
	fn push<T: Component>(self) -> Entry<PhantomData<T>, Empty> {
		Entry {
			val: PhantomData,
			parent: self,
		}
	}
	
	fn has<T: Component>(&self) -> bool { false }
}

impl<C: Component, P: PhantomComponents> PhantomComponents for Entry<PhantomData<C>, P> {
	fn push<T: Component>(self) -> Entry<PhantomData<T>, Self> {
		if self.has::<T>() { panic!("Added type to components list twice.") }
		
		Entry {
			val: PhantomData,
			parent: self,
		}
	}
	
	fn has<T: Component>(&self) -> bool {
		(TypeId::of::<T>() == TypeId::of::<C>()) || self.parent.has::<T>()
	}
}

/// A list where each entry is a `ComponentMap` 
pub trait Components {
	/// Get a reference to the supplied component's map.
	/// Panics if this list has no entry for `T`.
	fn get<T: Component>(&self) -> &ComponentMap<T>;
	
	/// Get a mutable reference to the supplied component's map.
	/// Panics if this list has no entry for `T`.
	fn get_mut<T: Component>(&mut self) -> &mut ComponentMap<T>;
}

impl Components for Empty {
	fn get<T: Component>(&self) -> &ComponentMap<T> {
		panic!("No component of given type");
	}
	
	fn get_mut<T: Component>(&mut self) -> &mut ComponentMap<T> {
		panic!("No component of given type");
	}
}

impl<C: Component, P: Components> Components for Entry<ComponentMap<C>, P> {
	fn get<T: Component>(&self) -> &ComponentMap<T> {
		if TypeId::of::<T>() == TypeId::of::<C>() {
			unsafe { 
				mem::transmute::<&ComponentMap<C>, &ComponentMap<T>>(&self.val)
			}
		} else {
			self.parent.get()
		}
	}
	
	fn get_mut<T: Component>(&mut self) -> &mut ComponentMap<T> {
		if TypeId::of::<T>() == TypeId::of::<C>() {
			unsafe { 
				mem::transmute::<&mut ComponentMap<C>, &mut ComponentMap<T>>(&mut self.val)
			}
		} else {
			self.parent.get_mut()
		}
	}
}

/// A list where every entry is wrapped in a mutex.
pub trait MutexComponents {
	/// Lock the mutex for `T`.
	/// Panics if this list has no entry for T.
	fn lock<T: Component>(&self) -> MutexGuard<ComponentMap<T>>;
}

impl MutexComponents for Empty {
	fn lock<T: Component>(&self) -> MutexGuard<ComponentMap<T>> {
		panic!("No component of given type");
	}
}

impl<C: Component, P: MutexComponents> MutexComponents for Entry<Mutex<ComponentMap<C>>, P> {
	fn lock<T: Component>(&self) -> MutexGuard<ComponentMap<T>> {
		if TypeId::of::<T>() == TypeId::of::<C>() {
			let guard = match self.val.lock() {
				Ok(g) => g,
				Err(p) => p.into_inner(),
			};
			
			unsafe {
				mem::transmute(guard)
			}
		} else {
			self.parent.lock()
		}
	}
}

/// Something that can be transformed into a type which implements `Components`.
pub trait IntoComponents {
	type Output: Components;
	
	fn into_components(self) -> Self::Output;
}

impl IntoComponents for Empty {
	type Output = Empty;
	
	fn into_components(self) -> Self::Output { Empty }
}

impl<C: Component, P: IntoComponents> IntoComponents for Entry<Mutex<ComponentMap<C>>, P> {
	type Output = Entry<ComponentMap<C>, <P as IntoComponents>::Output>;
	
	fn into_components(self) -> Self::Output { 
		let map = match self.val.into_inner() {
			Ok(m) => m,
			Err(p) => p.into_inner(),
		};
		
		Entry {
			val: map,
			parent: self.parent.into_components(),
		}
	}
}

impl<C: Component, P> IntoComponents for (Entry<PhantomData<C>, P>, MainAllocator) 
where (P, MainAllocator): IntoComponents {
	type Output = Entry<ComponentMap<C>, <(P, MainAllocator) as IntoComponents>::Output>;
	
	fn into_components(self) -> Self::Output {
		Entry {
			val: ComponentMap::new(self.1),
			parent: (self.0.parent, self.1).into_components(),
		}
	}
}

/// Something which can be transformed into a type which implements `MutexComponents`.
pub trait IntoMutexComponents {
	type Output: MutexComponents;
	
	fn into_mutex_components(self) -> Self::Output;
}

impl IntoMutexComponents for Empty {
	type Output = Empty;
	
	fn into_mutex_components(self) -> Self::Output { Empty }
}

impl<T: Component, P: IntoMutexComponents> IntoMutexComponents for Entry<ComponentMap<T>, P> {
	type Output = Entry<Mutex<ComponentMap<T>>, <P as IntoMutexComponents>::Output>;
	
	fn into_mutex_components(self) -> Self::Output {
		Entry {
			val: Mutex::new(self.val),
			parent: self.parent.into_mutex_components(),
		}
	}
}

/// A type that can be roundtripped into a MutexComponents and back again.
pub trait RoundTrip: IntoMutexComponents {}

impl<T: Components + IntoMutexComponents> RoundTrip for T where
<T as IntoMutexComponents>::Output: IntoComponents<Output=T> {}