use memory::collections::{VecDeque, Vector};


use std::slice;

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
        // TODO: add error handling.
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

/// Manages the creation and destruction of entities.
pub struct EntityManager {
    generation: Vector<u8, WorldAllocator>,
    unused: VecDeque<usize, WorldAllocator>,
    min_unused: usize,
}

impl EntityManager {
    /// Create a new entity manager.
    pub fn new(alloc: WorldAllocator) -> Self {
        EntityManager::with_min_unused(alloc, DEFAULT_MIN_UNUSED)
    }

    /// Creates a new entity manager which forces
    /// there to be `min_unused` dead entities before
    /// any are recycled.
    pub fn with_min_unused(alloc: WorldAllocator, min_unused: usize) -> Self {
        EntityManager {
            generation: Vector::with_alloc(alloc),
            unused: VecDeque::with_alloc_and_capacity(alloc, min_unused + 1),
            min_unused: min_unused,
        }
    }

    /// Creates an entity.
    pub fn next_entity(&mut self) -> Entity {
        if self.unused.len() <= self.min_unused {
            self.generation.push(0);
            let idx = self.generation.len() - 1;
            Entity::new(idx, 0)
        } else {
            let idx = self.unused.pop_front().unwrap();
            Entity::new(idx, self.generation[idx])
        }
    }

    /// Creates n entities and puts them into the slice given.
    /// If the slice is smaller than n, it only creates enough entities to fill the slice.
    ///
    /// Returns the number of entities created.
    pub fn next_entities(&mut self, buf: &mut [Entity], n: usize) -> usize {
        let num = ::std::cmp::min(n, buf.len());
        for i in 0..num {
            buf[i] = self.next_entity();
        }
        num
    }

    /// Whether an entity is currently "alive", or exists.
    pub fn is_alive(&self, e: Entity) -> bool {
        self.generation[index_of(e)] == generation_of(e)
    }

    /// Destroy an entity.
    pub fn destroy_entity(&mut self, e: Entity) {
        let idx = index_of(e);
        self.generation[idx] += 1;
        self.unused.push_back(idx);
    }

    /// Iterate over all living entities
    pub fn entities(&self) -> Entities {
        Entities {
            index: 0,
            generation: self.generation.iter(),
        }
    }
    
    /// Get an upper bound on the number of entities which could be live.
    pub fn size_hint(&self) -> usize {
        self.generation.len()
    }
}

/// An iterator over entities.
pub struct Entities<'a> {
    index: usize,
    generation: slice::Iter<'a, u8>,
}

impl<'a> Iterator for Entities<'a> {
    type Item = Entity;

    fn next(&mut self) -> Option<Entity> {
        self.generation.next().map(|gen| {
            let e = Entity::new(self.index, *gen);
            self.index += 1;
            e
        })
    }
}