//! A vector generic over its allocator.
//! Much of the documentation here is insufficient, as copying it 
//! from the standard library docs proved too great a task for me.
//! I would advise viewers of this page to see the 
//! (docs for std::vec)[doc.rust-lang.org/stable/std/vec/]

use alloc::heap;

use core::intrinsics;

use memory::allocator::{Allocator, DefaultAllocator};
use memory::AllocBox;

use std::mem;
use std::ops::{Deref, DerefMut};
use std::ptr;
use std::slice;

use super::raw_vec::RawVec;

/// A vector manages a contiguous block of memory housing some number
/// of elements. It will dynamically resize itself as it grows.
pub struct Vector<T, A=DefaultAllocator> where A: Allocator {
	buf: RawVec<T, A>,
	len: usize,
}

impl<T> Vector<T, DefaultAllocator> {
	/// Create the largest `Vector` possible which does not allocate, backed by the default allocator.
	/// If T has a non-zero size, this will create a vector with 0 capacity.
	/// If T has size zero, this will create a vector with usize::MAX capacity.
	pub fn new() -> Self {
		Vector::with_alloc(DefaultAllocator)
	}
	
	/// Create a new `Vector` with capacity for `n` elements, backed by the default allocator.
	pub fn with_capacity(n: usize) -> Self {
		Vector::with_alloc_and_capacity(DefaultAllocator, n)
	}
}

impl<T, A: Allocator> Vector<T, A> {
	/// Create a new `Vector` which will draw memory from the supplied allocator.
	pub fn with_alloc(alloc: A) -> Self {
		Vector {
			buf: RawVec::with_alloc(alloc),
			len: 0,
		}
	}
	
	/// Create a new `Vector` with initial capacity `n`, backed by the supplied allocator.
	pub fn with_alloc_and_capacity(alloc: A, n: usize) -> Self {
		Vector {
			buf: RawVec::with_alloc_and_capacity(alloc, n),
			len: 0,
		}
	}
	
	/// Get the length of the `Vector`
	pub fn len(&self) -> usize {
		self.len
	}
	
	/// Gets the capacity of the `Vector`. This is not the same as the length.
	pub fn capacity(&self) -> usize {
		self.buf.cap()
	}
	
	/// Reserve enough space for `n` additional elements.
	/// This may over-allocate for efficiency.
	pub fn reserve(&mut self, n: usize) {
		self.buf.reserve(self.len, n);
	}
	
	/// Reserve space for exactly `n` additional elements.
	pub fn reserve_exact(&mut self, n: usize) {
		self.buf.reserve_exact(self.len, n);
	}
	
	/// Shrinks the capacity of the vector as much as possible.
	pub fn shrink_to_fit(&mut self) {
		self.buf.shrink_to_fit(self.len);
	}
	
	pub fn into_boxed_slice(mut self) -> AllocBox<[T], A> {
		unsafe {
			self.shrink_to_fit();
			let buf = ptr::read(&self.buf);
			mem::forget(self);
			
			buf.into_box()
		}
	}
	
	/// Push an element onto the vector.
	pub fn push(&mut self, val: T) {
		let len = self.len();
		if len == self.capacity() {
			self.buf.double();
		}
		
		unsafe {
			let end = self.buf.ptr().offset(len as isize);
			ptr::write(end, val);
			self.len += 1;
		}
	}
	
	/// Pop an element from the vector.
	pub fn pop(&mut self) -> Option<T> {
		if self.len() != 0 {
			unsafe {
				let end = self.buf.ptr().offset(self.len() as isize);
				let val = ptr::read(end);
				self.len -= 1;
				Some(val)
			}
		} else {
			None
		}
	}
}

impl<T, A: Allocator> Deref for Vector<T, A> {
	type Target = [T];
	
	fn deref(&self) -> &[T] {
		let p = self.buf.ptr();
		unsafe { slice::from_raw_parts(p, self.len) }
	}
}

impl<T, A: Allocator> DerefMut for Vector<T, A> {
	fn deref_mut(&mut self) -> &mut [T] {
		let p = self.buf.ptr();
		unsafe { slice::from_raw_parts_mut(p, self.len) }
	}
}

impl<T: Clone, A: Allocator + Clone> Clone for Vector<T, A> {
	fn clone(&self) -> Self {
		let alloc = self.buf.clone_alloc();
		
		let mut v = Vector::with_alloc_and_capacity(alloc, self.capacity());
		for x in self.iter() { v.push(x.clone()) }
		
		v
	}
}

