//! Additional synchronization primitives over those in the standard library.
pub use self::rw_spinlock::RwSpinLock;

mod rw_spinlock;