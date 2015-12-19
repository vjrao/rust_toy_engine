//! A work-stealing fork-join queue used by the ECS to perform processors' work asynchronously.
use memory::Allocator;
use memory::boxed::{AllocBox, FnBox};
use memory::collections::Vector;

use std::cell::RefCell;
use std::mem;
use std::ptr;
use std::raw::TraitObject;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize};
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
// This is not yet lock-free. Currently, it locks,
// but I will move to a lock-free implementation shortly.
// a worker manages the private, front end of its queue.
// other workers, when idle, may steal from the public, back end of the queue.
struct Queue<A: Allocator> {
	buf: AllocBox<[QueueEntry], A>,
	// (bottom, top)
	ends: Mutex<(isize, isize)>,
	len: usize,
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
			ends: Mutex::new((0, 0)),
			len: len,
		}
	}
	
	// Push a job onto the private end of the queue.
	fn push(&self, job: Job) {
		let mut ends = self.ends.lock().unwrap();
		
		let mut slot = self.buf[(ends.0 & (self.len - 1) as isize) as usize].borrow_mut();
		*slot = Some(job);
		ends.0 += 1;
	}
	
	// Pop a job from the private end of the queue.
	fn pop(&self) -> Option<Job> {
		let mut ends = self.ends.lock().unwrap();
		
		// bottom will either equal top, or be greater
		if ends.0 - ends.1 <= 0 {
			None
		} else {
			ends.0 -= 1;
			let job = self.buf[(ends.0 & (self.len - 1) as isize) as usize].borrow_mut().take();
			debug_assert!(job.is_some());
			job
		}
	}
	
	// Steal a job from the public end of the queue.
	fn steal(&self) -> Option<Job> {
		let mut ends = self.ends.lock().unwrap();
		
		if ends.0 - ends.1 <= 0 {
			None
		} else {
			let job = self.buf[(ends.1 & (self.len - 1) as isize) as usize].borrow_mut().take();
			debug_assert!(job.is_some());
			ends.1 += 1;
			
			job
		}
	}
}

#[cfg(test)]
mod tests {
	use super::{Job, Queue};
	
	#[test]
	fn job_basics() {
		let mut i = 0;
		{
			let j = Job::new(|| i += 1);
			unsafe { j.call(()) };
		}
		assert_eq!(i, 1);
	}
}