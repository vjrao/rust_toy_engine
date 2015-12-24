//! A work-stealing fork-join queue used by the ECS to perform processors' work asynchronously.
//! This is intended to be short-lived. Long running asynchronous tasks should use another method.
use memory::{Allocator, AllocBox, Vector};

mod job;
mod queue;
mod pool;

use self::job::Job;
use self::pool::Pool;
use self::queue::Queue;

use std::any::Any;
use std::boxed::FnBox;
use std::io::Error;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

const MAX_JOBS: usize = 4096;

// messages to workers.
enum ToWorker {
	Start, // start
	Clear, // run all jobs to completion, then reset the pool.
	Shutdown, // all workers are clear and no jobs are running. safe to tear down pools.
}
// messages to the leader from workers.
enum ToLeader {
	Cleared, // done clearing. will wait for start or shutdown
    Panicked,
}

struct WorkerHandle {
	tx: Sender<ToWorker>,
	rx: Receiver<ToLeader>,
	thread: thread::JoinHandle<()>,
}

// Finite state machine for controlling workers.
#[derive(Clone, Copy, PartialEq)]
enum State {
    Running, // running jobs normally. waiting for state change.
    Paused, // paused, waiting for start or shutdown message.
}

pub struct Worker {
    queues: Arc<Vector<Queue>>,
    pools: Arc<Vector<Pool>>,
    idx: usize, // the index of this worker's queue and pool in the vector.
}

// TODO: implement !Sync for Worker. 

impl Worker {
    // pop a job from the worker's queue, or steal one from the queue with the most work.
    fn pop_or_steal(&self) -> Option<*mut Job> {
        if let Some(job) = unsafe { self.queues[self.idx].pop() } {
            return Some(job);
        }
        
        let (mut most_work, mut most_idx) = (0, None);
        for (idx, queue) in self.queues.iter().enumerate() {
            let work = queue.len();
            if work > most_work && idx != self.idx {
                most_work = work;
                most_idx = Some(idx);
            }
        }
      
        if let Some(idx) = most_idx {
            self.queues[idx].steal()
        } else {
            None
        }
    }
    
    // do a sweep through all the queues, saying whether all
    // were observed to be empty initially.
    fn clear_pass(&self) -> bool {
        let mut all_clear = true;
        for (idx, queue) in self.queues.iter().enumerate() {
            if idx != self.idx {
                while let Some(job) = queue.steal() {
                    all_clear = false;
                    unsafe { (*job).call(self) }
                }
            } else {
                while let Some(job) = unsafe { queue.pop() } {
                    all_clear = false;
                    unsafe { (*job).call(self) }
                }
            }
        }
        
        all_clear
    }
    
    fn clear(&self) {
        while !self.clear_pass() {}
    }
    
    // run the next job.
    fn run_next(&self) {
        if let Some(job) = self.pop_or_steal() {
            unsafe { (*job).call(self) }
        }
    }
    
    // This must be called on the thread the worker is active on.
    unsafe fn submit_internal<F>(&self, counter: *const AtomicUsize, f: F) where F: Send + FnOnce(&Worker) {
        let job = Job::new(counter, f);
        let job_ptr = self.pools[self.idx].alloc(job);
        self.queues[self.idx].push(job_ptr);
    }
    
    // construct a new spawning scope.
    // this waits for all jobs submitted internally
    // to complete.
    fn scope<'pool, 'new, F>(&'pool self, f: F)
    where F: FnOnce(&Spawner<'pool, 'new>) {
        let counter = AtomicUsize::new(0);
        let s = make_spawner(self, &counter);
        f(&s);
        
        while counter.load(Ordering::Relaxed) != 0 {
            self.run_next();    
        }
    }
}

/// A job spawner associated with a specific scope.
/// Jobs spawned using this may spawn new jobs with the same lifetime.
pub struct Spawner<'pool, 'scope> {
    worker: &'pool Worker,
    counter: *const AtomicUsize,
    // invariant lifetime.
    _marker: PhantomData<*mut &'scope mut ()>,
}

unsafe impl<'pool, 'scope> Sync for Spawner<'pool, 'scope> {}

impl<'pool, 'scope> Spawner<'pool, 'scope> {
    /// Execute a function which necessarily outlives the scope which
    /// this resides in.
    pub fn execute<F>(&self, f: F) 
    where F: 'scope + Send + FnOnce(&Spawner<'pool, 'scope>) {
        use std::mem;
        
