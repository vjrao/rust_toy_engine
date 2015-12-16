mod component_map;
mod entity;

pub use ecs::entity::{Entity, EntityManager};

mod world {
	use memory::allocator::{self, Address, Allocator, DefaultAllocator, Kind};
	// The main allocator: guaranteed to live as long as the world itself.
	#[derive(Clone, Copy)]
	pub struct MainAllocator(DefaultAllocator);
	
	impl MainAllocator {
		pub fn new() -> Self {
			MainAllocator(DefaultAllocator)
		}
	}
	
	// forwarding impl for allocator
	unsafe impl Allocator for MainAllocator {
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
		unsafe fn usable_size(&self, kind: Kind) -> allocator::Capacity {
			self.0.usable_size(kind)
		}
		
	}
}