use memory::collections::{VecDeque, Vector};

use super::world::WorldAllocator;

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
        Entity {
            id: id
        }
    }
}

pub fn index_of(e: Entity) -> u32 {
    e.id & INDEX_MASK
}

pub fn generation_of(e: Entity) -> u8 {
        ((e.id >> INDEX_BITS) & GEN_MASK) as u8
}

type GenVec = Vector<u8, WorldAllocator>;
type Unused = VecDeque<u32, WorldAllocator>;

/// Manages the creation and destruction of entities.
/// This is fully thread-safe.
pub struct EntityManager {
    generation: GenVec,
    unused: Unused,
}

impl EntityManager {
    /// Create a new entity manager.
    pub fn new(alloc: WorldAllocator) -> Self {
        EntityManager {
            generation: GenVec::with_alloc(alloc),
            unused: Unused::with_alloc_and_capacity(alloc, MIN_UNUSED + 1),
        }
    }
    
    /// Creates an entity.
    pub fn next_entity(&mut self) -> Entity {
        if self.unused.len() <= MIN_UNUSED {
            self.generation.push(0);
            let idx = self.generation.len() - 1;
            Entity::new(idx as u32, 0)
        } else {
            let idx = self.unused.pop_front().unwrap();
            Entity::new(idx, self.generation[idx as usize])
        }
    }

    /// Whether an entity is currently "alive", or exists.
    #[inline]
    pub fn is_alive(&self, e: Entity) -> bool {
        self.generation.get(index_of(e) as usize).and_then(|gen| {
            if *gen == generation_of(e) { Some(()) }
            else { None }
        }).is_some()
    }

    /// Destroy an entity.
    pub fn destroy_entity(&mut self, e: Entity) {
        if !self.is_alive(e) { return }
        
        let idx = index_of(e);
        self.generation[idx as usize].wrapping_add(1);
        self.unused.push_back(idx);
    }
    
    /// Get an upper bound on the number of entities which could be live.
    pub fn size_hint(&self) -> usize {
        self.generation.len()
    }
}