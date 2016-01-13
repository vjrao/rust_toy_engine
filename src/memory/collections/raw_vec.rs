// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
//
// This has been taken from the Rust source by me, Robert Habermeier,
// with modifications to support custom allocators, as per the API
// in the allocator module. Additionally, I have removed some of the
// functionality in an effort to get what I need up and running.


use alloc::heap;

use core::nonzero::NonZero;

use memory::allocator::{Allocator, DefaultAllocator, Kind};
use memory::AllocBox;

use std::cmp;
use std::mem;
use std::ptr::{self, Unique};
use std::ops::Drop;
use std::slice;

/// A low-level utility for more ergonomically allocating, reallocating, and deallocating a
/// a buffer of memory from an allocator without having to worry about all the corner cases
/// involved. This type is excellent for building your own data structures like Vec and VecDeque.
/// In particular:
///
/// * Produces heap::EMPTY on zero-sized types
/// * Produces heap::EMPTY on zero-length allocations
/// * Catches all overflows in capacity computations (promotes them to "capacity overflow" panics)
/// * Guards against 32-bit systems allocating more than isize::MAX bytes
/// * Guards against overflowing your length
/// * Aborts on OOM
/// * Avoids freeing heap::EMPTY
/// * Contains a ptr::Unique and thus endows the user with all related benefits
///
/// This type does not in anyway inspect the memory that it manages. When dropped it *will*
/// free its memory, but it *won't* try to Drop its contents. It is up to the user of RawVec
/// to handle the actual things *stored* inside of a RawVec.
///
/// Note that a RawVec always forces its capacity to be usize::MAX for zero-sized types.
/// This enables you to use capacity growing logic catch the overflows in your length
/// that might occur with zero-sized types.
///
/// However this means that you need to be careful when roundtripping this type
/// with a `Box<[T]>`: `cap()` won't yield the len. However `with_capacity`,
/// `shrink_to_fit`, and `from_box` will actually set RawVec's private capacity
/// field. This allows zero-sized types to not be special-cased by consumers of
/// this type.
#[unsafe_no_drop_flag]
pub struct RawVec<T, A = DefaultAllocator>
    where A: Allocator
{
    ptr: Unique<T>,
    cap: usize,
    alloc: A,
}

impl<T> RawVec<T, DefaultAllocator> {
    /// Creates the biggest possible RawVec without allocating. If T has positive
    /// size, then this makes a RawVec with capacity 0. If T has 0 size, then it
    /// it makes a RawVec with capacity `usize::MAX`. Useful for implementing
    /// delayed allocation.
    pub fn new() -> Self {
        RawVec::with_alloc(DefaultAllocator)
    }

    /// Creates a RawVec with exactly the capacity and alignment requirements
    /// for a `[T; cap]`. This is equivalent to calling RawVec::new when `cap` is 0
    /// or T is zero-sized. Note that if `T` is zero-sized this means you will *not*
    /// get a RawVec with the requested capacity!
    ///
    /// # Panics
    ///
    /// * Panics if the requested capacity exceeds `usize::MAX` bytes.
    /// * Panics on 32-bit platforms if the requested capacity exceeds
    ///   `isize::MAX` bytes.
    ///
    /// # Aborts
    ///
    /// Aborts on OOM
    pub fn with_capacity(cap: usize) -> Self {
        RawVec::with_alloc_and_capacity(DefaultAllocator, cap)
    }
}

impl<T, A: Allocator> RawVec<T, A> {
    /// Identical to `new`, but using a supplied allocator instead of the default.
    pub fn with_alloc(alloc: A) -> Self {
        unsafe {
            // !0 is usize::MAX. This branch should be stripped at compile time.
            let cap = if mem::size_of::<T>() == 0 {
                !0
            } else {
                0
            };

            // heap::EMPTY doubles as "unallocated" and "zero-sized allocation"
            RawVec {
                ptr: Unique::new(heap::EMPTY as *mut T),
                cap: cap,
                alloc: alloc,
            }
        }
    }

    /// Identical to `with_capacity`, but also using a supplied allocator instaed of the default.
    pub fn with_alloc_and_capacity(alloc: A, cap: usize) -> Self {
        let mut alloc = alloc;
        unsafe {
            let ptr = match Kind::array::<T>(cap) {
                Some(kind) => {
                    alloc_guard(*kind.size());
                    match alloc.alloc(kind) {
                        Ok(addr) => *addr,
                        Err(_) => {
                            alloc.oom();
                        }
                    }
                }

                None => {
                    // ZST or cap is 0.
                    heap::EMPTY as *mut u8
                }
            };

            let cap = if mem::size_of::<T>() == 0 {
                !0
            } else {
                cap
            };

            RawVec {
                ptr: Unique::new(ptr as *mut _),
                cap: cap,
                alloc: alloc,
            }
        }
    }
}

