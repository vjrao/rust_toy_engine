// Copyright 2015 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

//! Double-ended queue which is generic over its allocator. 
//!
//! This is largely a port from the standard library's 
//! (`VecDeque`)[http://doc.rust-lang.org/std/collections/struct.VecDeque.html].
//! Readers of this documentation are advised to read the official documentation for this structure
//! as it is far more in-depth. This lacks a few features of `VecDeque`, which may be added as desired.

use memory::allocator::{Address, Allocator, DefaultAllocator, Kind};

use std::cmp::Ordering;
use std::fmt;
use std::iter::{repeat, FromIterator};
use std::mem;
use std::ops::{Index, IndexMut};
use std::ptr;
use std::slice;
use std::usize;

use std::hash::{Hash, Hasher};
use std::cmp;

use super::raw_vec::RawVec;

const INITIAL_CAPACITY: usize = 7; // 2^3 - 1
const MINIMUM_CAPACITY: usize = 1; // 2 - 1
const MAXIMUM_ZST_CAPACITY: usize = 1 << (usize::BITS - 1); // Largest possible power of two

/// `VecDeque` is a growable ring buffer, which can be used as a double-ended
/// queue efficiently.
///
/// The "default" usage of this type as a queue is to use `push_back` to add to
/// the queue, and `pop_front` to remove from the queue. `extend` and `append`
/// push onto the back in this manner, and iterating over `VecDeque` goes front
/// to back.
pub struct VecDeque<T, A=DefaultAllocator> where A: Allocator {
    // tail and head are pointers into the buffer. Tail always points
    // to the first element that could be read, Head always points
    // to where data should be written.
    // If tail == head the buffer is empty. The length of the ringbuffer
    // is defined as the distance between the two.
    tail: usize,
    head: usize,
    buf: RawVec<T, A>,
}

impl<T, A: Allocator> Drop for VecDeque<T, A> {
    #[unsafe_destructor_blind_to_params]
    fn drop(&mut self) {
        self.clear();
        // RawVec handles deallocation
    }
}

impl<T, A> Default for VecDeque<T, A> where A: Allocator + Default {
    #[inline]
    fn default() -> VecDeque<T, A> {
        VecDeque::with_alloc(Default::default())
    }
}

impl<T, A: Allocator> VecDeque<T, A> {
    /// Marginally more convenient
	#[inline]
    fn ptr(&self) -> *mut T {
        self.buf.ptr()
    }

    /// Marginally more convenient
	#[inline]
    fn cap(&self) -> usize {
        if mem::size_of::<T>() == 0 {
            // For zero sized types, we are always at maximum capacity
            MAXIMUM_ZST_CAPACITY
        } else {
            self.buf.cap()
        }
    }

    /// Turn ptr into a slice
    #[inline]
    unsafe fn buffer_as_slice(&self) -> &[T] {
        slice::from_raw_parts(self.ptr(), self.cap())
    }

    /// Turn ptr into a mut slice
    #[inline]
    unsafe fn buffer_as_mut_slice(&mut self) -> &mut [T] {
        slice::from_raw_parts_mut(self.ptr(), self.cap())
    }

    /// Moves an element out of the buffer
    #[inline]
    unsafe fn buffer_read(&mut self, off: usize) -> T {
        ptr::read(self.ptr().offset(off as isize))
    }

    /// Writes an element into the buffer, moving it.
    #[inline]
    unsafe fn buffer_write(&mut self, off: usize, value: T) {
        ptr::write(self.ptr().offset(off as isize), value);
    }

    /// Returns true if and only if the buffer is at capacity
    #[inline]
    fn is_full(&self) -> bool {
        self.cap() - self.len() == 1
    }

    /// Returns the index in the underlying buffer for a given logical element
    /// index.
    #[inline]
    fn wrap_index(&self, idx: usize) -> usize {
        wrap_index(idx, self.cap())
    }

    /// Returns the index in the underlying buffer for a given logical element
    /// index + addend.
    #[inline]
    fn wrap_add(&self, idx: usize, addend: usize) -> usize {
        wrap_index(idx.wrapping_add(addend), self.cap())
    }

    /// Returns the index in the underlying buffer for a given logical element
    /// index - subtrahend.
    #[inline]
    fn wrap_sub(&self, idx: usize, subtrahend: usize) -> usize {
        wrap_index(idx.wrapping_sub(subtrahend), self.cap())
    }

    /// Copies a contiguous block of memory len long from src to dst
    #[inline]
    unsafe fn copy(&self, dst: usize, src: usize, len: usize) {
        debug_assert!(dst + len <= self.cap(),
                      "cpy dst={} src={} len={} cap={}",
                      dst,
                      src,
                      len,
                      self.cap());
        debug_assert!(src + len <= self.cap(),
                      "cpy dst={} src={} len={} cap={}",
                      dst,
                      src,
                      len,
                      self.cap());
        ptr::copy(self.ptr().offset(src as isize),
                  self.ptr().offset(dst as isize),
                  len);
    }

    /// Copies a contiguous block of memory len long from src to dst
    #[inline]
    unsafe fn copy_nonoverlapping(&self, dst: usize, src: usize, len: usize) {
        debug_assert!(dst + len <= self.cap(),
                      "cno dst={} src={} len={} cap={}",
                      dst,
                      src,
                      len,
                      self.cap());
        debug_assert!(src + len <= self.cap(),
                      "cno dst={} src={} len={} cap={}",
                      dst,
                      src,
                      len,
                      self.cap());
        ptr::copy_nonoverlapping(self.ptr().offset(src as isize),
                                 self.ptr().offset(dst as isize),
                                 len);
    }

