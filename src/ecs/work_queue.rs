//! A work-stealing fork-join queue used by the ECS to perform processors' work asynchronously.
//! This is intended to be short-lived. Long running asynchronous tasks should use another method.
use memory::{Allocator, AllocBox};
use memory::collections::Vector;

use std::cell::{Cell, RefCell};
use std::intrinsics;
use std::mem;
use std::ptr;
use std::raw::TraitObject;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, fence, Ordering};
use std::sync::mpsc::{Sender, Receiver};
use std::thread;

// Arguments that job functions take.
type Args = ();

const JOB_SIZE: usize = 256;
const ARR_SIZE: usize = (JOB_SIZE / 8) - 1;

const MAX_JOBS: usize = 4096;

// virtual wrapper for arbitrary function.
// the virtualization here is an acceptable overhead.
trait Code {
	unsafe fn call(&mut self, args: Args);	
}

// the result of a computation.
struct JobResult<R> {
	flag: AtomicBool,
	result: Option<R>,
}

impl<R: Send> JobResult<R> {
	fn set(&mut self, res: R) {
		// this may be racing against a get operation on another thread
		assert!(self.result.is_none(), "Job result set more than once.");
		
		// write the result before setting flag to true.
		self.result = Some(res);
		self.flag.store(true, Ordering::SeqCst);
	}
	
	#[inline]
	fn is_done(&self) -> bool {
		// only returns true if the result is fully written.
		self.flag.load(Ordering::SeqCst)
	}
	
	fn get(&mut self) -> Option<R> {
		if self.is_done() {
			let res = self.result.take();
			debug_assert!(res.is_some(), "Result set to None after job call.");
			res
		} else {
			None
		}
	}
}

// a closure and a destination pointer.
struct CodeImpl<F, R> {
	func: F,
	dest: *mut JobResult<R>,
}

impl<F, R: Send> Code for CodeImpl<F, R> where F: FnMut(Args) -> R {
	#[inline]
	unsafe fn call(&mut self, args: Args) {
		let res = (self.func)(args);
		(*self.dest).set(res);
	}
}

// A job is some raw binary data at least the size of a cache line.
// On construction, we pack inside it:
//   - a function pointer and its closed environment.
//   - a pointer to write the result of the computation to.
//   - a vtable pointer to reconstruct a trait object at the call site.
// All of the following together must be no larger than JOB_SIZE.
#[derive(Clone, Copy)]
struct Job {
	raw_data: [u64; ARR_SIZE], 
	vtable: *mut (),
}

impl Job {
	// create a new job with a destination to write the result to after it is called.
	// This pointer should be to a boxed memory location.
	fn new<F, R: Send>(f: F, dest: *mut JobResult<R>) -> Job where F: FnMut(Args) -> R {
		assert!(mem::align_of::<CodeImpl<F,R>>() <= mem::align_of::<[u64; ARR_SIZE]>(), 
			"Function alignment requirement is too high.");
			
		assert!(mem::size_of::<CodeImpl<F,R>>() <= ARR_SIZE * 8, 
			"Function environment too large.");
			
		assert!(!dest.is_null(), "Job created with null destination.");
		
		let mut raw_data = [0u64; ARR_SIZE];
		let code = CodeImpl {
			func: f,
			dest: dest,
		};
		
		unsafe {
			// write the closure's environment to the array.
			let code_ptr: *mut CodeImpl<F, R> = &mut raw_data[0] as *mut _ as *mut _;
			ptr::write(code_ptr, code);
			
			// create a custom trait object which stores a relative offset,
			// and store the vtable from it. we restore the data pointer at
			// the call site.
			let fat_ptr: *mut Code = code_ptr;
			
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
		let t_obj = TraitObject {
			data: &self.raw_data[0] as *const _ as *mut (),
			vtable: self.vtable,
		};
		
		let code_ptr: *mut Code = mem::transmute(t_obj);
		(*code_ptr).call(args);
	}
}

// a fixed size ring buffer for storing jobs.
// each worker will have one of these, as well as a
// reference to the other workers'.
//
// This is lock-free, built upon the assumption that 
// a worker manages the private, bottom end of its queue.
// other workers, when idle, may steal from the public, top end of the queue.
struct Queue {
	buf: [*mut Job; MAX_JOBS],
	// (bottom, top)
	top: AtomicIsize,
	bottom: AtomicIsize,
}

// transforms a (possibly negative) index into a wrapped index in the range
// [0, MAX_JOBS - 1].
fn wraparound_index(idx: isize) -> usize {
	(idx & ((MAX_JOBS - 1) as isize)) as usize
}
	
impl Queue {
	// create a new queue
	fn new() -> Self {
		unsafe { 
			Queue {
				buf: [ptr::null_mut(); MAX_JOBS],
				top: AtomicIsize::new(0),
				bottom: AtomicIsize::new(0),
			}
		}
	}
	