impl<T, A: Allocator> RawVec<T, A> {
    /// Gets a raw pointer to the start of the allocation. Note that this is
    /// heap::EMPTY if `cap = 0` or T is zero-sized. In the former case, you must
    /// be careful.
    pub fn ptr(&self) -> *mut T {
        *self.ptr
    }

    /// Gets the capacity of the allocation.
    ///
    /// This will always be `usize::MAX` if `T` is zero-sized.
    pub fn cap(&self) -> usize {
        if mem::size_of::<T>() == 0 {
            !0
        } else {
            self.cap
        }
    }

    /// Doubles the size of the type's backing allocation. This is common enough
    /// to want to do that it's easiest to just have a dedicated method. Slightly
    /// more efficient logic can be provided for this than the general case.
    ///
    /// This function is ideal for when pushing elements one-at-a-time because
    /// you don't need to incur the costs of the more general computations
    /// reserve needs to do to guard against overflow. You do however need to
    /// manually check if your `len == cap`.
    ///
    /// # Panics
    ///
    /// * Panics if T is zero-sized on the assumption that you managed to exhaust
    ///   all `usize::MAX` slots in your imaginary buffer.
    /// * Panics on 32-bit platforms if the requested capacity exceeds
    ///   `isize::MAX` bytes.
    ///
    /// # Aborts
    ///
    /// Aborts on OOM
    ///
    /// # Examples
    ///
    /// ```ignore
    /// struct MyVec<T> {
    ///     buf: RawVec<T>,
    ///     len: usize,
    /// }
    ///
    /// impl<T> MyVec<T> {
    ///     pub fn push(&mut self, elem: T) {
    ///         if self.len == self.buf.cap() { self.buf.double(); }
    ///         // double would have aborted or panicked if the len exceeded
    ///         // `isize::MAX` so this is safe to do unchecked now.
    ///         unsafe {
    ///             ptr::write(self.buf.ptr().offset(self.len as isize), elem);
    ///         }
    ///         self.len += 1;
    ///     }
    /// }
    /// ```
    #[inline(never)]
    #[cold]
    pub fn double(&mut self) {
        unsafe {
            let elem_size = mem::size_of::<T>();

            // since we set the capacity to usize::MAX when elem_size is
            // 0, getting to here necessarily means the RawVec is overfull.
            assert!(elem_size != 0, "capacity overflow");

            if self.cap == 0 {
                // skip to 4 because tiny Vec's are dumb; but not if that would cause overflow
                let new_cap = if elem_size > (!0) / 8 {
                    1
                } else {
                    4
                };

                let kind = Kind::array::<T>(new_cap).expect("capacity overflow");
                alloc_guard(*kind.size());
                match self.alloc.alloc(kind) {
                    Ok(addr) => {
                        self.cap = new_cap;
                        self.ptr = Unique::new(*addr as *mut _);
                    }

                    Err(_) => self.alloc.oom(),
                }
            } else {
                // Since we guarantee that we never allocate more than isize::MAX bytes,
                // `elem_size * self.cap <= isize::MAX` as a precondition, so this can't overflow
                let new_cap = 2 * self.cap;

                // safe to unwrap this since this request worked previously.
                let kind = Kind::array::<T>(self.cap).unwrap();
                alloc_guard(*kind.size());
                let new_kind = Kind::array::<T>(new_cap).expect("capacity overflow");

                match self.alloc.realloc(NonZero::new(*self.ptr as *mut u8), kind, new_kind) {
                    Ok(addr) => {
                        self.cap = new_cap;
                        self.ptr = Unique::new(*addr as *mut _);
                    }

                    Err(_) => {
                        self.alloc.oom();
                    }                             
                }
            };
        }
    }