    /// Copies a potentially wrapping block of memory len long from src to dest.
    /// (abs(dst - src) + len) must be no larger than cap() (There must be at
    /// most one continuous overlapping region between src and dest).
    unsafe fn wrap_copy(&self, dst: usize, src: usize, len: usize) {
        #[allow(dead_code)]
        fn diff(a: usize, b: usize) -> usize {
            if a <= b {
                b - a
            } else {
                a - b
            }
        }
        debug_assert!(cmp::min(diff(dst, src), self.cap() - diff(dst, src)) + len <= self.cap(),
                      "wrc dst={} src={} len={} cap={}",
                      dst,
                      src,
                      len,
                      self.cap());

        if src == dst || len == 0 {
            return;
        }

        let dst_after_src = self.wrap_sub(dst, src) < len;

        let src_pre_wrap_len = self.cap() - src;
        let dst_pre_wrap_len = self.cap() - dst;
        let src_wraps = src_pre_wrap_len < len;
        let dst_wraps = dst_pre_wrap_len < len;

        match (dst_after_src, src_wraps, dst_wraps) {
            (_, false, false) => {
                // src doesn't wrap, dst doesn't wrap
                //
                //        S . . .
                // 1 [_ _ A A B B C C _]
                // 2 [_ _ A A A A B B _]
                //            D . . .
                //
                self.copy(dst, src, len);
            }
            (false, false, true) => {
                // dst before src, src doesn't wrap, dst wraps
                //
                //    S . . .
                // 1 [A A B B _ _ _ C C]
                // 2 [A A B B _ _ _ A A]
                // 3 [B B B B _ _ _ A A]
                //    . .           D .
                //
                self.copy(dst, src, dst_pre_wrap_len);
                self.copy(0, src + dst_pre_wrap_len, len - dst_pre_wrap_len);
            }
            (true, false, true) => {
                // src before dst, src doesn't wrap, dst wraps
                //
                //              S . . .
                // 1 [C C _ _ _ A A B B]
                // 2 [B B _ _ _ A A B B]
                // 3 [B B _ _ _ A A A A]
                //    . .           D .
                //
                self.copy(0, src + dst_pre_wrap_len, len - dst_pre_wrap_len);
                self.copy(dst, src, dst_pre_wrap_len);
            }
            (false, true, false) => {
                // dst before src, src wraps, dst doesn't wrap
                //
                //    . .           S .
                // 1 [C C _ _ _ A A B B]
                // 2 [C C _ _ _ B B B B]
                // 3 [C C _ _ _ B B C C]
                //              D . . .
                //
                self.copy(dst, src, src_pre_wrap_len);
                self.copy(dst + src_pre_wrap_len, 0, len - src_pre_wrap_len);
            }
            (true, true, false) => {
                // src before dst, src wraps, dst doesn't wrap
                //
                //    . .           S .
                // 1 [A A B B _ _ _ C C]
                // 2 [A A A A _ _ _ C C]
                // 3 [C C A A _ _ _ C C]
                //    D . . .
                //
                self.copy(dst + src_pre_wrap_len, 0, len - src_pre_wrap_len);
                self.copy(dst, src, src_pre_wrap_len);
            }
            (false, true, true) => {
                // dst before src, src wraps, dst wraps
                //
                //    . . .         S .
                // 1 [A B C D _ E F G H]
                // 2 [A B C D _ E G H H]
                // 3 [A B C D _ E G H A]
                // 4 [B C C D _ E G H A]
                //    . .         D . .
                //
                debug_assert!(dst_pre_wrap_len > src_pre_wrap_len);
                let delta = dst_pre_wrap_len - src_pre_wrap_len;
                self.copy(dst, src, src_pre_wrap_len);
                self.copy(dst + src_pre_wrap_len, 0, delta);
                self.copy(0, delta, len - dst_pre_wrap_len);
            }
            (true, true, true) => {
                // src before dst, src wraps, dst wraps
                //
                //    . .         S . .
                // 1 [A B C D _ E F G H]
                // 2 [A A B D _ E F G H]
                // 3 [H A B D _ E F G H]
                // 4 [H A B D _ E F F G]
                //    . . .         D .
                //
                debug_assert!(src_pre_wrap_len > dst_pre_wrap_len);
                let delta = src_pre_wrap_len - dst_pre_wrap_len;
                self.copy(delta, 0, len - src_pre_wrap_len);
                self.copy(0, self.cap() - delta, delta);
                self.copy(dst, src, dst_pre_wrap_len);
            }
        }
    }

    /// Frobs the head and tail sections around to handle the fact that we
    /// just reallocated. Unsafe because it trusts old_cap.
    #[inline]
    unsafe fn handle_cap_increase(&mut self, old_cap: usize) {
        let new_cap = self.cap();

        // Move the shortest contiguous section of the ring buffer
        //    T             H
        //   [o o o o o o o . ]
        //    T             H
        // A [o o o o o o o . . . . . . . . . ]
        //        H T
        //   [o o . o o o o o ]
        //          T             H
        // B [. . . o o o o o o o . . . . . . ]
        //              H T
        //   [o o o o o . o o ]
        //              H                 T
        // C [o o o o o . . . . . . . . . o o ]

        if self.tail <= self.head {
            // A
            // Nop
        } else if self.head < old_cap - self.tail {
            // B
            self.copy_nonoverlapping(old_cap, 0, self.head);
            self.head += old_cap;
            debug_assert!(self.head > self.tail);
        } else {
            // C
            let new_tail = new_cap - (old_cap - self.tail);
            self.copy_nonoverlapping(new_tail, self.tail, old_cap - self.tail);
            self.tail = new_tail;
            debug_assert!(self.head < self.tail);
        }
        debug_assert!(self.head < self.cap());
        debug_assert!(self.tail < self.cap());
        debug_assert!(self.cap().count_ones() == 1);
    }
}

