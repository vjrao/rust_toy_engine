
// different ways of storing indices of entities
// maps are used if there are few entities in this array.
// arrays are used if there are many entities in this array.
enum IndexMode {
    Map(HashMap<Entity, usize>),
}

/// A representation of a query in data.
trait QuerySpec: Clone {
    /// The type produced when component data is loaded for an entity.
    type Entry;

    /// Whether this query spec has this component type
    fn has<T: Component>(&self) -> bool where Self: Sized;

    /// Tries to load an entry for an entity.
    fn load<M: ManagerLookup>(&self, &M , Entity) -> Option<Self::Entry> where Self: Sized;

    /// The number of components this query spec has.
    fn num_components(&self) -> usize;

    /// How many times each entity appears in this spec.
    fn entity_appearances<M: ManagerLookup>(&self, &M) -> HashMap<Entity, usize> where Self: Sized;

    /// Returns a vector of the depths of each of its components in arbitrary order.
    fn depths<M: ManagerLookup>(&self, &M) -> Vec<usize> where Self: Sized; 

    /// Adds a component to this query spec
    fn with_component<T: Component>(self) -> Result<(Self, PhantomData<T>), Self> where Self: Sized {
        if self.has::<T>() { Err(self) }
        else { Ok((self, PhantomData)) }
    }
}

// The unit type is at the root of every query spec.
// Since the methods of query spec are implemented in a recursive-like manner,
// all the implementations here will return the base cases for them.
impl QuerySpec for () {
    type Entry = ();
    fn has<T: Component>(&self) -> bool { false }
    fn load<M: ManagerLookup>(&self, managers: &M, e: Entity) -> Option<()> { Some(()) }
    fn num_components(&self) -> usize { 0 }
    fn entity_appearances<M: ManagerLookup>(&self, _m: &M) -> HashMap<Entity, usize> {
        HashMap::<Entity, usize>::new()
    }
    fn depths<M: ManagerLookup>(&self, _m: &M) -> Vec<usize> {
        Vec::new()
    }
}

// QuerySpecs are largely implemented as "Nested" 0-sized tuples.
impl<Q: QuerySpec, C: Component> QuerySpec for (Q, PhantomData<C>) {
    /// shouldn't really clone the data whenever it's queried, if it doesn't need to be.
    type Entry = (Q::Entry, C);

    fn has<T: Component>(&self) -> bool {
        if TypeId::of::<T>() == TypeId::of::<C>() { true }
        else { self.0.has::<T>() }
    }

    fn load<M: ManagerLookup>(&self, managers: &M, e: Entity) -> Option<Self::Entry> {
        self.0.load(managers, e).and_then(|entry| {
            managers.lookup::<C>()
            .and_then(|manager| manager.get(e))
            .map(|c| (entry, c.clone()))
        })
    }

    fn num_components(&self) -> usize { self.0.num_components() + 1 }

    fn entity_appearances<M: ManagerLookup>(&self, managers: &M) -> HashMap<Entity, usize> {
        let mut map = self.0.entity_appearances(managers);
        let manager = managers.lookup::<C>().expect("no manager found for component");
        for (entity, _) in manager.component_data() {
            let entry = map.entry(entity).or_insert(0);
            *entry += 1;
        }
        map
    }

    fn depths<M: ManagerLookup>(&self, managers: &M) -> Vec<usize> {
        let mut v = self.0.depths(managers);
        v.push(managers.depth_of::<C>().ok().expect("no manager found for component"));
        v
    }
}

pub trait QueryItem {
    fn get<T: Component>(&self) -> Option<&T> where Self: Sized;
}

/// A cold query just asks component managers for component data as needed.
pub struct ColdQuery<'a, Q, M: 'a> {
    spec: Q,
    managers: &'a M,
    entities: <Vec<Entity> as IntoIterator>::IntoIter,
}

pub struct ColdQueryItem<'a, Q, M: 'a> {
    spec: Q,
    managers: &'a M,
    entity: Entity,
}

impl<'a, Q: QuerySpec, M: 'a + ManagerLookup> QueryItem for ColdQueryItem<'a, Q, M> {
    fn get<T: Component>(&self) -> Option<&T> {
        if self.spec.has::<T>() { 
            self.managers.lookup::<T>().unwrap().get(self.entity) 
        } else {
            None
        }
    }
}

impl<'a, Q: QuerySpec, M: 'a + ManagerLookup> Iterator for ColdQuery<'a, Q, M> {
    type Item = ColdQueryItem<'a, Q, M>;

    fn next(&mut self) -> Option<Self::Item> {
        self.entities.next().map(|e| ColdQueryItem {
            spec: self.spec.clone(),
            managers: self.managers,
            entity: e
        })
    }
}

impl<'a, Q: QuerySpec, M: 'a + ManagerLookup> ColdQuery<'a, Q, M> {
    fn new(spec: Q, managers: &'a M) -> Self {
        let num_components = spec.num_components();
        let entities = spec.entity_appearances(managers).iter()
        .filter_map(|(e, v)| {
            if *v == num_components { Some(*e) }
            else { None }
        }).collect::<Vec<_>>();
        ColdQuery {
            spec: spec,
            managers: managers,
            entities: entities.into_iter(),
        }
    }
}