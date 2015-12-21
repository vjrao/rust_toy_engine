//! A work-stealing fork-join queue used by the ECS to perform processors' work asynchronously.
//! This is intended to be short-lived. Long running asynchronous tasks should use another method.
use memory::{Allocator, AllocBox};
use memory::collections::Vector;

use std::any::Any;
use std::boxed::FnBox;
use std::cell::{Cell, RefCell};
use std::intrinsics;
use std::mem;
use std::panic;
use std::ptr;
use std::raw::TraitObject;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicUsize, fence, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;

const JOB_SIZE: usize = 256;
const ARR_SIZE: usize = (JOB_SIZE / 8) - 1;

const MAX_JOBS: usize = 4096;

// virtual wrapper for arbitrary function.
// the virtualization here is an acceptable overhead.
trait Code {
	unsafe fn call(&mut self, args: &Worker);	
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

impl<F, R: Send> Code for CodeImpl<F, R> where F: FnMut(&Worker) -> R {
	#[inline]
	unsafe fn call(&mut self, args: &Worker) {
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
	fn new<F, R: Send>(f: F, dest: *mut JobResult<R>) -> Job where F: FnMut(&Worker) -> R {
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
	unsafe fn call(self, args: &Worker) {
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
	// this will loop either until it gets a job or the queue is empty.
	fn steal(&self) -> Option<*mut Job> {
		loop {
			let top = self.top.load(Ordering::SeqCst);
			
			unsafe { intrinsics::atomic_singlethreadfence() }; // compiler memory barrier
			
			let bottom = self.bottom.load(Ordering::SeqCst);
			
			if bottom <= top {
				// empty queue.
				return None
			} else {
				// non-empty queue.
				// we may be competing with concurrent pop or steal calls.
				let idx = wraparound_index(top);
				let j: *mut Job = self.buf[idx];
				
				if self.top.compare_and_swap(top, top + 1, Ordering::SeqCst) != top {
					// lost a race.
					continue;
				} else {
					// won a race. store null in the job slot again.
					unsafe { self.write_job(idx, ptr::null_mut()) }
					return Some(j)
				}
			}
		}
	}
    
    // get the number of jobs in the queue. Due to the multithreaded nature of 
    // the queue, this is not guaranteed to be completely accurate.
    fn len(&self) -> usize {
        let top = self.top.load(Ordering::Relaxed);
        let bottom = self.top.load(Ordering::Relaxed);
        
        ::std::cmp::min(top - bottom, 0) as usize
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

// messages to workers.
enum ToWorker {
	Start, // start
	Clear, // run all jobs to completion, then reset the pool.
	Shutdown, // all workers are clear and no jobs are running. safe to tear down pools.
}

// messages to the leader from workers.
enum ToLeader {
	Cleared, // done clearing. will wait for start or shutdown
	Panicked(Box<Any + Send>), // for propagating panics.
}

struct WorkerHandle {
	send: Sender<ToWorker>,
	recv: Receiver<ToLeader>,
	thread: thread::JoinHandle<()>,
}

/// The job system manages worker threads in a 
/// work-stealing fork-join thread pool.
pub struct JobSystem<A: Allocator> {
	workers: Vector<WorkerHandle, A>,
	queue: Arc<Queue>,
    queue_list: Vector<Arc<Queue>, A>,
}

pub struct Worker {
	queue: Arc<Queue>,
	sibling_queues: *const [Arc<Queue>],
}

unsafe impl Send for Worker {}

struct RecoverSafeParams {
	send: Sender<ToLeader>,
	recv: Receiver<ToWorker>,
	worker: Worker,
}

impl panic::RecoverSafe for RecoverSafeParams {}

impl<A: Allocator + Clone + Send> JobSystem<A> {
	/// Creates a job system with `n` worker threads, not including
	/// the local thread, backed by the given allocator.
	pub fn new(n: usize, alloc: A) -> Self {
		let mut queues = Vector::with_alloc_and_capacity(alloc.clone(), n + 1);
		let mut workers = Vector::with_alloc_and_capacity(alloc, n + 1);
		
		// make queues
		for _ in 0..(n+1) {
			queues.push(Arc::new(Queue::new()));
		}
		
		// spawn workers.
		for i in 1..(n+1) {
			let (work_send, work_recv) = channel();
			let (lead_send, lead_recv) = channel();
			let worker = Worker {
				queue: queues[i].clone(),
				sibling_queues: &queues[..] as *const _, 	
			};
			
			let join_handle = thread::spawn(move || {
                // recover from panics at the top level, so the thread local pool remains valid and no dangling pointers
				// to jobs are kept until the main thread panics.
				let outer_send = lead_send.clone();
				
				// no invariants will be broken when a worker goes down early.
				// the queues are all atomically reference counted, so they won't be destructed.
				// the job pool will be kept alive, so there are no dangling pointers in other queues.
				let params = RecoverSafeParams {
					send: lead_send,
					recv: work_recv,
					worker: worker,
				};
				
				let maybe_panicked = panic::recover(move || {
					let RecoverSafeParams { send: send, recv: recv, worker: worker } = params;
					worker_main(send, recv, worker);
				});
				
				// propagate panics up and spin until we get shut down.
				// we spin to keep the thread local pool alive.
				if let Err(panic) = maybe_panicked {
					let _ = outer_send.send(ToLeader::Panicked(panic));
					loop {
                        // loop just in case we get unparked somehow.
						thread::park();
					}
				}
            });
			
			workers.push(WorkerHandle {
				send: work_send,
				recv: lead_recv,
				thread: join_handle,
			});
        }
        
        // tell every worker to start
        for worker in &workers {
            worker.send.send(ToWorker::Start).ok().expect("Worker hung up on job system");
        }
		
		JobSystem {
			workers: workers,
			queue: queues[0].clone(),
            queue_list: queues,
		}
	}
}

impl<A: Allocator> Drop for JobSystem<A> {
    fn drop(&mut self) {
        // send every worker a clear message
        for worker in &self.workers {
            worker.send.send(ToWorker::Clear).ok().expect("Worker hung up on job system.");
        }
        
        // wait for confirmation from each worker.
        // the queues are not guaranteed to be empty until the last one responds!
        for worker in &self.workers {
            if let Ok(ToLeader::Cleared) = worker.recv.recv() {
                continue;
            } else {
                panic!("Worker failed to clear queue.");
            }
        }
        
        // now tell every worker they can shut down safely.
        for worker in &self.workers {
            worker.send.send(ToWorker::Shutdown).ok().expect("Worker hung up on job system");
        }
    }
}

impl Worker {
    // try to pop a job or steal one from the queue with the most jobs.
    fn get_job(&self) -> Option<*mut Job> {    
        if let Some(job) = unsafe { self.queue.pop() } {
            Some(job)
        } else {
            let (mut most_work, mut steal_idx) = (0, None);
            let siblings = unsafe { &*self.sibling_queues };
            for (idx, queue) in siblings.iter().enumerate() {
                let len = queue.len();
                if len > most_work { 
                    most_work = len;
                    steal_idx = Some(idx);
                }
            }
            
            if let Some(idx) = steal_idx {
               siblings[idx].steal()
            } else {
                None
            }
        }
    }
    
    // Do a sweep of each queue, including the worker's own, running jobs until it's complete.
    // Since jobs can spawn new jobs, this is not necessarily an indication that all queues are now
    // empty. If all queues are empty upon first inspection, this will return true. once all workers
    // have observed this state, all jobs have finished.
    fn clear_sweep(&self) -> bool {
        let mut all_clear = true;
        for queue in unsafe { &*self.sibling_queues } {
            while let Some(job) = queue.steal() {
                all_clear = false;
                unsafe { (*job).call(self) }
            }
        }
        
        all_clear
    }
    
    #[inline]
    fn clear(&self) {
        loop {
            if self.clear_sweep() { break }
        }
    }
}

fn worker_main(send: Sender<ToLeader>, recv: Receiver<ToWorker>, worker: Worker) {
    // Finite state machine for worker actions
    #[derive(Clone, Copy)]
    enum WorkerState {
        Running, // running jobs normally. waiting for state change.
        Paused, // paused, waiting for start or shutdown message.
    }
    
	let mut state = WorkerState::Paused;
    loop {
        match state {
            WorkerState::Running => {
                // check for state change, otherwise run the next job
                match recv.try_recv() {
                    Err(TryRecvError::Disconnected) => panic!("job system hung up on worker"),
                    Ok(msg) => match msg {
                        ToWorker::Clear => {
                            worker.clear();
                            send.send(ToLeader::Cleared);
                            state = WorkerState::Paused;
                            continue;
                        }
                        
                        _ => panic!("received unexpected message while running"),
                    },
                    
                    _ => {}
                }
                
                if let Some(job) = worker.get_job() {
                    unsafe { (*job).call(&worker) }
                }
                
                // FIXME: does it make sense to run this loop as fast as possible?
                // should i yield a time slice to the os scheduler somehow?
            }
            
            WorkerState::Paused => {
                // wait for start or shutdown
                match recv.recv().ok().expect("job system hung up on worker") {
                    ToWorker::Start => { 
                        state = WorkerState::Running; 
                        continue;
                    }
                    ToWorker::Shutdown => break,
                    _ => panic!("received unexpected message while paused"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
	use super::{JobSystem, Worker};
    
    use memory::DefaultAllocator;
    
    #[test]
    fn creation_destruction() {
        for i in 0..32 {
            let _ = JobSystem::new(i, DefaultAllocator);
        }
    }
}