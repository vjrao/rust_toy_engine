//! Mechanisms for handling component data for entities.

use super::{
    Component,
    Filterable,
    Entity,
};

use std::any::TypeId;
use std::mem;
use std::ops::Index;
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
    fn entities_filtered(&self, f: &<Self::Component as Filterable>::Filter)
    -> Vec<Entity> where Self::Component: Filterable;
}

impl<T: Component> Index<Entity> for ComponentMapper<Component=T> {
    type Output = T;

    /// Indexes into the component mapper.
    /// This should only be used when an entity is known to have
    /// the component.
    fn index<'a>(&'a self, e: Entity) -> &'a T {
        let opt = self.get(e);
        debug_assert!(opt.is_some());
        opt.unwrap()
    }
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
                                -> &ComponentMapper<Component=T> {
        let mapper: Option<&ComponentMapper<Component=T>>
        = self.0.get(&TypeId::of::<T>()).map(|h|
            unsafe { 
                mem::transmute_copy(&h.obj) 
            }
        );
        debug_assert!(mapper.is_some());
        mapper.unwrap()
    }

    /// Get a mutable reference to the component mapper for this type.
    pub fn get_mapper_mut<T: Component>(&mut self)
                                -> &mut ComponentMapper<Component=T> {
        let mapper: Option<&mut ComponentMapper<Component=T>>
        = self.0.get_mut(&TypeId::of::<T>()).map(|h|
            unsafe {mem::transmute_copy(&h.obj) }
        );
        debug_assert!(mapper.is_some());
        mapper.unwrap()
    }

    /// Create a query for selecting specific entities.
    pub fn query<'a>(&'a self) -> EntityQuery<'a> {
        EntityQuery {
            mappers: self,
            num_components: 0,
            candidates: Vec::new(),
            disallowed: Vec::new(),
        }
    }
}

/// A builder for entity queries.
pub struct EntityQuery<'a> {
    mappers: &'a ComponentMappers,
    num_components: usize,
    candidates: Vec<Vec<Entity>>,
    disallowed: Vec<Vec<Entity>>,
}

impl<'a> EntityQuery<'a> {
    /// Causes this query to only return entities with a component of type `C`.
    pub fn with_component<C>(mut self) -> Self
    where C: Component {
        let mapper = self.mappers.get_mapper::<C>();

        self.candidates.push(mapper.entities());
        self.num_components += 1;

        self
    }

    /// Causes this query to only return entities with a component of type `C`,
    /// Filtered with the associated filter.
    pub fn with_component_filtered<C, F>(mut self, filter: C::Filter) -> Self
    where C: Filterable {
        let mapper = self.mappers.get_mapper::<C>();
        self.candidates.push(mapper.entities_filtered(&filter));
        self.num_components += 1;

        self
    }

    /// Causes this query to only return entities without a component of type `C`.
    pub fn without_component<C>(mut self) -> Self
    where C: Component {
        let mapper = self.mappers.get_mapper::<C>();
        self.disallowed.push(mapper.entities());
        self
    }
}

/// The implementation of IntoIterator for EntityQuery executes
/// the query on the mappers.
impl<'a> IntoIterator for EntityQuery<'a> {
    type Item = Entity;
    type IntoIter = <Vec<Entity> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        let mut map = HashMap::new();
        let mut entities = Vec::new();

        // find the intersection between all the candidate lists
        for e in self.candidates.iter().flat_map(|v| v.iter()) {
            let i = map.entry(e).or_insert(0usize);
            *i += 1;
        }

        // but ensure that no disallowed entities will be included.
        for e in self.disallowed.iter().flat_map(|v| v.iter()) {
            map.insert(e, 0);
        }

        // collect all entities in the intersection
        for (e, i) in map {
            if i == self.num_components { entities.push(*e) }
        }

        entities.into_iter()
    }
}