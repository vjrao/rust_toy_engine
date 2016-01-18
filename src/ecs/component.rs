//! This module contains component-related types and traits.
//!
//! Included are some type-level component lists.
//! this disgusting type system abuse culminates in various types of mixed-type component lists.
//! These aren't really to be used externally, but instead provide some level of convenience
//! and abstraction over the components which worlds operate over.
//!
//! These lists will most likely have O(1) access to members, at least when compiled with optimizations.
//! If `TypeId::of` ever becomes a const fn, they definitely will.
use std::any::{Any, TypeId};
use std::intrinsics;
use std::marker::PhantomData;
use std::mem;

use super::entity::Entity;
use super::internal::ComponentOffsetTable;
use super::world::WorldAllocator;
use super::{COMPONENT_ALIGN, LARGE_SIZE};

/// Components are arbitrary data associated with entities.
pub trait Component: Any + Send + Sync {
    /// A list of dependencies which this component has, i.e.
    /// every entity with this component must also have each of
    /// components in the set.
    ///
    /// The set is expressed as a tuple of component types,
    /// e.g. `(A, B, C)` or `(Transform, Sprite)`.
    type Dependencies: ComponentSet = ();
}

// any checks for component types which cannot be done at compile-time
// will be done here.
fn assert_component_compliance<T: Component>() {
    use std::mem;

    assert!(mem::align_of::<T>() <= COMPONENT_ALIGN,
            "Component alignment too large. Maximum supported is {} bytes.",
            COMPONENT_ALIGN);

    assert!(mem::size_of::<T>() <= LARGE_SIZE,
            "Component size too large. Maximum supported is {} bytes.",
            LARGE_SIZE);
}

/// //////////////////////////////////////////////////
/// // Type-level component lists ////////////////////
/// //////////////////////////////////////////////////

/// Non-empty case of a list entry.
pub struct ListEntry<T: Any, P> {
    val: T,
    parent: P,
}

/// Empty case of a list entry.
pub struct Empty(PhantomData<()>);

#[doc(hidden)]
pub fn make_empty() -> Empty {
    Empty(PhantomData)
}

// A list of components where each entry is a zero-sized PhantomData.
// This is used to signify which components a world will manage, without yet constructing
// the offset tables themselves.
pub trait PhantomComponents {
    type Components: Components;
    // Push another component type onto this list. This additionally verifies
    // that no duplicates are pushed.
    fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Self> where Self: Sized;
    fn has<T: Component>(&self) -> bool;
    fn into_components(self, alloc: WorldAllocator) -> Self::Components;
}

impl PhantomComponents for Empty {
    type Components = Empty;

    fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Empty> {
        assert_component_compliance::<T>();
        ListEntry {
            val: PhantomData,
            parent: self,
        }
    }

    fn has<T: Component>(&self) -> bool {
        false
    }
    fn into_components(self, _: WorldAllocator) -> Empty {
        make_empty()
    }
}

impl<C: Component, P: PhantomComponents> PhantomComponents for ListEntry<PhantomData<C>, P> {
    type Components = ListEntry<ComponentOffsetTable<C>, P::Components>;

    fn push<T: Component>(self) -> ListEntry<PhantomData<T>, Self> {
        if self.has::<T>() {
            panic!("Added type to components list twice.")
        }
        assert_component_compliance::<T>();

        ListEntry {
            val: PhantomData,
            parent: self,
        }
    }

    fn has<T: Component>(&self) -> bool {
        (TypeId::of::<T>() == TypeId::of::<C>()) || self.parent.has::<T>()
    }

    fn into_components(self, alloc: WorldAllocator) -> Self::Components {
        ListEntry {
            val: ComponentOffsetTable::new(alloc),
            parent: self.parent.into_components(alloc),
        }
    }
}

// A list where each entry is an offset table for that component.
pub trait Components: Sized + Send + Sync {
    // Get a reference to the offset table for the given component type.
    // This panics if the component type is not present in this list.
    fn get<T: Component>(&self) -> &ComponentOffsetTable<T>;

    // Get a mutable reference to the offset table for the given component type.
    // This panics if the component type is not present in this list.
    fn get_mut<T: Component>(&mut self) -> &mut ComponentOffsetTable<T>;

    // Remove all component offsets from each entity in the slice.
    // we batch this because each component offset table may be located
    // somewhere else in memory, so we try to minimize cache misses.
    fn clear_components_for(&mut self, entities: &[Entity]);

    // assert that all component dependencies are fulfilled.
    // this is a pretty costly operation, so it will probably only be
    // performed in debug mode.
    fn assert_dependencies<T: Components>(&self, top_level: &T, entity: Entity);
}

impl Components for Empty {
    fn get<T: Component>(&self) -> &ComponentOffsetTable<T> {
        panic!("No such component in tables list.");
    }

