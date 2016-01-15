use memory::collections::{VecDeque, Vector};

use std::fmt;

use super::world::WorldAllocator;
use super::internal::{Granularity, Offset};

// The number of bits in the id to use for the generation.
const GEN_BITS: u32 = 8;
const GEN_MASK: u32 = (1 << GEN_BITS) - 1;

// it's enough to use 32 bit integers for all entities.
// this is only really a problem on 32-bit systems,
// where some applications could feasibly have
// greater than 2^24 entities, however unlikely that is.
const INDEX_BITS: u32 = (32 - GEN_BITS);
const INDEX_MASK: u32 = (1 << INDEX_BITS) - 1;

// The minimum amount of entities which must be marked as dead before
// beginning to recycle indices. Coupled with the bits which mark the generation
// of an entity, a user must spawn at least MIN_UNUSED * 2^GEN_BITS entities
// before an exact entity will resurface.
// for the original values of 1024 and 8, an entity will not be reused until
// at least 256K entities have been spawned.
pub const MIN_UNUSED: usize = 1024;

/// An unique entity.
/// Entities each have a unique id which serves
/// as a weak pointer to the entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
#[repr(C)]
pub struct Entity {
    id: u32,
}

impl Entity {
    fn new(index: u32, gen: u8) -> Entity {
        // if index > 2^INDEX_BITS, we're in trouble.
        assert!(index < (1 << INDEX_BITS));
        let id = (gen as u32).wrapping_shl(INDEX_BITS as u32) + index;
        Entity { id: id }
    }
}

impl fmt::Display for Entity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self.id)
    }
}

pub fn index_of(e: Entity) -> u32 {
    e.id & INDEX_MASK
}

pub fn generation_of(e: Entity) -> u8 {
    ((e.id >> INDEX_BITS) & GEN_MASK) as u8
}

// data associated with an entity slot.
// we don't directly embed an Offset here
// because it wastes 8 bytes by not reusing the
// Granularity's padding.
// We should be able to pack all this stuff here
// into 8/16 bytes.
struct EntityData {
    alive: bool,
    gen: u8,
    granularity: Granularity,
    inner_off: usize,
}

/// Manages the creation and destruction of entities.
pub struct EntityManager {
    data: Vector<EntityData, WorldAllocator>,
    unused: VecDeque<usize, WorldAllocator>,
}

impl EntityManager {
    /// Create a new entity manager.
    pub fn new(alloc: WorldAllocator) -> Self {
        EntityManager {
            data: Vector::with_alloc(alloc),
            unused: VecDeque::with_alloc_and_capacity(alloc, MIN_UNUSED + 1),
        }
    }

    /// Creates an entity, provided with the offset of its block.
    pub fn next_entity(&mut self, offset: Offset) -> Entity {
        let (gran, off) = offset.into_parts();
        if self.unused.len() <= MIN_UNUSED {
            self.data.push(EntityData {
                alive: true,
                gen: 0,
                granularity: gran,
                inner_off: off
            });
            let idx = self.data.len() - 1;
            Entity::new(idx as u32, 0)
        } else {
            let idx = self.unused.pop_front().unwrap();
            let datum = &mut self.data[idx as usize];
            datum.alive = true;
            datum.granularity = gran;
            datum.inner_off = off;
            Entity::new(idx as u32, datum.gen)
        }
    }

    /// Whether an entity is currently "alive", or exists.
    #[inline]
    pub fn is_alive(&self, e: Entity) -> bool {
        self.data
            .get(index_of(e) as usize)
            .and_then(|datum| {
                if datum.alive {
                    Some(())
                } else {
                    None
                }
            }).is_some()
    }
    
    pub fn offset_of(&self, e: Entity) -> Option<Offset> {
        if self.is_alive(e) {
            let datum = &self.data[index_of(e) as usize];
            Some(Offset::new(datum.granularity, datum.inner_off))
        } else {
            None
        }
    }

    /// Destroy an entity. The entity must be alive when this 
    /// is called. Destroying an entity multiple times can lead
    /// to memory unsafety.
    pub unsafe fn destroy_entity(&mut self, e: Entity) {
        let idx = index_of(e) as usize;
        self.data[idx].gen.wrapping_add(1);
        self.data[idx].alive = false;
        self.unused.push_back(idx);
    }

    /// Get an upper bound on the number of entities which could be live.
    pub fn size_hint(&self) -> usize {
        self.data.len()
    }
    
    /// Get an iterator over all living entities.
    pub fn iter(&self) -> Entities {
        Entities {
            slice: &self.data,
            cur_idx: 0,
        }
    }
}

/// An iterator over all living entities.
pub struct Entities<'a> {
    slice: &'a [EntityData],
    cur_idx: usize,
}

impl<'a> Iterator for Entities<'a> {
    type Item = Entity;
    
    fn next(&mut self) -> Option<Entity> {
        while let Some(data) = self.slice.get(self.cur_idx) {
            if data.alive {
                return Some(Entity::new(self.cur_idx as u32, data.gen));
            } else {
                self.cur_idx += 1;
            }
        }
        
        None
    }
}