impl<T> VecDeque<T, DefaultAllocator> {
    /// Creates an empty `VecDeque`.
    pub fn new() -> Self {
        VecDeque::with_alloc(DefaultAllocator)
    }

    /// Creates an empty `VecDeque` with space for at least `n` elements.
    pub fn with_capacity(n: usize) -> Self {
		VecDeque::with_alloc_and_capacity(DefaultAllocator, n)
    }
	
}

impl<T, A: Allocator> VecDeque<T, A> {
	/// Create an empty `VecDeque` backed by the given allocator.
	pub fn with_alloc(alloc: A) -> Self {
		VecDeque::with_alloc_and_capacity(alloc, INITIAL_CAPACITY)
	}
	
	/// Create an empty `VecDeque` with space for at least `n` elements, 
	/// backed by the given allocator.
	pub fn with_alloc_and_capacity(alloc: A, n: usize) -> Self {
		// +1 since the ringbuffer always leaves one space empty
        let cap = cmp::max(n + 1, MINIMUM_CAPACITY + 1).next_power_of_two();
        assert!(cap > n, "capacity overflow");

        VecDeque {
            tail: 0,
            head: 0,
            buf: RawVec::with_alloc_and_capacity(alloc, cap),
        }
	}

    /// Retrieves an element in the `VecDeque` by index.
    pub fn get(&self, index: usize) -> Option<&T> {
        if index < self.len() {
            let idx = self.wrap_add(self.tail, index);
            unsafe { Some(&*self.ptr().offset(idx as isize)) }
        } else {
            None
        }
    }

