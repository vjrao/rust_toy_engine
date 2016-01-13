#![feature(
	alloc,
	coerce_unsized,
	core_intrinsics,
	default_type_parameter_fallback,
	dropck_parametricity,
	drop_in_place,
	filling_drop,
	fn_traits,
	heap_api,
	nonzero,
	unboxed_closures,
	unique,
	unsafe_no_drop_flag,
	unsize,
)]

#![cfg_attr(test, feature(test))]

pub mod ecs;
pub mod memory;

extern crate alloc;
extern crate core;
extern crate num_cpus;
extern crate jobsteal;

#[cfg(test)]
extern crate test;
