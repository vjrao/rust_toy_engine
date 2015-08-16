use super::{
    Entity,
    Component,
};

use std::any::{Any, TypeId};
use std::mem;
use std::raw::TraitObject;
use std::collections::HashMap;

/// Maps entities to component data.
pub trait ComponentMapper {
    type Component: Component;
    
    /// Get an immutable reference to an entity's component data.
    fn get(&self, e: Entity) -> Option<&Self::Component>;
    /// Get a mutable reference to an entity's component data.
    fn get_mut(&mut self, e: Entity) -> Option<&mut Self::Component>;
    /// Sets the component data for a specific entity.
    fn set(&mut self, e: Entity, c: Self::Component);
    /// Stop managing component data for this entity.
    fn remove(&mut self, e: Entity);
    /// Get a vector of all the entities this manages.
    fn entities(&self) -> Vec<Entity>;
    /// Get a vector of all the entities this manages which fit the supplied filter.
    fn entities_filtered(&self, f: <Self::Component as Component>::Filter)
    -> Vec<Entity>;
}

struct MapperHandle {
    obj: TraitObject,
    mapper: Box<ComponentMapperExt>,
}

// Contains conduits for `ComponentMapper` methods which are not generic.
trait ComponentMapperExt: 'static {
    fn remove(&mut self, e: Entity);
}

impl<T, M> ComponentMapperExt for M
where T: Component, M: ComponentMapper<Component=T> + 'static {
    fn remove(&mut self, e: Entity) { M::remove(self, e) }
}

impl MapperHandle {
    fn from_mapper<C, M>(mapper: M) -> Self 
    where C: Component, M: ComponentMapper<Component=C> + 'static {
        let mut mapper = Box::new(mapper);
        let obj: TraitObject = unsafe 
            { mem::transmute(&mut *mapper as &mut ComponentMapper<Component=C>) };
        MapperHandle {
            obj: obj,
            mapper: mapper as Box<ComponentMapperExt>
        }
    }
}

/// Stores all component mappers.
pub struct ComponentMappers(HashMap<TypeId, MapperHandle>);

impl ComponentMappers {
    pub fn new() -> Self {
        ComponentMappers(HashMap::new())
    }
    pub fn insert<T, M>(&mut self, mapper: M)
    where T: Component, M: ComponentMapper<Component=T> + 'static {
        self.0.insert(TypeId::of::<T>(), MapperHandle::from_mapper(mapper));
    }
    /// Get an immutable reference to the component mapper for this type.
    pub fn get_mapper<T: Component>(&self)
                                -> Option<&ComponentMapper<Component=T>> {
        self.0.get(&TypeId::of::<T>()).map(|h|
            unsafe { mem::transmute_copy(&h.obj) }
        )
    }

    /// Get a mutable reference to the component mapper for this type.
    pub fn get_mapper_mut<T: Component>(&mut self)
                                -> Option<&mut ComponentMapper<Component=T>> {
        self.0.get_mut(&TypeId::of::<T>()).map(|h|
            unsafe { mem::transmute_copy(&h.obj) }
        )
    }
}