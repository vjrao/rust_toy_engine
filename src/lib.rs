#![feature(
	alloc,
	collections_range,
	core_intrinsics,
	default_type_parameter_fallback,
	dropck_parametricity,
	drop_in_place,
	filling_drop,
	nonzero,
	num_bits_bytes,
	heap_api,
	unique,
	unsafe_no_drop_flag,
)]

pub mod ecs;
pub mod memory;

extern crate alloc;
extern crate core;