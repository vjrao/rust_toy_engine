//! A work-stealing fork-join queue used by the ECS to perform processors' work asynchronously.
//! This is intended to be short-lived. Long running asynchronous tasks should use another method.
use memory::{Allocator, AllocBox};
use memory::collections::Vector;

use self::job::{Job, JOB_POOL, Pool, Queue};

use std::any::Any;
use std::boxed::FnBox;
use std::io::Error;
use std::panic;
use std::sync::Arc;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread;

mod job;

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
	pub fn new(n: usize, alloc: A) -> Result<Self, Error> {
        use std::mem;
        
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
            
            // give each thread enough stack for a pool
			let builder = thread::Builder::new()
                .name(format!("worker_{}", i));
			let thread_result = builder.spawn(move || {
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
					let RecoverSafeParams { send, recv, worker } = params;
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
            
            let join_handle = match thread_result {
                Ok(handle) => handle,
                Err(err) => return Err(err),
            };
			
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
		
		Ok(JobSystem {
			workers: workers,
			queue: queues[0].clone(),
            queue_list: queues,
		})
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
                            JOB_POOL.with(|pool| pool.reset());
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
            let _ = JobSystem::new(i, DefaultAllocator).unwrap();
        }
    }
}