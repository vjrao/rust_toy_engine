//! `AllocBox` is a reimplementation of Rust's `Box` type to be generic over allocators.
//! It is missing several features of `Box`, but those will be added as time and necessity allow.

use super::{Address, Allocator, DefaultAllocator, Kind};

use alloc::heap;

use core::nonzero::NonZero;

use std::marker::Unsize;
use std::ops::{CoerceUnsized, Deref, DerefMut};
use std::ptr::{self, Unique};

/// A boxed instance of type `T` allocated from the allocator `A`.
/// This defaults to the default allocator, which is the heap.
pub struct AllocBox<T: ?Sized, A=DefaultAllocator> where A: Allocator {
	ptr: Unique<T>,
	alloc: A,
}

impl<T> AllocBox<T, DefaultAllocator> {
	/// Create a new box using memory from the default allocator.
	pub fn new(x: T) -> Self {
		AllocBox::in_alloc(x, DefaultAllocator)
	}	
}

impl<T, A: Allocator> AllocBox<T, A> {
	/// Create a new box using memory from an arbitrary allocator.
	pub fn in_alloc(x: T, alloc: A) -> Self {
		let mut alloc = alloc;
		let addr: Address = match Kind::for_value(&x) {
			Some(kind) => {
				unsafe {
					match alloc.alloc(kind) {
						Ok(a) => { a }
						Err(_) => { alloc.oom() }
					}
				}
			}
			
			None => {
				unsafe { NonZero::new(heap::EMPTY as *mut u8) }
			}
		};
		
		let ptr: Unique<T> = unsafe { Unique::new(*addr as *mut T) };
		unsafe { ptr::write(*ptr, x) }
		
		AllocBox {
			ptr: ptr,
			alloc: alloc,
		}	
	}
	
	/// Manually move the value out of this box. This is an unfortunate workaround due to the fact
	/// that `Box` supports moving out due to compiler magic, and this type does not have that convenience.
	pub fn take(self) -> T {
		use std::mem;
		
		let mut this = self;
		let val = unsafe { ptr::replace(*this.ptr, mem::uninitialized()) };
		
		if let Some(kind) = Kind::for_value(&val) {
			debug_assert!(*this.ptr != heap::EMPTY as *mut T);
			unsafe { 
				let ptr = NonZero::new(*this.ptr as *mut u8);
				let _ = this.alloc.dealloc(ptr, kind);	
			}
		}
		
		mem::forget(this);
		val
	}
}

impl<T: ?Sized, A: Allocator> Deref for AllocBox<T, A> {
	type Target = T;
	
	fn deref(&self) -> &T {
		unsafe { &**self.ptr }
	}
}

impl<T: ?Sized, A: Allocator> DerefMut for AllocBox<T, A> {
	fn deref_mut(&mut self) -> &mut T {
		unsafe { &mut **self.ptr }
	}
}

impl<T, A> Clone for AllocBox<T, A> where T: Clone, A: Allocator + Clone {
	fn clone(&self) -> Self {
		AllocBox::in_alloc((**self).clone(), self.alloc.clone())
	}
}

impl<T: ?Sized, A: Allocator> Drop for AllocBox<T, A> {
	fn drop(&mut self) {
		unsafe {
			if let Some(kind) = Kind::for_value(&**self.ptr) {
				::core::intrinsics::drop_in_place(*self.ptr);
				
				let ptr: Address = NonZero::new(*self.ptr as *mut u8);
				let _ = self.alloc.dealloc(ptr, kind);
			}
		}
	}
}

impl<T: ?Sized, U: ?Sized, A: Allocator> CoerceUnsized<AllocBox<U, A>> for AllocBox<T, A> 
where T: Unsize<U> {}

/// A version of `FnOnce` intended to be used with boxed functions.
pub trait FnBox<Args, Alloc: Allocator> {
	type Output;
	
	fn call_box(this: AllocBox<Self, Alloc>, args: Args) -> Self::Output;
}

impl<Args, Alloc, F> FnBox<Args, Alloc> for F where Alloc: Allocator, F: FnOnce<Args> {
	type Output = F::Output;
	
	#[inline]
	fn call_box(this: AllocBox<Self, Alloc>, args: Args) -> Self::Output {
		this.take().call_once(args)
	}
}

impl<Alloc, Args, F> FnOnce<Args> for AllocBox<F, Alloc> where Alloc: Allocator, F: FnOnce<Args> {
	type Output = F::Output;
	
	#[inline]
	extern "rust-call" fn call_once(self, args: Args) -> Self::Output {
		FnBox::call_box(self, args)
	}
}


#[cfg(test)]
mod tests {
	use super::*;
	
	// ZST used for testing.
	struct Test;
	
	#[test]
	fn make_box() {
		let _: AllocBox<u32> = AllocBox::new(5);
		let _ = AllocBox::new(Test);
	}
	
	#[test]
	fn destructors() {
		struct Incrementor<'a> {
			i: &'a mut i32,
		}
		
		impl<'a> Drop for Incrementor<'a> {
			fn drop(&mut self) { *self.i += 1 }
		}
		
		let mut i = 0;
		{
			let _ = AllocBox::new(Incrementor {
				i: &mut i
			});
		}
		
		assert_eq!(i, 1);
	}
	
	#[test]
	fn cloning() {
		let a = AllocBox::new(5);
		let b = a.clone();
		assert_eq!(*b, 5);
	}
	
	#[test]
	fn fnbox() {
		let mut val = 0;
		{
			let b = AllocBox::new(|| val += 1);
			FnBox::call_box(b, ());
		}
		
		assert_eq!(val, 1);
	}
}