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

pub mod ecs;
pub mod memory;
mod util;

extern crate alloc;
extern crate core;