    /// Retrieves an element in the `VecDeque` mutably by index.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index < self.len() {
            let idx = self.wrap_add(self.tail, index);
            unsafe { Some(&mut *self.ptr().offset(idx as isize)) }
        } else {
            None
        }
    }

    /// Swaps elements at indices `i` and `j`.
    ///
    /// `i` and `j` may be equal.
    ///
    /// Fails if there is no element with either index.
    pub fn swap(&mut self, i: usize, j: usize) {
        assert!(i < self.len());
        assert!(j < self.len());
        let ri = self.wrap_add(self.tail, i);
        let rj = self.wrap_add(self.tail, j);
        unsafe {
            ptr::swap(self.ptr().offset(ri as isize),
                      self.ptr().offset(rj as isize))
        }
    }

    /// Returns the number of elements the `VecDeque` can hold without
    /// reallocating.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.cap() - 1
    }

    /// Reserves the minimum capacity for exactly `additional` more elements to be inserted in the
    /// given `VecDeque`. Does nothing if the capacity is already sufficient.
    ///
    /// Note that the allocator may give the collection more space than it requests. Therefore
    /// capacity can not be relied upon to be precisely minimal. Prefer `reserve` if future
    /// insertions are expected.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity overflows `usize` or the allocator cannot support the request.
    pub fn reserve_exact(&mut self, additional: usize) {
        self.reserve(additional);
    }

    /// Reserves capacity for at least `additional` more elements to be inserted in the given
    /// `VecDeque`. The collection may reserve more space to avoid frequent reallocations.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity overflows `usize` or the allocator cannot support the request.
    pub fn reserve(&mut self, additional: usize) {
        let old_cap = self.cap();
        let used_cap = self.len() + 1;
        let new_cap = used_cap.checked_add(additional)
                              .and_then(|needed_cap| needed_cap.checked_next_power_of_two())
                              .expect("capacity overflow");

        if new_cap > self.capacity() {
            self.buf.reserve_exact(used_cap, new_cap - used_cap);
            unsafe {
                self.handle_cap_increase(old_cap);
            }
        }
    }

    /// Shrinks the capacity of the `VecDeque` as much as possible.
    ///
    /// It will drop down as close as possible to the length but the allocator may still inform the
    /// `VecDeque` that there is space for a few more elements.
    pub fn shrink_to_fit(&mut self) {
        // +1 since the ringbuffer always leaves one space empty
        // len + 1 can't overflow for an existing, well-formed ringbuffer.
        let target_cap = cmp::max(self.len() + 1, MINIMUM_CAPACITY + 1).next_power_of_two();
        if target_cap < self.cap() {
            // There are three cases of interest:
            //   All elements are out of desired bounds
            //   Elements are contiguous, and head is out of desired bounds
            //   Elements are discontiguous, and tail is out of desired bounds
            //
            // At all other times, element positions are unaffected.
            //
            // Indicates that elements at the head should be moved.
            let head_outside = self.head == 0 || self.head >= target_cap;
            // Move elements from out of desired bounds (positions after target_cap)
            if self.tail >= target_cap && head_outside {
                //                    T             H
                //   [. . . . . . . . o o o o o o o . ]
                //    T             H
                //   [o o o o o o o . ]
                unsafe {
                    self.copy_nonoverlapping(0, self.tail, self.len());
                }
                self.head = self.len();
                self.tail = 0;
            } else if self.tail != 0 && self.tail < target_cap && head_outside {
                //          T             H
                //   [. . . o o o o o o o . . . . . . ]
                //        H T
                //   [o o . o o o o o ]
                let len = self.wrap_sub(self.head, target_cap);
                unsafe {
                    self.copy_nonoverlapping(0, target_cap, len);
                }
                self.head = len;
                debug_assert!(self.head < self.tail);
            } else if self.tail >= target_cap {
                //              H                 T
                //   [o o o o o . . . . . . . . . o o ]
                //              H T
                //   [o o o o o . o o ]
                debug_assert!(self.wrap_sub(self.head, 1) < target_cap);
                let len = self.cap() - self.tail;
                let new_tail = target_cap - len;
                unsafe {
                    self.copy_nonoverlapping(new_tail, self.tail, len);
                }
                self.tail = new_tail;
                debug_assert!(self.head < self.tail);
            }

            self.buf.shrink_to_fit(target_cap);

            debug_assert!(self.head < self.cap());
            debug_assert!(self.tail < self.cap());
            debug_assert!(self.cap().count_ones() == 1);
        }
    }
    
    /// Shortens a `VecDeque`, dropping excess elements from the back.
    ///
    /// If `len` is greater than the `VecDeque`'s current length, this has no
    /// effect.
    pub fn truncate(&mut self, len: usize) {
        for _ in len..self.len() {
            self.pop_back();
        }
    }

    /// Returns a front-to-back iterator.
    pub fn iter(&self) -> Iter<T> {
        Iter {
            tail: self.tail,
            head: self.head,
            ring: unsafe { self.buffer_as_slice() },
        }
    }

    /// Returns a front-to-back iterator that returns mutable references.
    pub fn iter_mut(&mut self) -> IterMut<T> {
        IterMut {
            tail: self.tail,
            head: self.head,
            ring: unsafe { self.buffer_as_mut_slice() },
        }
    }

    /// Returns a pair of slices which contain, in order, the contents of the
    /// `VecDeque`.
    #[inline]
    pub fn as_slices(&self) -> (&[T], &[T]) {
        unsafe {
            let contiguous = self.is_contiguous();
            let buf = self.buffer_as_slice();
            if contiguous {
                let (empty, buf) = buf.split_at(0);
                (&buf[self.tail..self.head], empty)
            } else {
                let (mid, right) = buf.split_at(self.tail);
                let (left, _) = mid.split_at(self.head);
                (right, left)
            }
        }
    }

    /// Returns a pair of slices which contain, in order, the contents of the
    /// `VecDeque`.
    #[inline]
    pub fn as_mut_slices(&mut self) -> (&mut [T], &mut [T]) {
        unsafe {
            let contiguous = self.is_contiguous();
            let head = self.head;
            let tail = self.tail;
            let buf = self.buffer_as_mut_slice();

            if contiguous {
                let (empty, buf) = buf.split_at_mut(0);
                (&mut buf[tail..head], empty)
            } else {
                let (mid, right) = buf.split_at_mut(tail);
                let (left, _) = mid.split_at_mut(head);

                (right, left)
            }
        }
    }

    /// Returns the number of elements in the `VecDeque`.
    pub fn len(&self) -> usize {
        count(self.tail, self.head, self.cap())
    }

    /// Returns true if the buffer contains no elements
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clears the buffer, removing all values.
    #[inline]
    pub fn clear(&mut self) {
        while let Some(x) = self.pop_front() {
            drop(x)
        }
    }

    /// Provides a reference to the front element, or `None` if the sequence is
    /// empty.
    pub fn front(&self) -> Option<&T> {
        if !self.is_empty() {
            Some(&self[0])
        } else {
            None
        }
    }

    /// Provides a mutable reference to the front element, or `None` if the
    /// sequence is empty.
    pub fn front_mut(&mut self) -> Option<&mut T> {
        if !self.is_empty() {
            Some(&mut self[0])
        } else {
            None
        }
    }

    /// Provides a reference to the back element, or `None` if the sequence is
    /// empty.
    pub fn back(&self) -> Option<&T> {
        if !self.is_empty() {
            Some(&self[self.len() - 1])
        } else {
            None
        }
    }

    /// Provides a mutable reference to the back element, or `None` if the
    /// sequence is empty.
    pub fn back_mut(&mut self) -> Option<&mut T> {
        let len = self.len();
        if !self.is_empty() {
            Some(&mut self[len - 1])
        } else {
            None
        }
    }

    /// Removes the first element and returns it, or `None` if the sequence is
    /// empty.
    pub fn pop_front(&mut self) -> Option<T> {
        if self.is_empty() {
            None
        } else {
            let tail = self.tail;
            self.tail = self.wrap_add(self.tail, 1);
            unsafe { Some(self.buffer_read(tail)) }
        }
    }

    /// Inserts an element first in the sequence.
    pub fn push_front(&mut self, value: T) {
        if self.is_full() {
            let old_cap = self.cap();
            self.buf.double();
            unsafe {
                self.handle_cap_increase(old_cap);
            }
            debug_assert!(!self.is_full());
        }

        self.tail = self.wrap_sub(self.tail, 1);
        let tail = self.tail;
        unsafe {
            self.buffer_write(tail, value);
        }
    }

    /// Appends an element to the back of a buffer
    ///
    /// # Panics
    /// Panics if the allocator runs out of memory.
    pub fn push_back(&mut self, value: T) {
        if self.is_full() {
            let old_cap = self.cap();
            self.buf.double();
            unsafe {
                self.handle_cap_increase(old_cap);
            }
            debug_assert!(!self.is_full());
        }

        let head = self.head;
        self.head = self.wrap_add(self.head, 1);
        unsafe { self.buffer_write(head, value) }
    }

    /// Removes the last element from a buffer and returns it, or `None` if
    /// it is empty.
    pub fn pop_back(&mut self) -> Option<T> {
        if self.is_empty() {
            None
        } else {
            self.head = self.wrap_sub(self.head, 1);
            let head = self.head;
            unsafe { Some(self.buffer_read(head)) }
        }
    }

    #[inline]
    fn is_contiguous(&self) -> bool {
        self.tail <= self.head
    }

    /// Removes an element from anywhere in the `VecDeque` and returns it, replacing it with the
    /// last element.
    ///
    /// This does not preserve ordering, but is O(1).
    ///
    /// Returns `None` if `index` is out of bounds.
    pub fn swap_remove_back(&mut self, index: usize) -> Option<T> {
        let length = self.len();
        if length > 0 && index < length - 1 {
            self.swap(index, length - 1);
        } else if index >= length {
            return None;
        }
        self.pop_back()
    }

    /// Removes an element from anywhere in the `VecDeque` and returns it,
    /// replacing it with the first element.
    ///
    /// This does not preserve ordering, but is O(1).
    ///
    /// Returns `None` if `index` is out of bounds.
    pub fn swap_remove_front(&mut self, index: usize) -> Option<T> {
        let length = self.len();
        if length > 0 && index < length && index != 0 {
            self.swap(index, 0);
        } else if index >= length {
            return None;
        }
        self.pop_front()
    }

    /// Inserts an element at `index` within the `VecDeque`. Whichever
    /// end is closer to the insertion point will be moved to make room,
    /// and all the affected elements will be moved to new positions.
    ///
    /// # Panics
    ///
    /// Panics if `index` is greater than `VecDeque`'s length
    /// or if the allocator runs out of memory.
    pub fn insert(&mut self, index: usize, value: T) {
        assert!(index <= self.len(), "index out of bounds");
        if self.is_full() {
            let old_cap = self.cap();
            self.buf.double();
            unsafe {
                self.handle_cap_increase(old_cap);
            }
            debug_assert!(!self.is_full());
        }

        // Move the least number of elements in the ring buffer and insert
        // the given object
        //
        // At most len/2 - 1 elements will be moved. O(min(n, n-i))
        //
        // There are three main cases:
        //  Elements are contiguous
        //      - special case when tail is 0
        //  Elements are discontiguous and the insert is in the tail section
        //  Elements are discontiguous and the insert is in the head section
        //
        // For each of those there are two more cases:
        //  Insert is closer to tail
        //  Insert is closer to head
        //
        // Key: H - self.head
        //      T - self.tail
        //      o - Valid element
        //      I - Insertion element
        //      A - The element that should be after the insertion point
        //      M - Indicates element was moved

        let idx = self.wrap_add(self.tail, index);

        let distance_to_tail = index;
        let distance_to_head = self.len() - index;

        let contiguous = self.is_contiguous();

        match (contiguous,
               distance_to_tail <= distance_to_head,
               idx >= self.tail) {
            (true, true, _) if index == 0 => {
                // push_front
                //
                //       T
                //       I             H
                //      [A o o o o o o . . . . . . . . .]
                //
                //                       H         T
                //      [A o o o o o o o . . . . . I]
                //

                self.tail = self.wrap_sub(self.tail, 1);
            }
            (true, true, _) => {
                unsafe {
                    // contiguous, insert closer to tail:
                    //
                    //             T   I         H
                    //      [. . . o o A o o o o . . . . . .]
                    //
                    //           T               H
                    //      [. . o o I A o o o o . . . . . .]
                    //           M M
                    //
                    // contiguous, insert closer to tail and tail is 0:
                    //
                    //
                    //       T   I         H
                    //      [o o A o o o o . . . . . . . . .]
                    //
                    //                       H             T
                    //      [o I A o o o o o . . . . . . . o]
                    //       M                             M

                    let new_tail = self.wrap_sub(self.tail, 1);

                    self.copy(new_tail, self.tail, 1);
                    // Already moved the tail, so we only copy `index - 1` elements.
                    self.copy(self.tail, self.tail + 1, index - 1);

                    self.tail = new_tail;
                }
            }
            (true, false, _) => {
                unsafe {
                    //  contiguous, insert closer to head:
                    //
                    //             T       I     H
                    //      [. . . o o o o A o o . . . . . .]
                    //
                    //             T               H
                    //      [. . . o o o o I A o o . . . . .]
                    //                       M M M

                    self.copy(idx + 1, idx, self.head - idx);
                    self.head = self.wrap_add(self.head, 1);
                }
            }
            (false, true, true) => {
                unsafe {
                    // discontiguous, insert closer to tail, tail section:
                    //
                    //                   H         T   I
                    //      [o o o o o o . . . . . o o A o o]
                    //
                    //                   H       T
                    //      [o o o o o o . . . . o o I A o o]
                    //                           M M

                    self.copy(self.tail - 1, self.tail, index);
                    self.tail -= 1;
                }
            }
            (false, false, true) => {
                unsafe {
                    // discontiguous, insert closer to head, tail section:
                    //
                    //           H             T         I
                    //      [o o . . . . . . . o o o o o A o]
                    //
                    //             H           T
                    //      [o o o . . . . . . o o o o o I A]
                    //       M M M                         M

                    // copy elements up to new head
                    self.copy(1, 0, self.head);

                    // copy last element into empty spot at bottom of buffer
                    self.copy(0, self.cap() - 1, 1);

                    // move elements from idx to end forward not including ^ element
                    self.copy(idx + 1, idx, self.cap() - 1 - idx);

                    self.head += 1;
                }
            }
            (false, true, false) if idx == 0 => {
                unsafe {
                    // discontiguous, insert is closer to tail, head section,
                    // and is at index zero in the internal buffer:
                    //
                    //       I                   H     T
                    //      [A o o o o o o o o o . . . o o o]
                    //
                    //                           H   T
                    //      [A o o o o o o o o o . . o o o I]
                    //                               M M M

                    // copy elements up to new tail
                    self.copy(self.tail - 1, self.tail, self.cap() - self.tail);

                    // copy last element into empty spot at bottom of buffer
                    self.copy(self.cap() - 1, 0, 1);

                    self.tail -= 1;
                }
            }
            (false, true, false) => {
                unsafe {
                    // discontiguous, insert closer to tail, head section:
                    //
                    //             I             H     T
                    //      [o o o A o o o o o o . . . o o o]
                    //
                    //                           H   T
                    //      [o o I A o o o o o o . . o o o o]
                    //       M M                     M M M M

                    // copy elements up to new tail
                    self.copy(self.tail - 1, self.tail, self.cap() - self.tail);

                    // copy last element into empty spot at bottom of buffer
                    self.copy(self.cap() - 1, 0, 1);

                    // move elements from idx-1 to end forward not including ^ element
                    self.copy(0, 1, idx - 1);

                    self.tail -= 1;
                }
            }
            (false, false, false) => {
                unsafe {
                    // discontiguous, insert closer to head, head section:
                    //
                    //               I     H           T
                    //      [o o o o A o o . . . . . . o o o]
                    //
                    //                     H           T
                    //      [o o o o I A o o . . . . . o o o]
                    //                 M M M

                    self.copy(idx + 1, idx, self.head - idx);
                    self.head += 1;
                }
            }
        }

        // tail might've been changed so we need to recalculate
        let new_idx = self.wrap_add(self.tail, index);
        unsafe {
            self.buffer_write(new_idx, value);
        }
    }

    /// Removes and returns the element at `index` from the `VecDeque`.
    /// Whichever end is closer to the removal point will be moved to make
    /// room, and all the affected elements will be moved to new positions.
    /// Returns `None` if `index` is out of bounds.
    pub fn remove(&mut self, index: usize) -> Option<T> {
        if self.is_empty() || self.len() <= index {
            return None;
        }

        // There are three main cases:
        //  Elements are contiguous
        //  Elements are discontiguous and the removal is in the tail section
        //  Elements are discontiguous and the removal is in the head section
        //      - special case when elements are technically contiguous,
        //        but self.head = 0
        //
        // For each of those there are two more cases:
        //  Insert is closer to tail
        //  Insert is closer to head
        //
        // Key: H - self.head
        //      T - self.tail
        //      o - Valid element
        //      x - Element marked for removal
        //      R - Indicates element that is being removed
        //      M - Indicates element was moved

        let idx = self.wrap_add(self.tail, index);

        let elem = unsafe { Some(self.buffer_read(idx)) };

        let distance_to_tail = index;
        let distance_to_head = self.len() - index;

        let contiguous = self.is_contiguous();

        match (contiguous,
               distance_to_tail <= distance_to_head,
               idx >= self.tail) {
            (true, true, _) => {
                unsafe {
                    // contiguous, remove closer to tail:
                    //
                    //             T   R         H
                    //      [. . . o o x o o o o . . . . . .]
                    //
                    //               T           H
                    //      [. . . . o o o o o o . . . . . .]
                    //               M M

                    self.copy(self.tail + 1, self.tail, index);
                    self.tail += 1;
                }
            }
            (true, false, _) => {
                unsafe {
                    // contiguous, remove closer to head:
                    //
                    //             T       R     H
                    //      [. . . o o o o x o o . . . . . .]
                    //
                    //             T           H
                    //      [. . . o o o o o o . . . . . . .]
                    //                     M M

                    self.copy(idx, idx + 1, self.head - idx - 1);
                    self.head -= 1;
                }
            }
            (false, true, true) => {
                unsafe {
                    // discontiguous, remove closer to tail, tail section:
                    //
                    //                   H         T   R
                    //      [o o o o o o . . . . . o o x o o]
                    //
                    //                   H           T
                    //      [o o o o o o . . . . . . o o o o]
                    //                               M M

                    self.copy(self.tail + 1, self.tail, index);
                    self.tail = self.wrap_add(self.tail, 1);
                }
            }
            (false, false, false) => {
                unsafe {
                    // discontiguous, remove closer to head, head section:
                    //
                    //               R     H           T
                    //      [o o o o x o o . . . . . . o o o]
                    //
                    //                   H             T
                    //      [o o o o o o . . . . . . . o o o]
                    //               M M

                    self.copy(idx, idx + 1, self.head - idx - 1);
                    self.head -= 1;
                }
            }
            (false, false, true) => {
                unsafe {
                    // discontiguous, remove closer to head, tail section:
                    //
                    //             H           T         R
                    //      [o o o . . . . . . o o o o o x o]
                    //
                    //           H             T
                    //      [o o . . . . . . . o o o o o o o]
                    //       M M                         M M
                    //
                    // or quasi-discontiguous, remove next to head, tail section:
                    //
                    //       H                 T         R
                    //      [. . . . . . . . . o o o o o x o]
                    //
                    //                         T           H
                    //      [. . . . . . . . . o o o o o o .]
                    //                                   M

                    // draw in elements in the tail section
                    self.copy(idx, idx + 1, self.cap() - idx - 1);

                    // Prevents underflow.
                    if self.head != 0 {
                        // copy first element into empty spot
                        self.copy(self.cap() - 1, 0, 1);

                        // move elements in the head section backwards
                        self.copy(0, 1, self.head - 1);
                    }

                    self.head = self.wrap_sub(self.head, 1);
                }
            }
            (false, true, false) => {
                unsafe {
                    // discontiguous, remove closer to tail, head section:
                    //
                    //           R               H     T
                    //      [o o x o o o o o o o . . . o o o]
                    //
                    //                           H       T
                    //      [o o o o o o o o o o . . . . o o]
                    //       M M M                       M M

                    // draw in elements up to idx
                    self.copy(1, 0, idx);

                    // copy last element into empty spot
                    self.copy(0, self.cap() - 1, 1);

                    // move elements from tail to end forward, excluding the last one
                    self.copy(self.tail + 1, self.tail, self.cap() - self.tail - 1);

                    self.tail = self.wrap_add(self.tail, 1);
                }
            }
        }

        return elem;
    }

    /// Retains only the elements specified by the predicate.
    ///
    /// In other words, remove all elements `e` such that `f(&e)` returns false.
    /// This method operates in place and preserves the order of the retained
    /// elements.
    pub fn retain<F>(&mut self, mut f: F)
        where F: FnMut(&T) -> bool
    {
        let len = self.len();
        let mut del = 0;
        for i in 0..len {
            if !f(&self[i]) {
                del += 1;
            } else if del > 0 {
                self.swap(i - del, i);
            }
        }
        if del > 0 {
            self.truncate(len - del);
        }
    }
}

