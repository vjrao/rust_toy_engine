//! An entity component system, attempting to be completely type-safe, multithreaded, and performant.
//! The brunt of the work in this module is done in user-provided `Processors`. The `World` manages entities and component data.
//!
//! The world can be used to create an execution context, which is then supplied with the processors to execute. It provides
//! control-flow utilities for defining a strict order for processors to execute in, while also allowing for non-competing processors
//! to execute their code efficiently across multiple threads.
//!
//! This is a work in progress and is not in a usable state at the moment.
#![allow(dead_code, unused_imports)]

mod component;
mod entity;
mod internal;
mod world;

pub use self::component::Component;
pub use self::entity::Entity;
pub use self::world::{World, WorldBuilder};

// Maximum guaranteed component alignment.
// There may be large implications of this being changed,
// most notably many things not working.
const COMPONENT_ALIGN: usize = 16;

// Sizes of blocks in each slab.
const SMALL_SIZE: usize = 512;
const MEDIUM_SIZE: usize = 2048;
const LARGE_SIZE: usize = 8192;