	// Push a job onto the private end of the queue.
	// this can only be done from the thread which logically owns this queue!
	unsafe fn push(&self, job: *mut Job) {
		debug_assert!(!job.is_null(), "null job pushed onto queue");
		let bottom = self.bottom.load(Ordering::SeqCst);
		
		let idx = wraparound_index(bottom);
		self.write_job(idx, job);
		
		intrinsics::atomic_singlethreadfence(); // compiler memory barrier.
		self.bottom.fetch_add(1, Ordering::SeqCst);
	}
	
	// Pop a job from the private end of the queue.
	// this can only be done from the thread which logically owns this queue!
	unsafe fn pop(&self) -> Option<*mut Job> {
		let new_bottom = self.bottom.load(Ordering::SeqCst) - 1;		
		self.bottom.store(new_bottom, Ordering::SeqCst);
		
		fence(Ordering::SeqCst); // full memory barrier needed here.
		
		let top = self.top.load(Ordering::SeqCst);
		
		if new_bottom <= top {
			// empty queue.
			// set bottom = top just in case it wasn't already
			self.bottom.store(top, Ordering::SeqCst);
			None
		} else {
			// non-empty
			
			let idx = wraparound_index(new_bottom);
			let j: *mut Job = self.buf[idx];
			
			if top != new_bottom {
				// many items in queue.
				debug_assert!(!j.is_null(), "popped job was empty. Top: {}, Bottom: {}", top, new_bottom);
				Some(j)
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
					// set bottom = top + 1 and the job slot to null.
					self.write_job(idx, ptr::null_mut());
					intrinsics::atomic_singlethreadfence(); // compiler memory barrier.
					
					// I have chosen to repeat myself here for the sake of explicitness.
					self.bottom.store(top + 1, Ordering::SeqCst);
					Some(j)
				}
			}
		}
	}
	
	// Try to steal a job from the public end of the queue.
	fn steal(&self) -> Option<*mut Job> {
		let top = self.top.load(Ordering::SeqCst);
		
		unsafe { intrinsics::atomic_singlethreadfence() }; // compiler memory barrier
		
		let bottom = self.bottom.load(Ordering::SeqCst);
		
		if bottom <= top {
			// empty queue.
			None
		} else {
			// non-empty queue.
			// we may be competing with concurrent pop or steal calls.
			let idx = wraparound_index(top);
			let j: *mut Job = self.buf[idx];
			
			if self.top.compare_and_swap(top, top + 1, Ordering::SeqCst) != top {
				// lost a race.
				None
			} else {
				// won a race. store null in the job slot again.
				unsafe { self.write_job(idx, ptr::null_mut()) }
				Some(j)
			}
		}
	}
	
	// writes the pointer given to the slot specified.
	#[inline(always)]
	unsafe fn write_job(&self, idx: usize, job: *mut Job) {
		let slot: *mut *mut Job = &self.buf[idx] as *const *mut Job as *mut _;
		unsafe { *slot = job };
	}
}

unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}

// A thread local pool allocator for jobs.
struct Pool {
	buf: [Job; MAX_JOBS],
	cur: Cell<usize>,
	last_reset: Cell<usize>,
}

impl Pool {
	fn new() -> Self {
		unsafe {
			Pool {
				buf: [mem::uninitialized(); MAX_JOBS],
				cur: Cell::new(0),
				last_reset: Cell::new(0),
			}
		}
	}
	
	// push a job onto the end of the pool and get a pointer to it.
	fn alloc(&self, job: Job) -> *mut Job {		
		unsafe {
			let cur = self.cur.get();
			let idx = cur & (MAX_JOBS - 1);
			self.cur.set(cur + 1);			
			assert!(cur < self.last_reset.get() + MAX_JOBS, 
			"Allocated too many jobs since last reset.");
			
			let slot: *mut Job = &self.buf[idx] as *const Job as *mut _;
			ptr::write(slot, job);
			slot
		}
	}
	
	fn reset(&self) {
		self.last_reset.set(self.cur.get());
	}
}

thread_local!(static JOB_POOL: Pool = Pool::new());

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
	
	#[test]
	fn queue_basics() {
		let mut i = 0;
		{
			let queue = Queue::new();
			for _ in 0..5 {
				unsafe { queue.push(Job::new(|| i += 1)); }
			}
			
			// no way these should fail. unlike rob
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