/// Returns the index in the underlying buffer for a given logical element index.
#[inline]
fn wrap_index(index: usize, size: usize) -> usize {
    // size is always a power of 2
    debug_assert!(size.is_power_of_two());
    index & (size - 1)
}

/// Calculate the number of elements left to be read in the buffer
#[inline]
fn count(tail: usize, head: usize, size: usize) -> usize {
    // size is always a power of 2
    (head.wrapping_sub(tail)) & (size - 1)
}

/// `VecDeque` iterator.
#[derive(Clone)]
pub struct Iter<'a, T: 'a> {
    ring: &'a [T],
    tail: usize,
    head: usize,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<&'a T> {
        if self.tail == self.head {
            return None;
        }
        let tail = self.tail;
        self.tail = wrap_index(self.tail.wrapping_add(1), self.ring.len());
        unsafe { Some(self.ring.get_unchecked(tail)) }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = count(self.tail, self.head, self.ring.len());
        (len, Some(len))
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a T> {
        if self.tail == self.head {
            return None;
        }
        self.head = wrap_index(self.head.wrapping_sub(1), self.ring.len());
        unsafe { Some(self.ring.get_unchecked(self.head)) }
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {}

/// `VecDeque` mutable iterator.
pub struct IterMut<'a, T: 'a> {
    ring: &'a mut [T],
    tail: usize,
    head: usize,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;

    #[inline]
    fn next(&mut self) -> Option<&'a mut T> {
        if self.tail == self.head {
            return None;
        }
        let tail = self.tail;
        self.tail = wrap_index(self.tail.wrapping_add(1), self.ring.len());

        unsafe {
            let elem = self.ring.get_unchecked_mut(tail);
            Some(&mut *(elem as *mut _))
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = count(self.tail, self.head, self.ring.len());
        (len, Some(len))
    }
}

impl<'a, T> DoubleEndedIterator for IterMut<'a, T> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a mut T> {
        if self.tail == self.head {
            return None;
        }
        self.head = wrap_index(self.head.wrapping_sub(1), self.ring.len());

        unsafe {
            let elem = self.ring.get_unchecked_mut(self.head);
            Some(&mut *(elem as *mut _))
        }
    }
}

impl<'a, T> ExactSizeIterator for IterMut<'a, T> {}

/// A by-value VecDeque iterator
#[derive(Clone)]
pub struct IntoIter<T, A=DefaultAllocator> where A: Allocator {
    inner: VecDeque<T, A>,
}

impl<T, A: Allocator> Iterator for IntoIter<T, A> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        self.inner.pop_front()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.inner.len();
        (len, Some(len))
    }
}

