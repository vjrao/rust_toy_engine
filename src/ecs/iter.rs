use super::{
    Component,
    ComponentMapper,
    ComponentMappers,
    Entity,
};

use std::iter::Filter;
use std::marker::PhantomData;

pub struct EntityIter<'a, I> {
    mappers: &'a ComponentMappers,
    iter: I
}

impl<'a, I> Iterator for EntityIter<'a, I>
where I: Iterator<Item=Entity> {
    type Item = Entity;
    fn next(&mut self) -> Option<Entity> { self.iter.next() }
}

impl<'a, I> EntityIter<'a, I> where I: Iterator<Item=Entity> {
    /// Filters this iter to only include entities with a component of type `C`.
    pub fn with_component<C>(self) 
    -> EntityIter<'a, Filter<Self, WithComponent<C>>>
    where C: Component, Self: Sized {
        let with = WithComponent::new(self.mappers);
        EntityIter {
            mappers: self.mappers,
            iter: self.filter(with)
        }
    }

    /// Filters this iter to only include entities with a component of type `C`.
    pub fn with_component_filtered<C, F>(self, filter: F) 
    -> EntityIter<'a, Filter<Self, WithComponent<C>>>
    where C: Component<Filter=F>, Self: Sized {
        let with = WithComponent::new_filtered(self.mappers, filter);
        EntityIter {
            mappers: self.mappers,
            iter: self.filter(with)
        }
    }

    /// Filters this iter to only include entities without a component of type `C`
    pub fn without_component<C>(self) 
    -> EntityIter<'a, Filter<Self, WithoutComponent<'a, C>>>
    where C: Component, Self: Sized {
        let without = WithoutComponent::new(self.mappers);
        EntityIter {
            mappers: self.mappers,
            iter: self.filter(without)
        }
    }
}

/// Keeps entities which have a specific component.
pub struct WithComponent<C> {
    entities: Vec<Entity>,
    _marker: PhantomData<C>
}

impl<'a, C> WithComponent<C> where C: Component {
    fn new(mappers: &'a ComponentMappers) -> Self {
        let entities = 
            mappers.get_mapper::<C>().map_or(Vec::new(), |m| m.entities());

        WithComponent { entities: entities, _marker: PhantomData }
    }

    fn new_filtered(mappers: &'a ComponentMappers, 
                    filter: C::Filter) -> Self {
        let entities =
            mappers.get_mapper::<C>()
            .map_or(Vec::new(), |m| m.entities_filtered(filter));
        WithComponent { entities: entities, _marker: PhantomData }
    } 
}

impl<'r, C: Component> FnOnce<(&'r Entity,)> for WithComponent<C> {
    type Output = bool;
    extern "rust-call" fn call_once(mut self, e: (&'r Entity,)) -> bool {
        self.call_mut(e)
    }
}

impl<'r, C: Component> FnMut<(&'r Entity,)> for WithComponent<C> {
    extern "rust-call" fn call_mut(&mut self, e: (&'r Entity,)) -> bool {
        for e2 in &self.entities { 
            if *e2 == *e.0 { return true }
        }
        return false
    }
}

/// Keeps entities which don't have a specific component.
pub struct WithoutComponent<'a, C> {
    mapper: Option<&'a ComponentMapper<Component=C>>,
    _marker: PhantomData<C>
}

impl<'a, C> WithoutComponent<'a, C> where C: Component {
    fn new(mappers: &'a ComponentMappers) -> Self {
        WithoutComponent {
            mapper: mappers.get_mapper::<C>(),
            _marker: PhantomData
        }
    }
}

impl<'a, 'r, C: Component> FnOnce<(&'r Entity,)> for WithoutComponent<'a, C> {
    type Output = bool;
    extern "rust-call" fn call_once(mut self, e: (&'r Entity,)) -> bool {
        self.call_mut(e)
    }
}

impl<'a, 'r, C: Component> FnMut<(&'r Entity,)> for WithoutComponent<'a, C> {
    extern "rust-call" fn call_mut(&mut self, e: (&'r Entity,)) -> bool {
        !self.mapper.map(|m| m.get(*e.0)).is_some()
    }
}