    /// Ensures that the buffer contains at least enough space to hold
    /// `used_cap + needed_extra_cap` elements. If it doesn't already,
    /// will reallocate the minimum possible amount of memory necessary.
    /// Generally this will be exactly the amount of memory necessary,
    /// but in principle the allocator is free to give back more than
    /// we asked for.
    ///
    /// If `used_cap` exceeds `self.cap()`, this may fail to actually allocate
    /// the requested space. This is not really unsafe, but the unsafe
    /// code *you* write that relies on the behavior of this function may break.
    ///
    /// # Panics
    ///
    /// * Panics if the requested capacity exceeds `usize::MAX` bytes.
    /// * Panics on 32-bit platforms if the requested capacity exceeds
    ///   `isize::MAX` bytes.
    ///
    /// # Aborts
    ///
    /// Aborts on OOM
    pub fn reserve_exact(&mut self, used_cap: usize, needed_extra_cap: usize) {
        unsafe {
            // NOTE: we don't early branch on ZSTs here because we want this
            // to actually catch "asking for more than usize::MAX" in that case.
            // If we make it past the first branch then we are guaranteed to
            // panic.

            // Don't actually need any more capacity.
            // Wrapping in case they gave a bad `used_cap`.
            if self.cap().wrapping_sub(used_cap) >= needed_extra_cap {
                return;
            }

            // Nothing we can really do about these checks :(
            let new_cap = used_cap.checked_add(needed_extra_cap).expect("capacity overflow");
            let new_kind = Kind::array::<T>(new_cap).expect("capacity overflow");
            alloc_guard(*new_kind.size());

            if self.cap == 0 {
                match self.alloc.alloc(new_kind) {
                    Ok(addr) => {
                        self.ptr = Unique::new(*addr as *mut _);
                        self.cap = new_cap;
                    }

                    Err(_) => {
                        self.alloc.oom();
                    }
                }
            } else {
                // safe to unwrap since the creation of this request worked last time.
                let kind = Kind::array::<T>(self.cap()).unwrap();
                match self.alloc.realloc(NonZero::new(*self.ptr as *mut u8), kind, new_kind) {
                    Ok(addr) => {
                        self.ptr = Unique::new(*addr as *mut _);
                        self.cap = new_cap;
                    }

                    Err(_) => {
                        self.alloc.oom();
                    }
                }
            }
        }
    }

    /// Ensures that the buffer contains at least enough space to hold
    /// `used_cap + needed_extra_cap` elements. If it doesn't already have
    /// enough capacity, will reallocate enough space plus comfortable slack
    /// space to get amortized `O(1)` behavior. Will limit this behavior
    /// if it would needlessly cause itself to panic.
    ///
    /// If `used_cap` exceeds `self.cap()`, this may fail to actually allocate
    /// the requested space. This is not really unsafe, but the unsafe
    /// code *you* write that relies on the behavior of this function may break.
    ///
    /// This is ideal for implementing a bulk-push operation like `extend`.
    ///
    /// # Panics
    ///
    /// * Panics if the requested capacity exceeds `usize::MAX` bytes.
    /// * Panics on 32-bit platforms if the requested capacity exceeds
    ///   `isize::MAX` bytes.
    ///
    /// # Aborts
    ///
    /// Aborts on OOM
    ///
    /// # Examples
    ///
    /// ```ignore
    /// struct MyVec<T> {
    ///     buf: RawVec<T>,
    ///     len: usize,
    /// }
    ///
    /// impl<T> MyVec<T> {
    ///     pub fn push_all(&mut self, elems: &[T]) {
    ///         self.buf.reserve(self.len, elems.len());
    ///         // reserve would have aborted or panicked if the len exceeded
    ///         // `isize::MAX` so this is safe to do unchecked now.
    ///         for x in elems {
    ///             unsafe {
    ///                 ptr::write(self.buf.ptr().offset(self.len as isize), x.clone());
    ///             }
    ///             self.len += 1;
    ///         }
    ///     }
    /// }
    /// ```
    pub fn reserve(&mut self, used_cap: usize, needed_extra_cap: usize) {
        unsafe {
            // NOTE: we don't early branch on ZSTs here because we want this
            // to actually catch "asking for more than usize::MAX" in that case.
            // If we make it past the first branch then we are guaranteed to
            // panic.

            // Don't actually need any more capacity.
            // Wrapping in case they give a bad `used_cap`
            if self.cap().wrapping_sub(used_cap) >= needed_extra_cap {
                return;
            }

            // Nothing we can really do about these checks :(
            let required_cap = used_cap.checked_add(needed_extra_cap)
                                       .expect("capacity overflow");

            // Cannot overflow, because `cap <= isize::MAX`, and type of `cap` is `usize`.
            let double_cap = self.cap * 2;

            // `double_cap` guarantees exponential growth.
            let new_cap = cmp::max(double_cap, required_cap);

            let new_kind = Kind::array::<T>(new_cap).expect("capacity overflow");

            // FIXME: may crash and burn on over-reserve
            alloc_guard(*new_kind.size());

            if self.cap == 0 {
                match self.alloc.alloc(new_kind) {
                    Ok(addr) => {
                        self.ptr = Unique::new(*addr as *mut _);
                        self.cap = new_cap;
                    }

                    Err(_) => {
                        self.alloc.oom();
                    }
                }
            } else {
                // safe to unwrap since the creation of this request worked last time.
                let kind = Kind::array::<T>(self.cap()).unwrap();
                match self.alloc.realloc(NonZero::new(*self.ptr as *mut u8), kind, new_kind) {
                    Ok(addr) => {
                        self.ptr = Unique::new(*addr as *mut _);
                        self.cap = new_cap;
                    }

                    Err(_) => {
                        self.alloc.oom();
                    }
                }
            }
        }
    }

