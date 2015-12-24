use std::cell::Cell;
use std::sync::Mutex;

use std::ptr;

use super::MAX_JOBS;
use super::job::Job;

const MASK: usize = MAX_JOBS - 1;

/// A double-ended job queue.
/// This is currently locking, but will be made lock-free when performance demands it.
pub struct Queue {
    buf: Mutex<[*mut Job; MAX_JOBS]>,
    top: Cell<usize>,
    bottom: Cell<usize>,
}

unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}

impl Queue {
    pub fn new() -> Self {
        Queue {
            buf: Mutex::new([ptr::null_mut(); MAX_JOBS]),
            top: Cell::new(0),
            bottom: Cell::new(0),
        }
    }
    
    // push a job onto the private end of the queue.
    pub unsafe fn push(&self, job: *mut Job) {
        assert!(!job.is_null(), "Attempted to push null job onto queue");
        let mut buf = self.buf.lock().unwrap();
        
        let b = self.bottom.get() + 1;
        buf[b & MASK] = job;
        self.bottom.set(b);
    }
    
    // pop a job from the private end of the queue.
    pub unsafe fn pop(&self) -> Option<*mut Job> {
        let buf = self.buf.lock().unwrap();
        
        let b = self.bottom.get();
        let t = self.top.get();
        
        if b > t {
            // at least one job. decrement bottom
            self.bottom.set(b - 1);
            Some(buf[b & MASK])
        } else {
            // no jobs.
            None
        }
    }
    
    pub fn steal(&self) -> Option<*mut Job> {
        let buf = self.buf.lock().unwrap();
        
        let b = self.bottom.get();
        let t = self.top.get();
        
        if b > t {
            // at least one job. increment top.
            self.top.set(t + 1);
            Some(buf[t & MASK])
        } else {
            None
        }
    }
    
    pub fn len(&self) -> usize {
        // lock the buffer so we can access the inner variables here.
        let _buf = self.buf.lock().unwrap();
        
        let b = self.bottom.get();
        let t = self.top.get();
        
        b - t
    }
}