use memory::allocator::{self, Address, Allocator, DefaultAllocator, Kind};
use memory::Vector;

use std::marker::PhantomData;
use std::sync::RwLock;

use super::component::{Component, Components, Empty, ListEntry, make_empty, PhantomComponents};

use super::entity::{Entity, EntityManager, MIN_UNUSED};

use super::internal::{Blob, Granularity, Offset, SlotError};

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
    unsafe fn realloc(&mut self,
                      addr: Address,
                      kind: Kind,
                      new_kind: Kind)
                      -> Result<Address, Self::Error> {
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
    dead_entities: Vector<Entity, WorldAllocator>,
    alloc: WorldAllocator,
}

impl<C: Components> State<C> {
    // get an entity from the entity manager, allocate a Small block for it,
    // and create an entry in the master offset table.
    fn next_entity(&mut self) -> Entity {
        // we give each entity a small block by default.
        let mut block = self.blob.next_block(Granularity::Small);
        let e = self.entities.next_entity(block.offset());

        unsafe { block.initialize(e) }
        e
    }

    // if the provided entity is currently alive, mark it dead and free its data block.
    fn destroy_entity(&mut self, e: Entity) {
        if !self.is_alive(e) {
            return;
        }
        
        if let Some(block_offset) = self.entities.offset_of(e) {
            let block = self.blob.get_block(block_offset);
            unsafe { self.blob.free_block(block) }
        }

        unsafe { self.entities.destroy_entity(e) }
        // cache dead entities until their index can possibly be recycled.
        // then have each component offset table clear them all at once,
        // minimizing cache misses for the batch.
        self.dead_entities.push(e);
        if self.dead_entities.len() > MIN_UNUSED {
            self.components.clear_components_for(&self.dead_entities[..]);
            self.dead_entities.clear();
        }
    }

    // destroy all entities in the iterator.
    // this can probably be optimized for cache coherence by doing multiple passes, once
    // for each portion of the destruction routine
    fn destroy_entities<I>(&mut self, entities: I)
        where I: IntoIterator<Item = Entity>
    {
        for e in entities {
            self.destroy_entity(e);
        }
    }

    // whether an entity has a component. this checks whether the entitiy is alive first.
    fn has_component<T: Component>(&self, e: Entity) -> bool {
        self.is_alive(e) && self.components.get::<T>().offset_of(e).is_some()
    }
    
    // whether a (possibly dead) entity has a component.
    // the caller should check that the entity is in fact alive before attempting
    // to use this offset.
    unsafe fn has_component_unchecked<T: Component>(&self, e: Entity) -> bool {
        self.components.get::<T>().offset_of(e).is_some()
    }

    // whether an entity is alive.
    fn is_alive(&self, e: Entity) -> bool {
        self.entities.is_alive(e)
    }

    // get a reference to an entity's component.
    fn get_component<T: Component>(&self, e: Entity) -> Option<&T> {
        use std::mem;

        self.entities.offset_of(e).and_then(|block_offset| {
            // We store offsets only for blocks which we can get handles for.
            let block = self.blob.get_block(block_offset);
            self.components.get::<T>().offset_of(e).map(|comp_offset| unsafe {
                mem::transmute::<*mut T,
                                 &T>(block.data_ptr().offset(comp_offset as isize) as *mut T)
            })
        })
    }

    // get a mutable reference to an entity's component.
    fn get_mut_component<T: Component>(&mut self, e: Entity) -> Option<&mut T> {
        use std::mem;

        self.entities.offset_of(e).and_then(|block_offset| {
            // We store offsets only for blocks which we can get handles for.
            let block = self.blob.get_block(block_offset);
            self.components.get::<T>().offset_of(e).map(|comp_offset| unsafe {
                mem::transmute::<*mut T,
                                 &mut T>(block.data_ptr().offset(comp_offset as isize) as *mut T)
            })
        })
    }

    /// Sets the component for an entity. Returns the old data, if it existed.
    fn set_component<T: Component>(&mut self, e: Entity, component: T) -> Option<T> {
        use std::mem;
        use std::ptr;

        let size = mem::size_of::<T>();

        self.entities.offset_of(e).and_then(|block_offset| {
            let mut block = self.blob.get_block(block_offset);
            if let Some(offset) = self.components.get::<T>().offset_of(e) {
                // already have a slot for this component.
                if size == 0 {
                    // we don't use space for zero-sized components.
                    // just return back the value they gave us...not like they can tell.
                    Some(component)
                } else {
                    let data_ptr = unsafe { block.data_ptr().offset(offset as isize) as *mut T };
                    unsafe { Some(ptr::replace(data_ptr, component)) }
                }
            } else {
                // no slot for this component yet. Try to create a new one.
                // I guess we just panic if we're using too much data?
                if size == 0 {
                    self.components.get_mut::<T>().set(e, 1);
                    None
                } else {
                    unsafe {
                        let offset = match block.next_free(size) {
                            Ok(offset) => offset,

                            Err(SlotError::NeedsPromote(new_gran)) => {
                                block = self.blob.promote_block(block, new_gran);
                                let offset = block.next_free(size).ok().unwrap();
                                offset
                            }

                            _ => panic!("Entity {:?} has too much component data", e),
                        };

                        // write the component data into the slot we found.
                        let data_ptr = block.data_ptr().offset(offset as isize) as *mut T;
                        ptr::write(data_ptr, component);
                        // store the offset in the component's table.
                        self.components.get_mut::<T>().set(e, offset as u16);
                        None // no prior data.
                    }
                }
            }
        })
    }

