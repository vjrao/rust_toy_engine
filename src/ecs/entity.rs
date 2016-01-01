use memory::collections::{VecDeque, Vector};

use std::cell::RefCell;
use std::slice;
use std::sync::RwLock;

use super::world::WorldAllocator;

// The number of bits in the id to use for the generation.
const GEN_BITS: usize = 8;
const GEN_MASK: usize = (1 << GEN_BITS) - 1;
// The number of bits in the id to use for the index.
// use usize::BITS when it becomes stable.
#[cfg(target_pointer_width = "32")]
const INDEX_BITS: usize = (32 - GEN_BITS);
#[cfg(target_pointer_width = "64")]
const INDEX_BITS: usize = (64 - GEN_BITS);

const INDEX_MASK: usize = (1 << INDEX_BITS) - 1;
// The default value for min_unused. 
const DEFAULT_MIN_UNUSED: usize = 1024;

// The amount of entities to iterate over before checking for writers.
const ITERS_BEFORE_RELOCKING: usize = 128;

/// An unique entity.
/// Entities each have a unique id which serves
/// as a weak pointer to the entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Entity {
    id: usize,
}

impl Entity {
    fn new(index: usize, gen: u8) -> Entity {
        // if index > 2^INDEX_BITS, we're in trouble.
        // this is only really a problem on 32-bit systems,
        // where some applications could feasibly have
        // greater than 2^24 entities, however unlikely that is.
        assert!(index < (1 << INDEX_BITS));
        let id = (gen as usize).wrapping_shl(INDEX_BITS as u32) + index;
        Entity {
            id: id
        }
    }
}

pub fn index_of(e: Entity) -> usize {
    e.id & INDEX_MASK
}

pub fn generation_of(e: Entity) -> u8 {
        ((e.id >> INDEX_BITS) & GEN_MASK) as u8
}

type GenVec = Vector<u8, WorldAllocator>;
type Unused = VecDeque<usize, WorldAllocator>;

/// Manages the creation and destruction of entities.
/// This is fully thread-safe.
pub struct EntityManager {
    generation: RwLock<GenVec>,
    unused: RefCell<Unused>,
    min_unused: usize,
}

impl EntityManager {
    /// Create a new entity manager.
    #[inline]
    pub fn new(alloc: WorldAllocator) -> Self {
        EntityManager::with_min_unused(alloc, DEFAULT_MIN_UNUSED)
    }

    /// Creates a new entity manager which forces
    /// there to be `min_unused` dead entities before
    /// any are recycled.
    pub fn with_min_unused(alloc: WorldAllocator, min_unused: usize) -> Self {
        EntityManager {
            generation: RwLock::new(GenVec::with_alloc(alloc)),
            unused: RefCell::new(Unused::with_alloc_and_capacity(alloc, min_unused + 1)),
            min_unused: min_unused,
        }
    }

    /// Creates an entity.
    pub fn next_entity(&self) -> Entity {
        let mut generation = self.generation.write().unwrap();
        let mut unused = self.unused.borrow_mut();
       
        make_entity(&mut *generation, &mut *unused, self.min_unused)
    }

    /// Creates n entities and puts them into the slice given.
    /// If the slice is smaller than n, it only creates enough entities to fill the slice.
    ///
    /// Returns the number of entities created.
    pub fn next_entities(&self, buf: &mut [Entity], n: usize) -> usize {
        let mut generation = self.generation.write().unwrap();
        let mut unused = self.unused.borrow_mut();
        let num = ::std::cmp::min(n, buf.len());
        
        for i in 0..num {
            buf[i] = make_entity(&mut *generation, &mut *unused, self.min_unused);
        }
        
        num
    }

    /// Whether an entity is currently "alive", or exists.
    pub fn is_alive(&self, e: Entity) -> bool {
        self.generation.read().unwrap()[index_of(e)] == generation_of(e)
    }

    /// Destroy an entity.
    pub fn destroy_entity(&self, e: Entity) {
        let mut gen = self.generation.write().unwrap();
        destroy_entity(&mut *gen, &mut *self.unused.borrow_mut(), e);
    }
    
    /// Destroy all the entities in the slice.
    pub fn destroy_entities(&self, entities: &[Entity]) {
        let mut gen = self.generation.write().unwrap();
        let mut unused = self.unused.borrow_mut();
        
        // defer to the helper so we only lock once.
        for e in entities.iter() {
            destroy_entity(&mut *gen, &mut *unused, *e);
        }
    }
    
    /// Get an upper bound on the number of entities which could be live.
    pub fn size_hint(&self) -> usize {
        self.generation.read().unwrap().len()
    }
}

// utility functions for creating/destroying entities
fn make_entity(gen: &mut GenVec, unused: &mut Unused, min_unused: usize) -> Entity {
    if unused.len() <= min_unused {
        gen.push(0);
        let idx = gen.len() - 1;
        Entity::new(idx, 0)
    } else {
        let idx = unused.pop_front().unwrap();
        Entity::new(idx, gen[idx])
    }
}

fn destroy_entity(gen: &mut GenVec, unused: &mut Unused, e: Entity) {
    let idx = index_of(e);
    
    // outdated handles should have no effect on the entity in the
    // same index as it.
    if gen[idx] != generation_of(e) { return }
    
    gen[idx].wrapping_add(1);
    unused.push_back(idx);
}