use memory::allocator::{self, Address, Allocator, DefaultAllocator, Kind};

use std::marker::PhantomData;
use std::sync::RwLock;

use super::component::{
    Component,
    Components,
    ComponentOffsetTable,
    Empty,
    ListEntry,
    make_empty,
    PhantomComponents,
};

use super::entity::EntityManager;

use util::work_pool::WorkPool;

/// The world's allocator: for long term storage.
///
/// For now, this just forwards to the heap.
#[derive(Clone, Copy)]
pub struct WorldAllocator(DefaultAllocator);

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

struct State<C: Components> {
    components: C,
    entities: EntityManager,
    alloc: WorldAllocator,
}

pub struct World<C: Components> {
	state: RwLock<State<C>>,
    pool: WorkPool,
}

/// Used to build a world with the given components.
pub struct WorldBuilder<T: PhantomComponents> {
    phantoms: T,
    /// The number of threads to create the thread pool with.
    pub num_threads: usize,
}

impl WorldBuilder<Empty> {
    // Creates a new world builder with no components and one thread for every logical CPU.
    pub fn new() -> Self {
        WorldBuilder {
            phantoms: make_empty(),
            // may want to get physical cores.
            num_threads: ::num_cpus::get(),
        }
    }
}

impl<T: PhantomComponents> WorldBuilder<T> {
    /// Add a component to the world.
    /// This will panic if the same component is added more than once.
    pub fn with_component<C: Component>(self) -> WorldBuilder<ListEntry<PhantomData<C>, T>> {
        WorldBuilder {
            phantoms: self.phantoms.push::<C>(),
            num_threads: self.num_threads,
        }
    }
    
    /// Set the number of threads to create the internal thread pool with.
    pub fn num_threads(mut self, threads: usize) -> Self {
        self.num_threads = threads;
        self
    }
    
    /// Consume this builder and create a world.
    /// Panics if it fails to create the thread pool.
    pub fn build(self) -> World<T::Components> {
        let alloc = WorldAllocator(DefaultAllocator);
        let components = self.phantoms.into_components(alloc);
        let state = State {
            components: components,
            entities: EntityManager::new(alloc),
            alloc: alloc,
        };
        
        World {
            state: RwLock::new(state),
            pool: WorkPool::new(self.num_threads).unwrap(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{World, WorldBuilder};
    
    struct Pos {
        x: i32,
        y: i32,
    }
    
    struct Vel {
        x: i32,
        y: i32,
    }
    
    #[test]
    #[should_panic]
    fn push_same_twice() {
        let _ = WorldBuilder::new().with_component::<Pos>().with_component::<Pos>();
    }
    
    #[test]
    fn make_world() {
        let _ = WorldBuilder::new().with_component::<Pos>().with_component::<Vel>().build();
    }
}