    /// Removes component data for an entity. Returns the old data, if it existed.
    fn remove_component<T: Component>(&mut self, e: Entity) -> Option<T> {
        use std::mem;
        use std::ptr;

        let size = mem::size_of::<T>();

        self.entities.offset_of(e).and_then(|block_offset| {
            let mut block = self.blob.get_block(block_offset);
            self.components.get_mut::<T>().remove(e).map(|off| unsafe {
                if size == 0 {
                    // zero-sized types are all the same...right?
                    // not allowed to transmute with unsubstituted type para,s 
                    let zero = ();
                    ptr::read(&zero as *const _ as *const T)
                } else {
                    // read the data out of the slot, then mark it free and attempt to merge adjacent slots
                    // to reduce fragmentation.
                    let data_ptr = block.data_ptr().offset(off as isize) as *mut T;
                    let data = ptr::read(data_ptr);
                    block.mark_free(off as usize, size);
                    block.merge_adjacent_slots();
                    data
                }
            })
        })
    }
    
    // Make a vector of all offsets which refer to blocks with entities that are both alive
    // and fulfill the predicate provided. Uses the given allocator to create the vector.
    fn all_which_fulfill<P, A>(&self, mut pred: P, alloc: A) -> Vector<Offset, A>
    where P: FnMut(Entity) -> bool, A: Allocator {
        let mut offsets = Vector::with_alloc(alloc);
        // probably could get better cache coherence by performing each step once
        // and using map_in_place.
        let producer = self.entities.iter()
            .filter(|e| pred(*e))
            .filter_map(|e| self.entities.offset_of(e));
            
        offsets.extend(producer);
        offsets
    }
}

/// The world manages state for entities and components.
/// Typically, you will alter state via `Processors` which run 
/// in groups you provide, but you can also do some specific state altering
/// through the world itself. Any kind of introspection on the world itself
/// requires exclusive access through a mutable reference.
pub struct World<C: Components> {
    state: RwLock<State<C>>,
    pool: WorkPool,
}

impl<C: Components> World<C> {
    // most of the methods from state using RwLock::get_mut() plus a few world-specific ones.

    /// Create a new entity.
    pub fn next_entity(&mut self) -> Entity {
        self.state.get_mut().unwrap().next_entity()
    }

    /// Destroy an entity.
    pub fn destroy_entity(&mut self, e: Entity) {
        self.state.get_mut().unwrap().destroy_entity(e);
    }

    /// Destroy all entities in the provided iterator.
    pub fn destroy_entities<I>(&mut self, entities: I)
        where I: IntoIterator<Item = Entity>
    {
        self.state.get_mut().unwrap().destroy_entities(entities);
    }

    /// Attempt to get a reference to an entity's component data.
    pub fn get_component<T: Component>(&mut self, entity: Entity) -> Option<&T> {
        self.state.get_mut().unwrap().get_component(entity)
    }

    /// Attempt to get a mutable reference to an entity's component data.
    pub fn get_mut_component<T: Component>(&mut self, entity: Entity) -> Option<&mut T> {
        self.state.get_mut().unwrap().get_mut_component(entity)
    }

    /// Whether an entity has a component.
    pub fn has_component<T: Component>(&mut self, entity: Entity) -> bool {
        self.state.get_mut().unwrap().has_component::<T>(entity)
    }

    /// Manually set the component data for an entity.
    /// Returns the old data, if it existed.
    pub fn set_component<T: Component>(&mut self, entity: Entity, component: T) -> Option<T> {
        self.state.get_mut().unwrap().set_component(entity, component)
    }

    /// Manually remove the component data for an entity.
    /// Returns the old data, if it existed.
    pub fn remove_component<T: Component>(&mut self, entity: Entity) -> Option<T> {
        self.state.get_mut().unwrap().remove_component(entity)
    }
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
    use ecs::Component;

    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    struct Pos {
        x: i32,
        y: i32,
    }
    
    impl Component for Pos {}

    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    struct Vel {
        x: i32,
        y: i32,
    }
    
    impl Component for Vel {}

    #[test]
    #[should_panic]
    fn push_same_twice() {
        let _ = WorldBuilder::new().with_component::<Pos>().with_component::<Pos>();
    }

    #[test]
    fn make_world() {
        let _ = WorldBuilder::new().with_component::<Pos>().with_component::<Vel>().build();
    }

    #[test]
    fn make_5k_entities() {
        let mut world = WorldBuilder::new().with_component::<Pos>().build();
        let entities: Vec<_> = (0..(5 * 1024)).map(|_| world.next_entity()).collect();
        for entity in entities.iter().cloned() {
            assert!(!world.has_component::<Pos>(entity));
            assert!(world.set_component(entity, Pos::default()).is_none());
            assert!(world.has_component::<Pos>(entity));
            assert_eq!(*world.get_component::<Pos>(entity).unwrap(),
                       Default::default());
        }
        world.destroy_entities(entities);
    }

    #[test]
    fn remove_attach() {
        // remove and attach a component some amount of times.
        // this is supposed to indicate that the memory released when removing the component
        // is merged with other free memory, and then used again when reattaching to prevent
        // unbounded growth.
        const ATTACH_TIMES: usize = 10_000;

        let mut world = WorldBuilder::new().with_component::<Pos>().build();
        let e = world.next_entity();
        assert!(world.set_component(e, Pos::default()).is_none());
        for _ in 0..ATTACH_TIMES {
            assert!(world.has_component::<Pos>(e));
            assert_eq!(world.remove_component::<Pos>(e).unwrap(), Pos::default());
            assert!(!world.has_component::<Pos>(e));
            assert!(world.set_component(e, Pos::default()).is_none());
        }

        world.destroy_entity(e);
    }
}
