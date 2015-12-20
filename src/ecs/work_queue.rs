//! A work-stealing fork-join queue used by the ECS to perform processors' work asynchronously.
use memory::Allocator;
use memory::boxed::{AllocBox, FnBox};
use memory::collections::Vector;

use std::cell::RefCell;
use std::mem;
use std::ptr;
use std::raw::TraitObject;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicIsize, fence, Ordering};
use std::thread;

// Arguments that job functions take.
type Args = ();

const JOB_SIZE: usize = 64;
const ARR_SIZE: usize = (JOB_SIZE / 8) - 1;

// A job is some raw binary data the size of a cache line
// that will have a closure packed inside it.
// 
// this also stores a vtable pointer to create a trait object at the call site.
// this does not run destructors.
struct Job {
	raw_data: [u64; ARR_SIZE], 
	vtable: *mut (),
}

impl Job {
	fn new<F>(f: F) -> Job where F: FnMut<Args, Output=()> {
		use std::intrinsics;
		
		unsafe { assert!(!intrinsics::needs_drop::<F>(), "Job functions cannot have destructors."); }
		
		assert!(mem::align_of::<F>() <= mem::align_of::<[u64; ARR_SIZE]>(), "Function alignment requirement is too high.");
		assert!(mem::size_of::<F>() <= ARR_SIZE * 8, "Function environment too large.");
		
		let mut raw_data = [0u64; ARR_SIZE];
		
		unsafe {
			// write the closure's environment to the end.
			let fn_ptr: *mut F = &mut raw_data[0] as *mut _ as *mut F;
			ptr::write(fn_ptr, f);
			
			// create a custom trait object which stores a relative offset,
			// and put that at the beginning.
			let fat_ptr: *mut FnMut<Args, Output=()> = fn_ptr;
			
			// don't store this trait object, just the vtable.
			// we know where the data is, and the data pointer here
			// might become obsolete if we move the Job around.
			let t_obj: TraitObject = mem::transmute(fat_ptr);
			
			Job {
				raw_data: raw_data,
				vtable: t_obj.vtable,
			}
		}
	}
	
	// Any unsafety that could occur when using a `Job` occurs here:
	//   - captured data escaping its lifetime
	//   - calling an invalid `Job` structure
	// users of this method (me!) must be careful to avoid such cases. 
	unsafe fn call(self, args: Args) {
		unsafe {
			let t_obj = TraitObject {
				data: &self.raw_data[0] as *const _ as *mut (),
				vtable: self.vtable,
			};
			
			let fn_ptr: *mut FnMut<Args, Output=()> = mem::transmute(t_obj);
			(*fn_ptr).call_mut(args);
		}
	}
}

type QueueEntry = RefCell<Option<Job>>;

// a fixed size ring buffer for storing jobs.
// each worker will have one of these, as well as a
// reference to the other workers'.
//
// This is lock-free, built upon the assumption that 
// a worker manages the private, bottom end of its queue.
// other workers, when idle, may steal from the public, top end of the queue.
struct Queue<A: Allocator> {
	buf: AllocBox<[QueueEntry], A>,
	// (bottom, top)
	top: AtomicIsize,
	bottom: AtomicIsize,
	len: usize,
}

// transforms a (possibly negative) index into a wrapped index in the range
// [0, self.len - 1].
fn wraparound_index(idx: isize, len: usize) -> usize {
	(idx & ((len - 1) as isize)) as usize
}
	
impl<A: Allocator> Queue<A> {
	// create a new queue with the given length using memory from the allocator.
	// the length must be a power of two.
	fn new(len: usize, alloc: A) -> Self {
		assert!(len.is_power_of_two(), "Job queue size must be a power of two.");
		let mut v = Vector::with_alloc_and_capacity(alloc, len);
		for _ in 0..len {
			v.push(RefCell::new(None));
		}
		
		Queue {
			buf: v.into_boxed_slice(),
			top: AtomicIsize::new(0),
			bottom: AtomicIsize::new(0),
			len: len,
		}
	}
	
