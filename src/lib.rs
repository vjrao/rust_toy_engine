#![feature(alloc, core_intrinsics, drop_in_place, nonzero, heap_api, unique)]

pub mod ecs;
pub mod memory;

extern crate alloc;
extern crate core;