    fn get_mut<T: Component>(&mut self) -> &mut ComponentOffsetTable<T> {
        panic!("No such component in tables list.");
    }

    fn clear_components_for(&mut self, _: &[Entity]) {}

    fn assert_dependencies<T: Components>(&self, _: &T, _: Entity) {}
}

impl<C: Component, P: Components> Components for ListEntry<ComponentOffsetTable<C>, P> {
    fn get<T: Component>(&self) -> &ComponentOffsetTable<T> {
        // TypeIds should have resolved to concrete values by the optimization phase.
        // My guess is that every monomorphization of this function will be inlined into
        // a large if-else tree, which will then be optimized down to a single return or panic.
        if TypeId::of::<T>() == TypeId::of::<C>() {
            unsafe { mem::transmute(&self.val) }
        } else {
            self.parent.get()
        }
    }

    fn get_mut<T: Component>(&mut self) -> &mut ComponentOffsetTable<T> {
        if TypeId::of::<T>() == TypeId::of::<C>() {
            unsafe { mem::transmute(&mut self.val) }
        } else {
            self.parent.get_mut()
        }
    }

    fn clear_components_for(&mut self, entities: &[Entity]) {
        for entity in entities.iter() {
            self.val.remove(*entity);
        }

        self.parent.clear_components_for(entities);
    }

    fn assert_dependencies<T: Components>(&self, top_level: &T, entity: Entity) {
        if self.val.offset_of(entity).is_some() {
            if let Err(failed_name) = C::Dependencies::has_all(top_level, entity) {
                panic!("Entity {} failed to have component {}.\nDependency required by component \
                        {}",
                       entity,
                       failed_name,
                       unsafe { intrinsics::type_name::<C>() });
            }
        }

        self.parent.assert_dependencies(top_level, entity);
    }
}

/// Trait for sets of components. This is intended to be used by the general user in the form of its
/// tuple implementation as arguments to queries, dependencies for components, and possibly more.
/// The trait itself shouldn't be implemented by the user, and using custom implementations will
/// most likely break functionality of the ECS.
pub trait ComponentSet {
    /// Whether this component set contains the type T.
    fn contains<T: Component>() -> bool;

    /// Whether an entity has all of the components in this set.
    /// This will either return Ok, or an error with the name of the first component it failed
    /// to have.
    fn has_all<C: Components>(components: &C, e: Entity) -> Result<(), &'static str>;
}

macro_rules! impl_component_set {
    ($($comp:ident, )*) => {
        impl<
            $($comp: Component,)*
        >
        ComponentSet for
        ($($comp, )*) {
            fn contains<OTHER: Component>() -> bool {
                $(
                    if TypeId::of::<OTHER>() == TypeId::of::<$comp>() {
                        return true;
                    }
                )*
                
                false
            }
            
            fn has_all<COMPONENTS: Components>(components: &COMPONENTS, e: Entity) -> Result<(), &'static str> {
                #![allow(unused)] // for the zero-length case.
                $({
                    if components.get::<$comp>().offset_of(e).is_none() {
                        return Err(unsafe { intrinsics::type_name::<$comp>() });
                    }
                })*
                
                Ok(())
            }
        }
    };
}

impl<T: Component> ComponentSet for T {
    #[inline]
    fn contains<C: Component>() -> bool {
        TypeId::of::<C>() == TypeId::of::<T>()
    }

    #[inline]
    fn has_all<C: Components>(components: &C, e: Entity) -> Result<(), &'static str> {
        if components.get::<T>().offset_of(e).is_none() {
            Err(unsafe { intrinsics::type_name::<T>() })
        } else {
            Ok(())
        }
    }
}

// couldn't get a recursive macro to work. Seems like they've tightened the rules?
impl_component_set!(A, B, C, D, E, F, G, H, I, J, K, L,);
impl_component_set!(A, B, C, D, E, F, G, H, I, J, K,);
impl_component_set!(A, B, C, D, E, F, G, H, I, J,);
impl_component_set!(A, B, C, D, E, F, G, H, I,);
impl_component_set!(A, B, C, D, E, F, G, H,);
impl_component_set!(A, B, C, D, E, F, G,);
impl_component_set!(A, B, C, D, E, F,);
impl_component_set!(A, B, C, D, E,);
impl_component_set!(A, B, C, D,);
impl_component_set!(A, B, C,);
impl_component_set!(A, B,);
impl_component_set!(A,);
impl_component_set!();

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    impl Component for A {}

    struct B;
    impl Component for B {}

    struct C;
    impl Component for C {}

    struct D;
    impl Component for D {}

    #[test]
    #[should_panic]
    fn push_twice() {
        let _ = make_empty().push::<A>().push::<A>();
    }
}