	// Push a job onto the private end of the queue.
	// this can only be done from the thread which logically owns this queue!
	unsafe fn push(&self, job: Job) {
		let (top, bottom) = (self.top.load(Ordering::SeqCst), self.bottom.load(Ordering::SeqCst));
		
		let idx = wraparound_index(bottom, self.len);
		
		// FIXME: rustc is saying this borrows self mutably when put in one line.
		// I've split it into two for now, but maybe later I can conglomerate them again.
		let mut slot = self.buf[idx].borrow_mut();
		*slot = Some(job);
		
		fence(Ordering::SeqCst);
		self.bottom.fetch_add(1, Ordering::SeqCst);
	}
	
	// Pop a job from the private end of the queue.
	// this can only be done from the thread which logically owns this queue!
	unsafe fn pop(&self) -> Option<Job> {
		// loop until the queue is empty or we get a job!
		let (top, new_bottom) = (self.top.load(Ordering::SeqCst), self.bottom.load(Ordering::SeqCst) - 1);		
		self.bottom.store(new_bottom, Ordering::SeqCst);
		
		if new_bottom <= top {
			// empty queue.
			// set bottom = top just in case it wasn't already
			self.bottom.store(top, Ordering::SeqCst);
			None
		} else {
			// non-empty
			
			let idx = wraparound_index(new_bottom, self.len);
			let j = unsafe {
				ptr::read(&*self.buf[idx].borrow())
			};
			
			if top != new_bottom {
				// many items in queue.
				debug_assert!(j.is_some(), "popped job was empty. Top: {}, Bottom: {}", top, new_bottom);
				j
			} else {
				// last item in queue. set bottom = top + 1.
				
				// may be racing against a steal operation, so 
				// try to increment the top and see what the state is.
				if self.top.compare_and_swap(top, top + 1, Ordering::SeqCst) != top {
					// lost a race. set bottom to top + 1
					// since the queue is now empty, and the steal
					// must have incremented top
					self.bottom.store(top + 1, Ordering::SeqCst);
					None
				} else {
					// won the race (if there was one).
					// set bottom = top + 1 and the job slot to None.
					// I have chosen to repeat myself here for the sake of explicitness.
					let mut slot = self.buf[idx].borrow_mut();
					*slot = None;
					
					self.bottom.store(top + 1, Ordering::SeqCst);
					debug_assert!(j.is_some(), "popped job was empty. Top: {}, Bottom: {}", top, new_bottom);
					j
				}
			}
		}
	}
	
	// Try to steal a job from the public end of the queue.
	fn steal(&self) -> Option<Job> {
		let (top, bottom) = (self.top.load(Ordering::SeqCst), self.bottom.load(Ordering::SeqCst));
		
		if bottom <= top {
			// empty queue.
			None
		} else {
			// non-empty queue.
			// we may be competing with concurrent pop or steal calls.
			let idx = wraparound_index(top, self.len);
			let j = unsafe {
				ptr::read(&*self.buf[idx].borrow())
			};
			
			if self.top.compare_and_swap(top, top + 1, Ordering::SeqCst) != top {
				// lost a race.
				None
			} else {
				// won a race. store `None` in the job slot again.
				let mut slot = self.buf[idx].borrow_mut();
				*slot = None;
				
				// any jobs that were in the ostensibly full part of the queue should be valid!
				debug_assert!(j.is_some(), "stolen job was empty. Top: {}, Bottom: {}", top, bottom);
				
				j
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{Job, Queue};
	use memory::allocator::DefaultAllocator;
	
	#[test]
	fn job_basics() {
		let mut i = 0;
		{
			let j = Job::new(|| i += 1);
			unsafe { j.call(()) };
		}
		assert_eq!(i, 1);
	}
	
	#[test]
	fn queue_basics() {
		let mut i = 0;
		{
			let queue = Queue::new(256, DefaultAllocator);
			for _ in 0..5 {
				unsafe { queue.push(Job::new(|| i += 1)); }
			}
			
			// no way these should fail.
			unsafe { queue.pop().unwrap().call(()); }
			for _ in 0..4 {
				unsafe { queue.steal().unwrap().call(()); }
			}
			
			unsafe { assert!(queue.pop().is_none()); }
			assert!(queue.steal().is_none());
		}
		assert_eq!(i, 5);
	}
}