impl<T, A: Allocator> DoubleEndedIterator for IntoIter<T, A> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        self.inner.pop_back()
    }
}

impl<T, A: Allocator> ExactSizeIterator for IntoIter<T, A> {}

impl<T, A> Clone for VecDeque<T, A> where T: Clone, A: Allocator + Clone {
    fn clone(&self) -> Self {
        let mut v = VecDeque::with_alloc_and_capacity(self.buf.clone_alloc(), self.len());
        for x in self.iter() {
            v.push_back(x.clone());
        }
        
        v
    }
}

// FIXME: Should the allocator be equal too?
impl<T: PartialEq, A: Allocator> PartialEq for VecDeque<T, A> {
    fn eq(&self, other: &VecDeque<T, A>) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.eq(b))
    }
}

impl<T: Eq, A: Allocator> Eq for VecDeque<T, A> {}

impl<T: PartialOrd, A: Allocator> PartialOrd for VecDeque<T, A> {
    fn partial_cmp(&self, other: &VecDeque<T, A>) -> Option<Ordering> {
        self.iter().partial_cmp(other.iter())
    }
}

impl<T: Ord, A: Allocator> Ord for VecDeque<T, A> {
    #[inline]
    fn cmp(&self, other: &VecDeque<T, A>) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<T: Hash, A: Allocator> Hash for VecDeque<T, A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.len().hash(state);
        for elt in self {
            elt.hash(state);
        }
    }
}

