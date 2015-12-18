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
	
	fn call(self, args: Args) {
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

#[cfg(test)]
mod tests {
	use super::Job;
	
	#[test]
	fn job_basics() {
		let mut i = 0;
		{
			let j = Job::new(|| i += 1);
			j.call(());
		}
		assert_eq!(i, 1);
	}
}