use memory::allocator::{self, Address, Allocator, DefaultAllocator, Kind};

use super::component::{Component, ComponentMap, ComponentMaps};

/// The world's allocator: for long term storage.
///
/// For now, this just forwards to the heap.
#[cfg(not(test))] #[derive(Clone, Copy)]
pub struct WorldAllocator(DefaultAllocator);

#[cfg(test)]
pub type WorldAllocator = DefaultAllocator;

#[cfg(not(test))]
unsafe impl Allocator for WorldAllocator {
	type Error = <DefaultAllocator as Allocator>::Error;
	
	#[inline]
	unsafe fn alloc(&mut self, kind: Kind) -> Result<Address, Self::Error> {
		self.0.alloc(kind)
	}
	
	#[inline]
	unsafe fn dealloc(&mut self, addr: Address, kind: Kind) -> Result<(), Self::Error> {
		self.0.dealloc(addr, kind)
	}
	
	#[inline]
	unsafe fn realloc(&mut self, addr: Address, kind: Kind, new_kind: Kind) -> Result<Address, Self::Error> {
		self.0.realloc(addr, kind, new_kind)
	}
	
	#[inline]
	unsafe fn usable_size(&self, kind: Kind) -> (allocator::Capacity, allocator::Capacity) {
		self.0.usable_size(kind)
	}
}

#[derive(Clone)]
pub struct World<C: ComponentMaps> {
	components: C,
}