impl<T, A: Allocator> Index<usize> for VecDeque<T, A> {
    type Output = T;

    #[inline]
    fn index(&self, index: usize) -> &T {
        self.get(index).expect("Out of bounds access")
    }
}

impl<T, A: Allocator> IndexMut<usize> for VecDeque<T, A> {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut T {
        self.get_mut(index).expect("Out of bounds access")
    }
}

impl<T, A: Allocator + Default> FromIterator<T> for VecDeque<T, A> {
    fn from_iter<I: IntoIterator<Item = T>>(iterable: I) -> Self {
        let iterator = iterable.into_iter();
        let (lower, _) = iterator.size_hint();
        let mut deq = VecDeque::with_alloc_and_capacity(Default::default(), lower);
        deq.extend(iterator);
        deq
    }
}

impl<T, A: Allocator> IntoIterator for VecDeque<T, A> {
    type Item = T;
    type IntoIter = IntoIter<T, A>;

    /// Consumes the list into a front-to-back iterator yielding elements by
    /// value.
    fn into_iter(self) -> IntoIter<T, A> {
        IntoIter { inner: self }
    }
}

impl<'a, T: 'a, A: 'a + Allocator> IntoIterator for &'a VecDeque<T, A> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<'a, T: 'a, A: 'a + Allocator> IntoIterator for &'a mut VecDeque<T, A> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;

    fn into_iter(mut self) -> IterMut<'a, T> {
        self.iter_mut()
    }
}

impl<T, A: Allocator> Extend<T> for VecDeque<T, A> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for elt in iter {
            self.push_back(elt);
        }
    }
}

