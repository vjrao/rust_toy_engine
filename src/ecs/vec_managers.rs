//! A default and generic implementation of ComponentManager.
//!
//! This stores all component data in a vector, where
//! offsets into the vector are stored in a hashmap.
//! Users of the library may find it is more efficient
//! to write custom ComponentManager types.
//! For example, some kind of spatial partitioning system would likely
//! be more efficient for storing Position data in many cases, particularly
//! for collision detection.

use super::{ComponentManager, Component, Entity};
use std::collections::{HashMap, VecDeque};

/// Default, generic, implementation of a component Manager.
/// It is backed by a vector and designed to be cache-friendly.
/// This should be sufficient for most component types.
pub struct VecManager<T> {
    instance_data: Vec<(Entity, T)>,
    offsets: HashMap<Entity, usize>,
    unused_slots: VecDeque<usize>,
}

impl<T> VecManager<T> {
    ///Creates a new VecManager
    pub fn new() -> VecManager<T> {
        VecManager {
            instance_data: Vec::new(),
            offsets: HashMap::new(),
            unused_slots: VecDeque::new(),
        }
    }
}

impl<T: Component> ComponentManager for VecManager<T> {
    type Component = T;
    fn set(&mut self, e: Entity, c: T) {
        match self.offsets.get(&e) {
            Some(idx) => {
                self.instance_data[*idx].1 = c;
                return;
            }

            _ => {}
        }
        if let Some(idx) = self.unused_slots.pop_front() {
            self.offsets.insert(e, idx);
            self.instance_data[idx] = (e, c);
        } else {
            self.instance_data.push((e, c));
            self.offsets.insert(e, self.instance_data.len() - 1);
        }
    }

    fn get(&self, e: Entity) -> Option<&T> {
        match self.offsets.get(&e) {
            Some(idx) => Some(&self.instance_data[*idx].1),
            None => None
        }
    }

    fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        match self.offsets.get(&e) {
            Some(idx) => Some(&mut self.instance_data[*idx].1),
            None => None
        }
    }

    fn remove(&mut self, e: Entity) {
        if let Some(idx) = self.offsets.remove(&e) {
            self.unused_slots.push_back(idx);
        }
    }

    fn entities(&self) -> Vec<Entity> {
        self.instance_data.iter().map(|p| p.0).collect()
    }
}