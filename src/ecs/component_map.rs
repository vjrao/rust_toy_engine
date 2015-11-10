use std::any::{Any, TypeId};
use std::marker::PhantomData;

use super::{Component, CowHandle, DataMap, WriteHandle};

/// A recursive variadic trait which maps component types to data maps.
pub trait ComponentMap {
    /// Try to get a data map for this component type.
    fn get<T: Component>(&self) -> Option<&DataMap<T>> where Self: Sized;

    /// Try to get a mutable reference to a data map for this component type.
    fn get_mut<T: Component>(&mut self) -> Option<&mut DataMap<T>> where Self: Sized;

    /// Update values of this from the other.
    fn update_from<T: ComponentMap>(&mut self, other: T) where Self: Sized;
}

/// A recursive variadic trait which maps component types to syncrhonized data maps.
pub trait SyncComponentMap: Clone {
    /// Try to get a data map for this component type.
    fn get<T: Component>(&self) -> Option<&DataMap<T>> where Self: Sized;

    /// Try to get a mutable reference to a data map for this component type.
    /// This yields a reference to a write-only array behind a mutex.
    fn get_mut<T: Component>(&self) -> Option<WriteHandle<DataMap<T>>> where Self: Sized;
}

/// Something that can transform into a Copy-On-Write variant.
pub trait MakeCowMap<'a> {
    type Output: 'a + SyncComponentMap + MaybeIntoOwnedMap;

    /// Make a COW map variant of this.
    fn make_cow_map(&'a self) -> Self::Output;
}

// Something that can consume itself and hold all the owned values.
pub trait MaybeIntoOwnedMap {
    type Output: ComponentMap + 'static;

    fn into_owned_map(self) -> Self::Output;
}

/// Base case of ComponentMap.
#[derive(Clone)]
pub struct Empty;

/// Maybe an entry, maybe not.
/// Used in MaybeIntoOwnedMap.
#[derive(Clone)]
pub enum MaybeEntry<M: ComponentMap, T: Component> {
    Success(Entry<M, T, DataMap<T>>),
    Failure(M),
}

/// Non-empty case of ComponentMap
#[derive(Clone)]
pub struct Entry<M, C: Component, D> {
    data: D,
    parent: M,
    _marker: PhantomData<C>,
}


impl ComponentMap for Empty {
    fn get<T: Component>(&self) -> Option<&DataMap<T>> { None }
    fn get_mut<T: Component>(&mut self) -> Option<&mut DataMap<T>> { None }
    fn update_from<T: ComponentMap>(&mut self, _: T) {}
}

impl<M: ComponentMap, C: Component> ComponentMap for Entry<M, C, DataMap<C>> {
    fn get<T: Component>(&self) -> Option<&DataMap<T>> { 
        if TypeId::of::<C>() == TypeId::of::<T>() {
            // can transmute if this turns out to be slow.
            // it shouldn't be, though.
            Some((&self.data as &Any).downcast_ref().unwrap())
        } else {
            self.parent.get()
        }
    }

    fn get_mut<T: Component>(&mut self) -> Option<&mut DataMap<T>> { 
        if TypeId::of::<C>() == TypeId::of::<T>() {
            Some((&mut self.data as &mut Any).downcast_mut().unwrap())
        } else {
            self.parent.get_mut()
        }
    }

    fn update_from<T: ComponentMap>(&mut self, other: T) {
        if let Some(new_val) = other.get::<C>() {
            self.data = new_val.clone();
        }

        self.parent.update_from(other);
    }
}

impl<M: ComponentMap, C: Component> ComponentMap for MaybeEntry<M, C> {
    fn get<T: Component>(&self) -> Option<&DataMap<T>> { 
        match self {
            &MaybeEntry::Success(ref val) => val.get(),
            &MaybeEntry::Failure(ref parent) => parent.get() ,
        }
    }

    fn get_mut<T: Component>(&mut self) -> Option<&mut DataMap<T>> { 
        match self {
            &mut MaybeEntry::Success(ref mut val) => val.get_mut(),
            &mut MaybeEntry::Failure(ref mut parent) => parent.get_mut(),
        }
    }

    fn update_from<T: ComponentMap>(&mut self, other: T) {
        match self {
            &mut MaybeEntry::Success(ref mut val) => val.update_from(other),
            &mut MaybeEntry::Failure(ref mut parent) => parent.update_from(other),
        }
    }
}

impl<'a> MakeCowMap<'a> for Empty {
    type Output = Self;
    fn make_cow_map(&'a self) -> Self { Empty }
}

impl MaybeIntoOwnedMap for Empty {
    type Output = Self;
    fn into_owned_map(self) -> Self { Empty }
}

impl SyncComponentMap for Empty {
    fn get<T: Component>(&self) -> Option<&DataMap<T>> { None }
    fn get_mut<T: Component>(&self) -> Option<WriteHandle<DataMap<T>>> { None }
}

impl<'a, M: ComponentMap + MakeCowMap<'a>, C: Component> MakeCowMap<'a> for Entry<M, C, DataMap<C>> {
    type Output = Entry<<M as MakeCowMap<'a>>::Output, C, CowHandle<'a, DataMap<C>>>;

    fn make_cow_map(&'a self) -> Self::Output {
        Entry {
            data: CowHandle::new(&self.data),
            parent: self.parent.make_cow_map(),
            _marker: PhantomData,
        }
    }
}

impl<'a, M: SyncComponentMap + MaybeIntoOwnedMap, C: Component> SyncComponentMap for Entry<M, C, CowHandle<'a, DataMap<C>>> {
    fn get<T: Component>(&self) -> Option<&DataMap<T>> {
        use std::mem; 

        if TypeId::of::<C>() == TypeId::of::<T>() {
            let read: &DataMap<C> = self.data.get_read();
            Some(unsafe { mem::transmute::<&DataMap<C>, &DataMap<T>>(read) })
        } else {
            self.parent.get()
        }
    }

    fn get_mut<'b, T: Component>(&'b self) -> Option<WriteHandle<'b, DataMap<T>>> {
        use std::mem;
        if TypeId::of::<C>() == TypeId::of::<T>() {
            let handle = self.data.lock_write();
            Some(unsafe { mem::transmute(handle) } )
        } else {
            self.parent.get_mut()
        }
    }
}

impl<'a, M: MaybeIntoOwnedMap, C: Component> MaybeIntoOwnedMap for Entry<M, C, CowHandle<'a, DataMap<C>>> {
    type Output = MaybeEntry<M::Output, C>;

    fn into_owned_map(self) -> MaybeEntry<M::Output, C> {
        let parent_owned = self.parent.into_owned_map();
        if let Some(new_val) = self.data.into_inner() {
            MaybeEntry::Success(Entry {
                data: new_val,
                parent: parent_owned,
                _marker: PhantomData,
            })
        } else {
            MaybeEntry::Failure(parent_owned)
        }
    }
}