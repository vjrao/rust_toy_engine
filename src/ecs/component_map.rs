use memory::Vector;

use super::{entity, Entity, EntityManager};
use super::world::MainAllocator;

/// Maps entities to component data.
pub struct ComponentMap<T> {
    indices: Vector<Option<usize>, MainAllocator>,
    data: Vector<Entry<T>, MainAllocator>,
    
    // fifo deque / linked-list of free 
    // indices.
    first_free: Option<usize>,
    last_free: Option<usize>,
}

enum Entry<T> {
    Full(FullEntry<T>),
    Empty(Option<usize>),
}

impl<T> Entry<T> {
    // Attempts to take the data out of this entry.
    // Replaces this entry with an empty one with no next index.
    fn take(&mut self) -> Option<FullEntry<T>> {
        use std::mem;
        
        let entry = Entry::Empty(None);
        
        match mem::replace(self, entry) {
            Entry::Full(full) => Some(full),
            Entry::Empty(next) => {
                *self = Entry::Empty(next);
                None
            }
        }
    }
}

struct FullEntry<T> {
    data: T,
}

impl<T> ComponentMap<T> {
    // Pop the first value from the free list.
    // None if there isn't one.
    fn freelist_pop(&mut self) -> Option<usize> {
        let next = self.first_free.take();
        if let &Some(ref next_index) = &next {
            match &self.data[*next_index] {
                &Entry::Empty(ref next) => {
                    self.first_free = next.clone();
                }
                _ => unreachable!(),
            }
        } else {
            self.last_free = None;
        }
        
        next
    }
    
    // Pushes a free index onto the free list.
    // Does not alter the data entry for that index.
    fn freelist_push(&mut self, index: usize) {
        let last_free = self.last_free.take();
        if let &Some(ref last_index) = &last_free {
            // pushing onto non-empty list
            self.data[*last_index] = Entry::Empty(Some(index));
        } else {
            // pushing onto empty list.
            self.first_free = Some(index);
        }
        
        self.last_free = Some(index);
    }
    
    pub fn new(alloc: MainAllocator) -> Self {
        ComponentMap {
            indices: Vector::with_alloc(alloc),
            data: Vector::with_alloc(alloc),
            first_free: None,
            last_free: None,
        }
    }
    
    /// Supply this ComponentMap with an EntityManager to get a usable handle.
    pub fn supply<'a>(&'a mut self, entity_manager: &'a EntityManager) -> MapHandle<'a ,T> {
        // ensure capacity for indices so unchecked indexing always works.
        let len = entity_manager.size_hint();
        self.indices.reserve(len);
        while self.indices.len() < len {
            self.indices.push(None);
        }
        
        MapHandle {
            map: self,
            entity_manager: entity_manager,
        }
    }
}

pub struct MapHandle<'a, T: 'a> {
    map: &'a mut ComponentMap<T>,
    entity_manager: &'a EntityManager, 
}

impl<'a, T: 'a> MapHandle<'a, T> {
    /// Set the data stored for `e` to the value stored in `data`.
    pub fn set(&mut self, e: Entity, data: T) {
        if !self.entity_manager.is_alive(e) { return; }
        
        let entity_index = entity::index_of(e);
        let entry = Entry::Full(FullEntry {
            data: data,
        });
        
        if let Some(index) = self.map.indices[entity_index].clone() {
            self.map.data[index] = entry;
        } else if let Some(index) = self.map.freelist_pop() {
            self.map.data[index] = entry;
            self.map.indices[entity_index] = Some(index);
        } else {
            self.map.data.push(entry);
            self.map.indices[entity_index] = Some(self.map.data.len() - 1);
        }
    }
    
    /// Try to get a reference to the data stored for `e`.
    /// Returns None if there isn't any.
    pub fn get(&self, e: Entity) -> Option<&T> {
        if !self.entity_manager.is_alive(e) { return None; }
        
        if let &Some(ref index) = &self.map.indices[entity::index_of(e)] {
            match &self.map.data[*index] {
                &Entry::Full(ref full) => Some(&full.data),
                // if we're storing an index for an entity, the entry is guaranteed to be full.
                _ => unreachable!(), 
            }
        } else {
            None
        }
    }
    
    /// Try to get a mutable reference to the data stored for `e`.
    /// Returns None if there isn't any.
    pub fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        if !self.entity_manager.is_alive(e) { return None; }
        
        if let &mut Some(ref index) = &mut self.map.indices[entity::index_of(e)] {
            match &mut self.map.data[*index] {
                &mut Entry::Full(ref mut full) => Some(&mut full.data),
                // if we're storing an index for an entity, the entry is still guaranteed to be full.
                _ => unreachable!(),
            }
        } else {
            None
        }
    }
    
    /// Removes the data stored for `e`. Returns the old data
    /// if it existed.
    pub fn remove(&mut self, e: Entity) -> Option<T> {
        if !self.entity_manager.is_alive(e) { return None; }
        let entity_index = entity::index_of(e);
        if let Some(index) = self.map.indices[entity_index].clone() {
            self.map.freelist_push(index);
            self.map.data[index].take().map(|entry| entry.data)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use ecs::EntityManager;
    use super::ComponentMap;
    use ecs::world::MainAllocator;
    
    #[test]
    fn map_basics() {
        #[derive(Debug, PartialEq, Eq)]
        struct Counter(usize);
        
        let alloc = MainAllocator::new();
        
        let mut em = EntityManager::new(alloc);
        let mut map: ComponentMap<Counter> = ComponentMap::new(alloc);
        
        let first = em.next_entity();
        let second = em.next_entity();
        let third = em.next_entity();
        em.destroy_entity(third);
        
        {
            let mut handle = map.supply(&em);
            handle.set(first, Counter(0));
            // third is dead, set doesn't work.
            handle.set(third, Counter(1));
            
            assert_eq!(handle.get(first), Some(&Counter(0)));
            assert_eq!(handle.get(second), None);
            assert_eq!(handle.get(third), None); // third entity is not alive.
            
            assert_eq!(handle.remove(first), Some(Counter(0)));
            assert_eq!(handle.remove(second), None);
            assert_eq!(handle.remove(third), None);
        }
    }
}