        unsafe { 
            // increment the counter first just in case, somehow, this job
            // gets grabbed before we have a chance to increment it,
            // and we wait for all jobs to complete.
            (*self.counter).fetch_add(1, Ordering::Relaxed);
            self.worker.submit_internal(self.counter, move |worker| {
                // make a new spawner associated with the same scope,
                // but with the correct worker for the thread -- so if 
                // this job spawns any children, we won't break any 
                // invariants by accessing other workers' queues/pools
                // in unexpected ways.
                let spawner = make_spawner(worker, self.counter);
                f(&mem::transmute(spawner) )
            });
        }
    }
    
    /// Construct a new spawning scope smaller than the one this spawner resides in.
    pub fn scope<'new, F>(&self, f: F)
    where 'scope: 'new, F: FnOnce(&Spawner<'pool, 'new>) {
        self.worker.scope(f);
    }
}
fn make_spawner<'a, 'b>(worker: &'a Worker, counter: *const AtomicUsize) -> Spawner<'a, 'b> {
    Spawner {
        worker: worker,
        counter: counter,
        _marker: PhantomData,
    }
}

fn worker_main(tx: Sender<ToLeader>, rx: Receiver<ToWorker>, worker: Worker) {
    struct PanicGuard(Sender<ToLeader>);
    impl Drop for PanicGuard {
        fn drop(&mut self) {
            if thread::panicking() {
                let _ = self.0.send(ToLeader::Panicked);
            }
        }
    }
    // if the worker for this thread panics,
    let _guard = PanicGuard(tx.clone());
    
    let mut state = State::Paused;
    loop {
        match state {
            State::Running => {
                match rx.recv().expect("Pool hung up on worker") {
                    ToWorker::Clear => {
                        worker.clear();
                        tx.send(ToLeader::Cleared).expect("Pool hung up on worker");
                        state = State::Paused;
                    }
                    
                    _ => unreachable!()
                }
                
                worker.run_next();
            }
            
            State::Paused => {
                if let Ok(msg) = rx.recv() {
                    match msg {
                        ToWorker::Start => {
                            state = State::Running;
                            continue;
                        }
                        
                        ToWorker::Shutdown => {
                            break;
                        }
                        
                        _ => unreachable!()
                    }
                } else {
                    panic!("Pool hung up on worker");
                }
            }
        }
    }
}

/// The work pool manages worker threads in a 
/// work-stealing fork-join thread pool.
// The fields here are put in vectors for cache contiguity.
pub struct WorkPool {
	workers: Vector<WorkerHandle>,
    local_worker: Worker,
    state: State,
}

impl WorkPool {
	/// Creates a job system with `n` worker threads.
	pub fn new(n: usize) -> Result<Self, Error> {
        // one extra queue and pool for the job system.
        let queues = Arc::new((0..n+1).map(|_| Queue::new()).collect::<Vector<_>>());
        let pools = Arc::new((0..n+1).map(|_| Pool::new()).collect::<Vector<_>>());
        let mut workers = Vector::with_capacity(n);
        
        for i in 0..n {
            let (work_send, work_recv) = channel();
            let (lead_send, lead_recv) = channel();
            
            let worker = Worker {
                queues: queues.clone(),
                pools: pools.clone(),
                idx: i + 1,
            };
            
            let builder = thread::Builder::new().name(format!("worker_{}", i));
            let handle = try!(builder.spawn(|| worker_main(lead_send, work_recv, worker)));
            
            workers.push(WorkerHandle {
                tx: work_send,
                rx: lead_recv,
                thread: handle,
            });
        }
        
        Ok(WorkPool {
            workers: workers,
            local_worker: Worker {
                queues: queues,
                pools: pools,
                idx: 0,
            },
            state: State::Paused,
        })
	}
    
