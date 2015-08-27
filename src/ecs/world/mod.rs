//! Provides `World` and `WorldBuilder` functionality.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::mpsc::channel;

use super::{
    Component,
    ComponentEdit,
    ComponentMapper,
    ComponentMappers,
    EntityManager,
    System,
};

pub use self::deferred::{
    EntityCreationGuard,
    WorldHandle,
};

mod deferred;

/// The `World` ties together entities, components, and systems.
pub struct World {
    entity_manager: EntityManager,
    component_mappers: ComponentMappers,
    systems: Vec<Box<System>>,
}

impl World {
    /// Process all the systems in this world in arbitrary order.
    pub fn process_systems(&mut self) {
        use self::deferred::{
            EditHolder,
            Message,
            world_handle,
        };

        // process systems
        let (message_tx, message_rx) = channel();
        {
            let handle = world_handle(
                &self.entity_manager,
                &self.component_mappers,
                message_tx,
            );
            for sys in &mut self.systems {
                sys.process(&handle.clone());
            }
        }

        let mut edit_holders: HashMap<TypeId, Box<EditHolder>> = HashMap::new();
        // process messages
        for message in message_rx {
            match message {
                Message::Next(n, init) => { 
                    let e_vec = self.entity_manager.next_entities(n);
                    for e in e_vec {
                        init(&mut self.component_mappers, e);
                    }
                }
                Message::Edit(id, entity, edit, init) => {
                    let edit_holder = edit_holders.entry(id).or_insert(init());
                    edit_holder.push(entity, edit);
                }
                Message::Destroy(e) => {
                    self.entity_manager.destroy_entity(e);
                }
            }
        }

        for edit_holder in edit_holders.values() {
            edit_holder.apply(&mut self.component_mappers, &self.entity_manager);
        }
    }

    /// Gets a mutable reference to the entity manager.
    pub fn entity_manager(&mut self) -> &mut EntityManager {
        &mut self.entity_manager
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