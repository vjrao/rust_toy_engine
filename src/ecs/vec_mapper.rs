use super::{Entity, ComponentMapper, Component};
use std::collections::HashMap;

/// Default implementation of a component mapper.
/// It is backed by a vector and designed to be cache-friendly.
/// This should be sufficient for most component types.
pub struct VecMapper<T> {
    instance_data: Vec<T>,
    offsets: HashMap<Entity, usize>,
    unused_slots: Vec<usize>,
}

impl<T> VecMapper<T> {
    ///Creates a new VecMapper
    pub fn new() -> VecMapper<T> {
        VecMapper {
            instance_data: Vec::new(),
            offsets: HashMap::new(),
            unused_slots: Vec::new(),
        }
    }
}

impl<T: Component> ComponentMapper for VecMapper<T> {
    type Component = T;
    fn set(&mut self, e: Entity, c: T) {
        match self.offsets.get(&e) {
            Some(idx) => {
                self.instance_data[*idx] = c;
                return;
            }

            _ => {}
        }
        if let Some(idx) = self.unused_slots.pop() {
            self.offsets.insert(e, idx);
            self.instance_data[idx] = c;
        } else {
            self.instance_data.push(c);
            self.offsets.insert(e, self.instance_data.len() - 1);
        }
    }

    fn get(&self, e: Entity) -> Option<&T> {
        match self.offsets.get(&e) {
            Some(idx) => Some(&self.instance_data[*idx]),
            None => None
        }
    }

    fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        match self.offsets.get(&e) {
            Some(idx) => Some(&mut self.instance_data[*idx]),
            None => None
        }
    }

    fn remove(&mut self, e: Entity) {
        if let Some(idx) = self.offsets.remove(&e) {
            self.unused_slots.push(idx);
        }
    }

    fn entities(&self) -> Vec<Entity> {
        self.offsets.keys().cloned().collect()
    }
}