    /// Shrinks the allocation down to the specified amount. If the given amount
    /// is 0, actually completely deallocates.
    ///
    /// # Panics
    ///
    /// Panics if the given amount is *larger* than the current capacity.
    ///
    /// # Aborts
    ///
    /// Aborts on OOM.
    pub fn shrink_to_fit(&mut self, amount: usize) {
        let elem_size = mem::size_of::<T>();

        // Set the `cap` because they might be about to promote to a `Box<[T]>`
        if elem_size == 0 {
            self.cap = amount;
            return;
        }

        // This check is my waterloo; it's the only thing Vec wouldn't have to do.
        assert!(self.cap >= amount, "Tried to shrink to a larger capacity");

        // This can be unwrapped since the vector is already this large.
        let kind = Kind::array::<T>(self.cap).unwrap();

        if amount == 0 {
            unsafe {
                let _ = self.alloc.dealloc(NonZero::new(*self.ptr as *mut u8), kind);
                self.ptr = Unique::new(heap::EMPTY as *mut _);
                self.cap = 0;
            }
        } else if self.cap != amount {
            unsafe {
                // Overflow checks are unnecessary as the vector is already at
                // least this large. We have also already branched at
                // elem_size == 0
                let new_kind = Kind::array::<T>(amount).unwrap();

                match self.alloc.realloc(NonZero::new(*self.ptr as *mut u8), kind, new_kind) {
                    Ok(addr) => {
                        self.ptr = Unique::new(*addr as *mut _);
                        self.cap = amount;
                    }

                    Err(_) => {
                        self.alloc.oom();
                    }
                }
            }
        }
    }

    /// Convert this into a boxed slice.
    pub fn into_box(self) -> AllocBox<[T], A> {
        unsafe {
            let slice = slice::from_raw_parts_mut(self.ptr(), self.cap);
            let alloc: A = ptr::read(&self.alloc);
            let output: AllocBox<[T], A> = AllocBox::from_raw_parts(slice, alloc);
            mem::forget(self);

            output
        }
    }

    /// This is a stupid name in the hopes that someone will find this in the
    /// not too distant future and remove it with the rest of
    /// #[unsafe_no_drop_flag]
    pub fn unsafe_no_drop_flag_needs_drop(&self) -> bool {
        self.cap != mem::POST_DROP_USIZE
    }
}

impl<T, A: Allocator + Clone> RawVec<T, A> {
    /// Get a copy of the allocator which this uses.
    pub fn clone_alloc(&self) -> A {
        self.alloc.clone()
    }
}

impl<T, A: Allocator> Drop for RawVec<T, A> {
    #[unsafe_destructor_blind_to_params]
    /// Frees the memory owned by the RawVec *without* trying to Drop its contents.
    fn drop(&mut self) {
        let elem_size = mem::size_of::<T>();
        if elem_size != 0 && self.cap != 0 && self.unsafe_no_drop_flag_needs_drop() {

            // buffer is already at least this large
            let kind = Kind::array::<T>(self.cap).unwrap();
            unsafe {
                let _ = self.alloc.dealloc(NonZero::new(*self.ptr as *mut u8), kind);
            }
        }
    }
}



// We need to guarantee the following:
// * We don't ever allocate `> isize::MAX` byte-size objects
// * We don't overflow `usize::MAX` and actually allocate too little
//
// On 64-bit we just need to check for overflow since trying to allocate
// `> isize::MAX` bytes will surely fail. On 32-bit we need to add an extra
// guard for this in case we're running on a platform which can use all 4GB in
// user-space. e.g. PAE or x32

#[inline]
fn alloc_guard(alloc_size: usize) {
    if cfg!(target_pointer_width = "32") {
        assert!(alloc_size <= ::core::isize::MAX as usize,
                "capacity overflow");
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserve_does_not_overallocate() {
        {
            let mut v: RawVec<u32> = RawVec::new();
            // First `reserve` allocates like `reserve_exact`
            v.reserve(0, 9);
            assert_eq!(9, v.cap());
        }

        {
            let mut v: RawVec<u32> = RawVec::new();
            v.reserve(0, 7);
            assert_eq!(7, v.cap());
            // 97 if more than double of 7, so `reserve` should work
            // like `reserve_exact`.
            v.reserve(7, 90);
            assert_eq!(97, v.cap());
        }

        {
            let mut v: RawVec<u32> = RawVec::new();
            v.reserve(0, 12);
            assert_eq!(12, v.cap());
            v.reserve(12, 3);
            // 3 is less than half of 12, so `reserve` must grow
            // exponentially. At the time of writing this test grow
            // factor is 2, so new capacity is 24, however, grow factor
            // of 1.5 is OK too. Hence `>= 18` in assert.
            assert!(v.cap() >= 12 + 12 / 2);
        }
    }

}