// Iterators

/// An iterator for a `Vector`
pub struct IntoIter<T, A=DefaultAllocator> where A: Allocator {
	_buf: RawVec<T, A>,
	ptr: *const T,
	end: *const T,
}

unsafe impl<T: Send, A: Allocator + Send> Send for IntoIter<T, A> {}
unsafe impl<T: Sync, A: Allocator + Sync> Sync for IntoIter<T, A> {}

impl<T, A: Allocator> Iterator for IntoIter<T, A> {
	type Item = T;
	
	
    #[inline]
    fn next(&mut self) -> Option<T> {
        unsafe {
            if self.ptr == self.end {
                None
            } else {
                if mem::size_of::<T>() == 0 {
                    // purposefully don't use 'ptr.offset' because for
                    // vectors with 0-size elements this would return the
                    // same pointer.
                    self.ptr = intrinsics::arith_offset(self.ptr as *const i8, 1) as *const T;

                    // Use a non-null pointer value
                    Some(ptr::read(heap::EMPTY as *mut T))
                } else {
                    let old = self.ptr;
                    self.ptr = self.ptr.offset(1);

                    Some(ptr::read(old))
                }
            }
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let diff = (self.end as usize) - (self.ptr as usize);
        let size = mem::size_of::<T>();
        let exact = diff /
                    (if size == 0 {
                         1
                     } else {
                         size
                     });
        (exact, Some(exact))
    }

    #[inline]
    fn count(self) -> usize {
        self.size_hint().0
    }
}


impl<T, A: Allocator> DoubleEndedIterator for IntoIter<T, A> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        unsafe {
            if self.end == self.ptr {
                None
            } else {
                if mem::size_of::<T>() == 0 {
                    // See above for why 'ptr.offset' isn't used
                    self.end = intrinsics::arith_offset(self.end as *const i8, -1) as *const T;

                    // Use a non-null pointer value
                    Some(ptr::read(heap::EMPTY as *mut T))
                } else {
                    self.end = self.end.offset(-1);

                    Some(ptr::read(self.end))
                }
            }
        }
    }
}

impl<T, A: Allocator> ExactSizeIterator for IntoIter<T, A> {}

impl<T, A: Allocator> Drop for IntoIter<T, A> {
    #[unsafe_destructor_blind_to_params]
    fn drop(&mut self) {
        // destroy the remaining elements
        for _x in self {}

        // RawVec handles deallocation
    }
}

impl<'a, T, A: Allocator> IntoIterator for &'a Vector<T, A> {
	type Item = &'a T;
	type IntoIter = slice::Iter<'a, T>;
	
	fn into_iter(self) -> slice::Iter<'a, T> {
		self.iter()
	}
}

impl<'a, T, A: Allocator> IntoIterator for &'a mut Vector<T, A> {
	type Item = &'a mut T;
	type IntoIter = slice::IterMut<'a, T>;
	
	fn into_iter(self) -> slice::IterMut<'a, T> {
		self.iter_mut()
	}
}


impl<T, A: Allocator> IntoIterator for Vector<T, A> {
	type Item = T;
	type IntoIter = IntoIter<T, A>;
	
	#[inline]
    fn into_iter(self) -> IntoIter<T, A> {
        unsafe {
            let ptr = self.buf.ptr();
            let begin = ptr as *const T;
            let end = if mem::size_of::<T>() == 0 {
                intrinsics::arith_offset(ptr as *const i8, self.len() as isize) as *const T
            } else {
                ptr.offset(self.len() as isize) as *const T
            };
            let buf = ptr::read(&self.buf);
            mem::forget(self);
            IntoIter {
                _buf: buf,
                ptr: begin,
                end: end,
            }
        }
    }
}

impl<T, A: Allocator> Drop for Vector<T, A> {
	fn drop(&mut self) {
		// I think this checks if it's been dropped already?
		if !self.buf.unsafe_no_drop_flag_needs_drop() { return; }
		unsafe {
			if intrinsics::needs_drop::<T>() {
				for x in self.iter_mut() {
					// The same could more or less be accomplished
					// by mem::replacing it with mem::unitialized().
					intrinsics::drop_in_place(x);
				}
			}
		}
	}
}