impl<'a, T: 'a + Copy, A: 'a + Allocator> Extend<&'a T> for VecDeque<T, A> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        self.extend(iter.into_iter().cloned());
    }
}

impl<T: fmt::Debug, A: Allocator> fmt::Debug for VecDeque<T, A> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_list().entries(self).finish()
    }
}

#[cfg(test)]
mod tests {
    use core::iter::Iterator;
    use core::option::Option::Some;

    use super::VecDeque;

    #[test]
    fn test_swap_front_back_remove() {
        fn test(back: bool) {
            // This test checks that every single combination of tail position and length is tested.
            // Capacity 15 should be large enough to cover every case.
            let mut tester = VecDeque::with_capacity(15);
            let usable_cap = tester.capacity();
            let final_len = usable_cap / 2;

            for len in 0..final_len {
                let expected = if back {
                    (0..len).collect()
                } else {
                    (0..len).rev().collect()
                };
                for tail_pos in 0..usable_cap {
                    tester.tail = tail_pos;
                    tester.head = tail_pos;
                    if back {
                        for i in 0..len * 2 {
                            tester.push_front(i);
                        }
                        for i in 0..len {
                            assert_eq!(tester.swap_remove_back(i), Some(len * 2 - 1 - i));
                        }
                    } else {
                        for i in 0..len * 2 {
                            tester.push_back(i);
                        }
                        for i in 0..len {
                            let idx = tester.len() - 1 - i;
                            assert_eq!(tester.swap_remove_front(idx), Some(len * 2 - 1 - i));
                        }
                    }
                    assert!(tester.tail < tester.cap());
                    assert!(tester.head < tester.cap());
                    assert_eq!(tester, expected);
                }
            }
        }
        test(true);
        test(false);
    }

    #[test]
    fn test_insert() {
        // This test checks that every single combination of tail position, length, and
        // insertion position is tested. Capacity 15 should be large enough to cover every case.

        let mut tester = VecDeque::with_capacity(15);
        // can't guarantee we got 15, so have to get what we got.
        // 15 would be great, but we will definitely get 2^k - 1, for k >= 4, or else
        // this test isn't covering what it wants to
        let cap = tester.capacity();


        // len is the length *after* insertion
        for len in 1..cap {
            // 0, 1, 2, .., len - 1
            let expected = (0..).take(len).collect();
            for tail_pos in 0..cap {
                for to_insert in 0..len {
                    tester.tail = tail_pos;
                    tester.head = tail_pos;
                    for i in 0..len {
                        if i != to_insert {
                            tester.push_back(i);
                        }
                    }
                    tester.insert(to_insert, to_insert);
                    assert!(tester.tail < tester.cap());
                    assert!(tester.head < tester.cap());
                    assert_eq!(tester, expected);
                }
            }
        }
    }

    #[test]
    fn test_remove() {
        // This test checks that every single combination of tail position, length, and
        // removal position is tested. Capacity 15 should be large enough to cover every case.

        let mut tester = VecDeque::with_capacity(15);
        // can't guarantee we got 15, so have to get what we got.
        // 15 would be great, but we will definitely get 2^k - 1, for k >= 4, or else
        // this test isn't covering what it wants to
        let cap = tester.capacity();

        // len is the length *after* removal
        for len in 0..cap - 1 {
            // 0, 1, 2, .., len - 1
            let expected = (0..).take(len).collect();
            for tail_pos in 0..cap {
                for to_remove in 0..len + 1 {
                    tester.tail = tail_pos;
                    tester.head = tail_pos;
                    for i in 0..len {
                        if i == to_remove {
                            tester.push_back(1234);
                        }
                        tester.push_back(i);
                    }
                    if to_remove == len {
                        tester.push_back(1234);
                    }
                    tester.remove(to_remove);
                    assert!(tester.tail < tester.cap());
                    assert!(tester.head < tester.cap());
                    assert_eq!(tester, expected);
                }
            }
        }
    }

    #[test]
    fn test_shrink_to_fit() {
        // This test checks that every single combination of head and tail position,
        // is tested. Capacity 15 should be large enough to cover every case.

        let mut tester = VecDeque::with_capacity(15);
        // can't guarantee we got 15, so have to get what we got.
        // 15 would be great, but we will definitely get 2^k - 1, for k >= 4, or else
        // this test isn't covering what it wants to
        let cap = tester.capacity();
        tester.reserve(63);
        let max_cap = tester.capacity();
        

        for len in 0..cap + 1 {
            // 0, 1, 2, .., len - 1
            let expected = (0..).take(len).collect();
            for tail_pos in 0..max_cap + 1 {
                
                tester.tail = tail_pos;
                tester.head = tail_pos;
                tester.reserve(63);
                for i in 0..len {
                    tester.push_back(i);
                }
                tester.shrink_to_fit();
                assert!(tester.capacity() <= cap);
                assert!(tester.tail < tester.cap());
                assert!(tester.head < tester.cap());
                assert_eq!(tester, expected);
                
            }
        }
    }

    #[test]
    fn test_zst_push() {
        const N: usize = 8;

        // Zero sized type
        struct Zst;

        // Test that for all possible sequences of push_front / push_back,
        // we end up with a deque of the correct size

        for len in 0..N {
            let mut tester = VecDeque::with_capacity(len);
            assert_eq!(tester.len(), 0);
            assert!(tester.capacity() >= len);
            for case in 0..(1 << len) {
                assert_eq!(tester.len(), 0);
                for bit in 0..len {
                    if case & (1 << bit) != 0 {
                        tester.push_front(Zst);
                    } else {
                        tester.push_back(Zst);
                    }
                }
                assert_eq!(tester.len(), len);
                assert_eq!(tester.iter().count(), len);
                tester.clear();
            }
        }
    }
}