    /// Finish all current jobs which are queued, and synchronize the workers until
    /// it's time to start again. 
    ///
    /// If any of the threads have panicked, that panic
    /// will be propagated to this thread. Until the next job is submitted,
    /// workers will block, allowing other threads to have higher priority.
    pub fn synchronize(&mut self) {
        self.clear_all();
    }
    
    /// Create a new spawning scope for submitting jobs.
    pub fn scope<'pool, 'new, F>(&'pool mut self, f: F)
    where F: FnOnce(&Spawner<'pool, 'new>) {
        if self.state != State::Running { 
            for worker in &self.workers {
                worker.tx.send(ToWorker::Start).expect("Worker hung up on pool");
            }
            
            self.state = State::Running;
        }
        self.local_worker.scope(f);
    }
    
    // clear all the workers and sets the state to paused.
    fn clear_all(&mut self) {
        if self.state == State::Paused { return; }
        // send every worker a clear message
        for worker in &self.workers {
            worker.tx.send(ToWorker::Clear).ok().expect("Worker hung up on pool.");
        }
        
        // do a clear run on the local worker as well.
        self.local_worker.clear();
        
        let mut panicked = false;
        // wait for confirmation from each worker.
        // the queues are not guaranteed to be empty until the last one responds!
        for worker in &self.workers {
            match worker.rx.recv() {
                Ok(msg) => {
                    match msg {
                        ToLeader::Cleared => continue,
                        ToLeader::Panicked => {
                            panicked = true;
                        }
                    }
                }
                
                Err(_) => panic!("Worker hung up on job system.")
            }
        }
        
        // in debug mode, make sure the queues really are all empty.
        if cfg!(debug_assertions) {
            for queue in self.local_worker.queues.iter() {
                assert!(queue.len() == 0, "Queues not cleared properly.");
            }
        }
        
        // cleared and panicked workers are waiting for a shutdown message.
        if panicked { 
            panic!("Propagating worker thread panic")
        }
        
        // reset the master pool
        self.local_worker.pools[0].reset();
        self.state = State::Paused;
    }
}

impl Drop for WorkPool {
    fn drop(&mut self) {
        // finish all work.
        self.clear_all();
        
        // now tell every worker they can shut down safely.
        for worker in &self.workers {
            worker.tx.send(ToWorker::Shutdown).ok().expect("Worker hung up on job system");
        }
    }
}

#[cfg(test)]
mod tests {
	use super::{WorkPool, Worker};
    
    #[test]
    fn creation_destruction() {
        for i in 0..32 {
            let _ = WorkPool::new(i).unwrap();
        }
    }
    
    #[test]
    fn split_work() {
        let mut pool = WorkPool::new(4).unwrap();
        let mut v = vec![0; 1024];
        pool.scope(|spawner| {
            for (idx, v) in v.iter_mut().enumerate() {
                spawner.execute(move |_| {
                    *v += idx;
                });
            }
        });
        
        assert_eq!(v, (0..1024).collect::<Vec<_>>());
    }
    
    #[test]
    fn multilevel_scoping() {
        let mut pool = WorkPool::new(4).unwrap();
        pool.scope(|spawner| {
            let mut v = vec![0; 256];
            spawner.scope(|_| {
               for i in &mut v { *i += 1 }
            });
            // job is forcibly joined here.
            
            assert_eq!(v, (0..256).map(|_| 1).collect::<Vec<_>>())
        }); // any other jobs would be forcibly joined here.
    }
}