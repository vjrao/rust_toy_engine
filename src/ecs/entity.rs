use std::collections::VecDeque;

// The number of bits in the id to use for the generation.
const GEN_BITS: usize = 8;
const GEN_MASK: usize = (1 << GEN_BITS) - 1;
// The number of bits in the id to use for the index.
#[cfg(target_pointer_width = "32")]
const INDEX_BITS: usize = (32 - GEN_BITS);
#[cfg(target_pointer_width = "64")]
const INDEX_BITS: usize = (64 - GEN_BITS);
const INDEX_MASK: usize = (1 << INDEX_BITS) - 1;
// The minimum number of unused indices that need to exist before one is used.
const MIN_UNUSED: usize = 1024;

/// An unique entity.
/// Entities each have a unique id which serves
/// as a weak pointer to the entity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Entity {
    id: usize,
}

impl Entity {
    fn new(index: usize, gen: u8) -> Entity {
        Entity {
            id: (gen as usize).wrapping_shl(INDEX_BITS as u32) + index
        }
    }

    fn index(&self) -> usize {
        self.id & INDEX_MASK
    }

    fn generation(&self) -> u8 {
        ((self.id >> INDEX_BITS) & GEN_MASK) as u8
    }
}
/// Manages the creation and destruction of entities.
pub struct EntityManager {
    generation: Vec<u8>,
    unused: VecDeque<usize>,
}

impl EntityManager {
    pub fn new() -> Self {
        EntityManager {
            generation: Vec::new(),
            unused: VecDeque::with_capacity(MIN_UNUSED + 1)
        }
    }

    /// Creates an entity.
    pub fn next_entity(&mut self) -> Entity {
        if self.unused.len() <= MIN_UNUSED {
            self.generation.push(0);
            let idx = self.generation.len() - 1;
            Entity::new(idx, 0)
        } else {
            let idx = self.unused.pop_front().unwrap();
            self.generation[idx] += 1;
            Entity::new(idx, self.generation[idx])
        }
    }

    /// Creates a vector of n entities.
    pub fn next_entities(&mut self, n: usize) -> Vec<Entity> {
        let mut entities = Vec::with_capacity(n);
        for _ in 0..n {
            entities.push(self.next_entity());
        }
        entities
    }

    /// Whether an entity is currently "alive", or exists.
    pub fn is_alive(&self, e: Entity) -> bool {
        self.generation[e.index()] == e.generation()
    }

    /// Destroy an entity.
    pub fn destroy_entity(&mut self, e: Entity) {
        self.generation[e.index()] += 1;
    }
}