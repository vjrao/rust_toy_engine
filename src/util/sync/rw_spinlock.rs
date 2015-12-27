use std::cell::{Ref, RefCell, RefMut};
use std::ops::{Deref, DerefMut};
use std::sync::{LockResult, PoisonError};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

#[cfg(target_pointer_width = "32")]
const BITS: usize = 32;
#[cfg(target_pointer_width = "64")]
const BITS: usize = 64;

// a flag for the writers field.
// this indicates an active writer when set.
const ACTIVE_FLAG: usize = 1 << (BITS - 1);

/// A version of a Read-Write-Lock which spins until it can acquire a guard.
/// There may be an infinite amount of concurrent readers as long as there are no writers,
/// and there may be only one writer at a time.
/// This is only really practical in situations where wait times are
/// expected to be shorter than the cost of a context switch.
///
/// This implementation intentionally gives writers a higher priority than readers.
/// ReadGuards may become starved if writers hold the lock for too long.
/// If this is your use-case, a standard `RwLock` would probably be more useful,
/// since spinning + starvation are generally a bad combination.
//  TODO: Poisoning?
pub struct RwSpinLock<T> {
    data: RefCell<T>,
    // simple count of the number of writers.
    readers: AtomicUsize,
    // waiting writers + active writer flag.
    writers: AtomicUsize,
    poison: AtomicBool,
}

impl<T> RwSpinLock<T> {
    /// Create a new RwSpinLock wrapping the given data.
    pub fn new(data: T) -> Self {
        RwSpinLock {
            data: RefCell::new(data),
            readers: AtomicUsize::new(0),
            writers: AtomicUsize::new(0),
            poison: AtomicBool::new(false),
        }
    }
    
    /// Get a handle for reading the data.
    /// This will spin until all pending writers are done.
    ///
    /// # Deadlock
    /// Calling this in a situation where there is a writer which may never be
    /// released can lead to deadlock.
    pub fn read(&self) -> LockResult<ReadGuard<T>> {
        loop {
            // spin until all pending writers have finished.
            while self.writers.load(Ordering::Relaxed) != 0 {}
            // may be racing against a new writer. if they win,
            // we need to give them precedence.
            self.readers.fetch_add(1, Ordering::AcqRel);
            if self.writers.load(Ordering::Acquire) == 0 {
                // won the race. return a lock.
                // there is an edge case where a new writer 
                // just declared itself pending, and we didn't have
                // the chance to see it. Although we didn't technically
                // win the race, the pending writer will wait for us.
                let g = ReadGuard {
                    data: self.data.borrow(),
                    counter: &self.readers,
                };
                
                return 
                if self.poison.load(Ordering::Relaxed) {
                    Err(PoisonError::new(g))
                } else {
                    Ok(g)
                };
            }
            // lost a race, try waiting again.
            self.readers.fetch_sub(0, Ordering::Relaxed);
            continue;
        }
    }
    
    /// Get a handle for reading the data. Once this request has been made,
    /// no readers will begin executing until the lock has been released.
    ///
    /// # Deadlock
    /// Calling this in a situation where the current writer may never be released
    // (i.e. in a single-threaded program) will lead to deadlock.
    pub fn write(&self) -> LockResult<WriteGuard<T>> {
        // increment the writers count to indicate that we are waiting.
        self.writers.fetch_add(1, Ordering::AcqRel) + 1;
        // first, spin until current readers finish.
        // no pending readers should begin now that they see 
        // a pending writer. The acquire 
        while self.readers.load(Ordering::Acquire) > 0 {}
        // wait for the current writer to finish,
        // and then try to grab the open spot.
        loop {
            // spin until the active flag is unset.
            let mut num_writers;
            loop {
                num_writers = self.writers.load(Ordering::Relaxed);
                if num_writers & ACTIVE_FLAG == 0 {
                    break;
                }
            }
            // might be racing against other pending writers here.
            // try to swap the old value with a new one, with one fewer
            // pending writer and the active flag set.
            let new_writers = (num_writers - 1) | ACTIVE_FLAG;
            if self.writers.compare_and_swap(num_writers,
                                    new_writers,
                                    Ordering::AcqRel) == num_writers {
                // won the race. return a handle.
                let g = WriteGuard {
                    data: self.data.borrow_mut(),
                    counter: &self.writers,
                    poison: &self.poison,
                };
                
                return
                if self.poison.load(Ordering::Relaxed) {
                    Err(PoisonError::new(g))             
                } else {
                    Ok(g)
                }                       
            }
            // lost the race.
        }        
    }
    
