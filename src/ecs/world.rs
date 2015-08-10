//! Provides `World` and `WorldBuilder` functionality.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};

use super::{
    Component,
    ComponentMapper,
    ComponentMappers,
    Entity,
    MapperHandle,
    System,
    Prototype
};

/// The `World` ties together entities, components, and systems.
pub struct World {
    next: Entity,
    entities: HashSet<Entity>,
    component_mappers: ComponentMappers,
    systems: Vec<Box<System>>,
}

impl World {
    /// Process all the systems in this world in arbitrary order.
    pub fn process_systems(&mut self) {
        for sys in &mut self.systems {
            let mut counts: HashMap<Entity, usize> = HashMap::new();
            let comps = sys.dependent_components();
            let mut entity_vecs = Vec::new();
            for c in &comps {
                entity_vecs.push(
                    &self.component_mappers
                    .get_handle(c)
                    .unwrap()
                    .entities()
                );
            }
            for entity in entity_vecs.into_iter().flat_map(|v|
                v.into_iter()
            ) {
                let counter = counts.entry(*entity).or_insert(0);
                *counter += 1;
            }
            
            let entities = counts.iter().filter_map(|(e, c)|
                if *c == comps.len() { Some(*e) }
                else { None }
            ).collect::<Vec<_>>();
            
            if entities.len() == 0 { continue }
            
            sys.process(&entities, &mut self.component_mappers);
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

    /// Creates an entity, initializing it with a prototype.
    pub fn next_entity_prototyped<P: Prototype>(&mut self, proto: &P) -> Entity {
        let e = self.next_entity();
        proto.initialize(e, &mut self.component_mappers);
        e
    }

    /// Creates a vector of n entities.
    pub fn next_entities(&mut self, n: usize) -> Vec<Entity> {
        let mut entities = Vec::with_capacity(n);
        for _ in 0..n {
            entities.push(self.next_entity());
        }
        entities
    }

    /// Creates n entities, initialized with a prototype.
    pub fn next_entities_prototyped<P>(&mut self, n: usize, proto: P) -> Vec<Entity>
    where P: Prototype {
        let entities = self.next_entities(n);
        for e in &entities {
            proto.initialize(*e, &mut self.component_mappers);
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
        for (_, h) in self.component_mappers.0.iter_mut() {
            h.handle.remove(e);
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
            component_mappers: ComponentMappers(HashMap::new()),
            systems: Vec::new() 
        }
    }

    /// Add a component mapper to this world.
    pub fn with_component_mapper<T, M>(self, mapper: M) -> WorldBuilder
    where T: Component, M: ComponentMapper<Component=T> + Any {
        let mut s = self;
        s.component_mappers.0.insert(
            M::Component::id(),
            MapperHandle::from_mapper(mapper)
        );
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
            next: Entity { id: 0 },
            entities: HashSet::new(),
            component_mappers: self.component_mappers,
            systems: self.systems
        }
    }

}