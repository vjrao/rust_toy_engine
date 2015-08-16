//! Provides `World` and `WorldBuilder` functionality.

use std::any::Any;
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};

use super::{
    Component,
    ComponentMapper,
    ComponentMappers,
    Entity,
    System,
};

/// Manages the creation and destruction of entities.
pub struct EntityManager {
    next: Entity,
    entities: HashSet<Entity>,
}

impl EntityManager {
    pub fn new() -> Self {
        EntityManager {
            next: Entity { id: 0 },
            entities: HashSet::new()
        }
    }
    /// Creates an entity.
    pub fn next_entity(&mut self) -> Entity {
        while self.is_alive(self.next) {
            self.next.id += 1;
        }
        self.entities.insert(self.next);
        self.next
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
        self.entities.contains(&e)
    }

    /// Destroy an entity.
    pub fn destroy_entity(&mut self, e: Entity) {
        self.entities.remove(&e);
    }
}

/// The `World` ties together entities, components, and systems.
pub struct World {
    entity_manager: EntityManager,
    component_mappers: ComponentMappers,
    systems: Vec<Box<System>>,
}

impl World {
    /// Process all the systems in this world in arbitrary order.
    pub fn process_systems(&mut self) {
        for sys in &mut self.systems {
            sys.process(&self.component_mappers, &self.entity_manager);
        }
    }
}

impl Deref for World {
    type Target = ComponentMappers;
    fn deref(&self) -> &ComponentMappers {
        &self.component_mappers
    }
}

impl DerefMut for World {
    fn deref_mut(&mut self) -> &mut ComponentMappers {
        &mut self.component_mappers
    }
}

/// Factory for `World`. 
pub struct WorldBuilder {
    component_mappers: ComponentMappers,
    systems: Vec<Box<System>>,
}

impl WorldBuilder {
    /// Create a new `WorldBuilder`.
    pub fn new() -> WorldBuilder {
        WorldBuilder { 
            component_mappers: ComponentMappers::new(),
            systems: Vec::new() 
        }
    }

    /// Add a component mapper to this world.
    pub fn with_component_mapper<T, M>(self, mapper: M) -> WorldBuilder
    where T: Component, M: ComponentMapper<Component=T> + Any {
        let mut s = self;
        s.component_mappers.insert(mapper);
        s
    }

    /// Add a system to this world.
    pub fn with_system<S>(self, sys: S) -> WorldBuilder
    where S: System + 'static {
        let mut s = self;
        s.systems.push(Box::new(sys));
        s
    }

    /// Builds a world from self.
    pub fn build(self) -> World {
        World {
            entity_manager: EntityManager::new(),
            component_mappers: self.component_mappers,
            systems: self.systems
        }
    }

}