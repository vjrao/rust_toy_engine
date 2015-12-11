#![feature(
	alloc,
	core_intrinsics,
	default_type_parameter_fallback,
	drop_in_place,
	nonzero,
	heap_api,
	unique
)]

pub mod ecs;
pub mod memory;

extern crate alloc;
extern crate core;