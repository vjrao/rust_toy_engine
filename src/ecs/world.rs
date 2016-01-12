use memory::allocator::{self, Address, Allocator, DefaultAllocator, Kind};
use memory::Vector;

use std::marker::PhantomData;
use std::sync::RwLock;

use super::component::{
    Component,
    Components,
    Empty,
    ListEntry,
    make_empty,
    PhantomComponents,
};

use super::entity::{Entity, EntityManager, MIN_UNUSED};

use super::internal::{
    Blob,
    BlockHandle,
    ComponentOffsetTable,
    MasterOffsetTable,
};

use jobsteal::WorkPool;

/// The world's allocator: for long term storage.
///
/// For now, this just forwards to the heap.
#[cfg(not(test))]
#[derive(Clone, Copy)]
pub struct WorldAllocator(DefaultAllocator);

#[cfg(test)]
#[derive(Clone, Copy)]
pub struct WorldAllocator(pub DefaultAllocator);

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
    blob: Blob,
    offsets: MasterOffsetTable,
    dead_entities: Vector<Entity, WorldAllocator>,
    alloc: WorldAllocator,
}

impl<C: Components> State<C> {
    fn next_entity(&mut self) -> Entity {
        self.entities.next_entity()
    }
    
    fn destroy_entity(&mut self, e: Entity) {
        if !self.is_alive(e) { return }
        
        unsafe { self.entities.destroy_entity(e) }
        self.dead_entities.push(e);
        self.offsets.remove(e);
        
        
        // cache dead entities until their index can possibly be recycled.
        // then have each component offset table clear them all at once,
        // minimizing cache misses for the batch.
        if self.dead_entities.len() > MIN_UNUSED {
            self.components.clear_components_for(&self.dead_entities[..]);
            self.dead_entities.clear(); // reset the vector.
        }
    }
    
    fn destroy_entities<I>(&mut self, entities: I) where I: IntoIterator<Item=Entity> {
        for e in entities {
            self.destroy_entity(e);
        }
    }
    
    fn has_component<T: Component>(&self, e: Entity) -> bool {
        self.is_alive(e) && 
        self.components.get::<T>().offset_of(e).is_some()
    }
    
    fn is_alive(&self, e: Entity) -> bool {
        self.entities.is_alive(e)
    }
    
    fn get_component<T: Component>(&self, e: Entity) -> Option<&T> {
        use std::mem;
        
        self.offsets.offset_of(e).and_then(|(gran, index)| {
            // We store offsets only for blocks which we can get handles for.
            let block = self.blob.get_block(gran, index).expect("Could not get handle for stored block.");
            self.components.get::<T>().offset_of(e).map(|comp_offset| unsafe {
                mem::transmute::<*mut T, &T>(
                    block.data_ptr().offset(comp_offset as isize) as *mut T)
            })
        })
    }
    
    fn get_mut_component<T: Component>(&mut self, e: Entity) -> Option<&mut T> {
        use std::mem;
        
        self.offsets.offset_of(e).and_then(|(gran, index)| {
            // We store offsets only for blocks which we can get handles for.
            let block = self.blob.get_block(gran, index).expect("Could not get handle for stored block.");
            self.components.get::<T>().offset_of(e).map(|comp_offset| unsafe {
                mem::transmute::<*mut T, &mut T>(
                    block.data_ptr().offset(comp_offset as isize) as *mut T)
            })
        })
    }
}

pub struct World<C: Components> {
	state: RwLock<State<C>>,
    pool: WorkPool,
}

impl<C: Components> World<C> {
    // most of the methods from state using RwLock::get_mut() plus a few world-specific ones.
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
            blob: Blob::new(alloc),
            offsets: MasterOffsetTable::new(alloc),
            dead_entities: Vector::with_alloc_and_capacity(alloc, MIN_UNUSED),
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