    /// Whether this lock is "poisoned" or a holder of a write lock has panicked while
    /// holding it.
    pub fn is_poisoned(&self) -> bool {
        self.poison.load(Ordering::Acquire)
    }
}

unsafe impl<T: Send> Send for RwSpinLock<T> {}
unsafe impl<T: Send> Sync for RwSpinLock<T> {}

/// A scope-bounded reading guard yielded by an `RWSpinLock`
pub struct ReadGuard<'a, T: 'a> {
    data: Ref<'a, T>,
    counter: &'a AtomicUsize,
}

impl<'a, T: 'a> Deref for ReadGuard<'a, T> {
    type Target = T;
    
    fn deref(&self) -> &T {
        &*self.data
    }
}

impl<'a, T: 'a> Drop for ReadGuard<'a, T> {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// A scope-bounded writing guard yielded by an `RWSpinLock`
pub struct WriteGuard<'a, T: 'a> {
    data: RefMut<'a, T>,
    counter: &'a AtomicUsize,
    poison: &'a AtomicBool,
}

impl<'a, T: 'a> Deref for WriteGuard<'a, T> {
    type Target = T;
    
    fn deref(&self) -> &T {
        &*self.data
    }
}

impl<'a, T: 'a> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut *self.data
    }
}

impl<'a, T: 'a> Drop for WriteGuard<'a, T> {
    fn drop(&mut self) {
        // unset the active flag and poison if necessary.
        if thread::panicking() {
            self.poison.store(true, Ordering::Release);
        }
        self.counter.fetch_and(!ACTIVE_FLAG, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::channel;
    use std::thread;
    
    use super::RwSpinLock;
    
    #[test]
    fn multiple_readers() {
        let lock = RwSpinLock::new(0);
        {
            let mut locks = Vec::new();
            for _ in 0..32 {
                locks.push(lock.read().unwrap())
            }
        }
    }
    
    // ported over a few tests from std::sync::RwLock
    
    #[test]
    fn smoke() {
        let l = RwSpinLock::new(());
        drop(l.read().unwrap());
        drop(l.write().unwrap());
        drop((l.read().unwrap(), l.read().unwrap()));
        drop(l.write().unwrap());
    }
    
    #[test]
    fn test_rw_arc_poison_wr() {
        let arc = Arc::new(RwSpinLock::new(1));
        let arc2 = arc.clone();
        let _: Result<(), _> = thread::spawn(move|| {
            let _lock = arc2.write().unwrap();
            panic!();
        }).join();
        assert!(arc.read().is_err());
    }

    #[test]
    fn test_rw_arc_poison_ww() {
        let arc = Arc::new(RwSpinLock::new(1));
        assert!(!arc.is_poisoned());
        let arc2 = arc.clone();
        let _: Result<(), _> = thread::spawn(move|| {
            let _lock = arc2.write().unwrap();
            panic!();
        }).join();
        assert!(arc.write().is_err());
        assert!(arc.is_poisoned());
    }

    #[test]
    fn test_rw_arc_no_poison_rr() {
        let arc = Arc::new(RwSpinLock::new(1));
        let arc2 = arc.clone();
        let _: Result<(), _> = thread::spawn(move|| {
            let _lock = arc2.read().unwrap();
            panic!();
        }).join();
        let lock = arc.read().unwrap();
        assert_eq!(*lock, 1);
    }
    #[test]
    fn test_rw_arc_no_poison_rw() {
        let arc = Arc::new(RwSpinLock::new(1));
        let arc2 = arc.clone();
        let _: Result<(), _> = thread::spawn(move|| {
            let _lock = arc2.read().unwrap();
            panic!()
        }).join();
        let lock = arc.write().unwrap();
        assert_eq!(*lock, 1);
    }
    
    #[test]
    fn test_rw_arc() {
        let arc = Arc::new(RwSpinLock::new(0));
        let arc2 = arc.clone();
        let (tx, rx) = channel();

        thread::spawn(move || {
            let mut lock = arc2.write().unwrap();
            for _ in 0..10 {
                let tmp = *lock;
                *lock = -1;
                thread::yield_now();
                *lock = tmp + 1;
            }
            let _ = tx.send(());
        });

        // Readers try to catch the writer in the act
        let mut children = Vec::new();
        for _ in 0..5 {
            let arc3 = arc.clone();
            children.push(thread::spawn(move|| {
                let lock = arc3.read().unwrap();
                assert!(*lock >= 0);
            }));
        }

        // Wait for children to pass their asserts
        for r in children {
            assert!(r.join().is_ok());
        }

        // Wait for writer to finish
        rx.recv().unwrap();
        let lock = arc.read().unwrap();
        assert_eq!(*lock, 10);
    }
}