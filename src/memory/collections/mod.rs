//! Various collections and data structures, all of which are generic over allocators.
//! Many of these are slightly less efficient implementations of those in the standard library.
//! In anticipation of data structures generic over an allocator eventually being incorporated
//! into `std`, I have made the deliberate decision not to agonize excessively over minor optimizations,
//! instead choosing to get these up and working.

pub mod vector;
pub mod vec_deque;
pub mod raw_vec;

pub use self::vector::Vector;