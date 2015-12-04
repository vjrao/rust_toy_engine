mod entity;

pub use self::entity::{Entity, EntityManager};
use self::entity::index_of;

/// Maps entities to component data.
pub struct ComponentMap<T> {
    indices: Vec<Option<usize>>,
    data: Vec<Entry<T>>,
    // fifo deque / linked-list of free 
    // indices.
    first_free: Option<usize>,
    last_free: Option<usize>,
}

enum Entry<T> {
    Full(FullEntry<T>),
    Empty(Option<usize>),
}

struct FullEntry<T> {
    entity: Entity,
    data: T,
}

impl<T> ComponentMap<T> {
    // push an index onto the freelist
    fn freelist_pop(&mut self) -> Option<usize> {
        match self.first_free.take() {
            Some(index) => {
                let next = match self.data[index] {
                    Entry::Empty(next) => { next }
                    // full entries shouldn't ever get put in the linked list.
                    Entry::Full(_) => unreachable!(),
                };
                
                if next.is_none() {
                    // list is now empty.
                    self.last_free = None;
                }
                self.first_free = next;
                
                Some(index)
            }
            None => None
        }
    }
    
    fn freelist_push(&mut self, index: usize) {
        match self.last_free.take() {
            Some(last_free) => {
                self.data[last_free] = Entry::Empty(Some(index));
                self.last_free = Some(index);
            }
            None => {
                // if last free is empty, first free must also be empty.
                self.first_free = Some(index);
                self.last_free = Some(index);
            }
        }
        
        self.data[index] = Entry::Empty(None);
    }
    
    // get a new slot either by popping from the free list or 
    // increasing the size of the backing vector
    fn next_slot(&mut self) -> usize {
        if let Some(idx) = self.freelist_pop() {
            idx
        } else {
            self.data.push(Entry::Empty(None));
            
            self.data.len()
        }
    }
    
    /// Sets the component data for this entity, as long as it is alive.
    /// Returns a handle to the old data, if it existed.
    pub fn set(&mut self, e: Entity, data: T) -> Option<T> {
        use std::mem;
      
        let mut data = data;
        if let &Some(idx) = &self.indices[entity::index_of(e)] {
            match self.data[idx] {
                Entry::Full(ref mut full) => {
                    let old_gen = entity::generation_of(full.entity);
                    let new_gen = entity::generation_of(e);
                    if old_gen <= new_gen {
                        // supplied entity handle is not outdated.
                        // update it just in case it's newer.
                        full.entity = e;
                        if old_gen == new_gen {
                            mem::swap(&mut full.data, &mut data);
                            Some(data)
                        } else {
                            // just update if the existing handle is outdated.
                            full.data = data;
                            None
                        }
                    } else {
                        // supplied entity handle is outdated. no-op.
                        None
                    }
                }
                
                // if an entity's got an associated index,
                // there will always be an Full entry at that slot, even if
                // the entity is outdated. when we remove entries, we always set the
                // data to empty.
                _ => unreachable!(),
            }
        } else {
            // no index currently for this entity.
            let next_slot = self.next_slot();
            self.data[next_slot] = Entry::Full(FullEntry {
                entity: e,
                data: data,
            });
            
            None
        }
    }
    
    /// Removes the component data for this entity. Returns the old data,
    /// if it existed.
    pub fn remove(&mut self, e: Entity) -> Option<T> {
        use std::mem;
        
        let entity_idx = entity::index_of(e);
        if let &Some(idx) = &self.indices[entity_idx] {
            let mut empty = Entry::Empty(None);
            mem::swap(&mut self.data[idx], &mut empty);
            let old_entry = empty;
            
            if let Entry::Full(entry) = old_entry {
                let old_gen = entity::generation_of(entry.entity);
                let new_gen = entity::generation_of(e);
                
                if old_gen <= new_gen {
                    // remove the entry for this entity, but only return the data
                    // if the entity handle specified is exactly the same.
                    self.freelist_push(idx);
                    self.indices[entity_idx] = None;
                    if old_gen == new_gen {
                        Some(entry.data)
                    } else {
                        None
                    }
                } else {
                    // the handle used is outdated. restore the old data.
                    self.data[idx] = Entry::Full(entry);
                    None
                }
            } else {
                // no entry existed.
                None
            }
        } else {
            None
        }
    }
    
    /// Attempts to get a reference to this entity's component data.
    pub fn get(&self, e: Entity) -> Option<&T> {
        if let &Some(idx) = &self.indices[entity::index_of(e)] {
            if let Entry::Full(ref entry) = self.data[idx] {
                if entity::generation_of(e) == entity::generation_of(entry.entity) {
                    Some(&entry.data)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }
    
    /// Attempts to get a mutable reference to this entity's component data.
    pub fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        if let &mut Some(idx) = &mut self.indices[entity::index_of(e)] {
            if let Entry::Full(ref mut entry) = self.data[idx] {
                if entity::generation_of(e) == entity::generation_of(entry.entity) {
                    Some(&mut entry.data)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    fn it_works() {
        struct Position {
            x: i32,
            y: i32,
        }

        struct MovementProcessor;
        impl Processor for MovementProcessor {
            fn process(&mut self, world: WorldHandle) {
                world.query::<Position>().update_all_async(|p| {
                    Position {

                    }
                });
            }
        }
    }
}