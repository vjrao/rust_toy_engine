//! Memory management utilities.
//! 
//! The memory module contains code for custom memory allocators, taken directly from
//! [pnkfelix's Allocators, Take III RFC](https://github.com/rust-lang/rfcs/pull/1398).
//! When this lands in the language, this code will switch to that.
//! 
//! This module will also be home to a reimplementation of `Box` (named `AllocBox`) to be generic over allocators,
//! as well as various collections made generic over allocators as need arises. When these features land,
//! this module will become more or less obsolete.

pub mod allocator;
pub mod boxed;
pub mod collections;

pub use self::allocator::{Address, Allocator, DefaultAllocator, Kind};
pub use self::boxed::AllocBox;
pub use self::collections::Vector;