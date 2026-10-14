//! Direct bridge API for satellite sources compiled inside `molt-runtime`.
//!
//! Reduced stdlib profiles compile selected satellite source files directly by
//! `#[path]`. Those files depend on a `crate::bridge` access layer; this facade
//! preserves that API while routing helper families to focused submodules.

#[cfg(any(not(feature = "stdlib_http"), not(feature = "stdlib_itertools")))]
mod attrs;
#[cfg(any(
    not(feature = "stdlib_http"),
    not(feature = "stdlib_itertools"),
    not(feature = "stdlib_path")
))]
mod core;
#[cfg(not(feature = "stdlib_itertools"))]
mod ext_state;
#[cfg(not(feature = "stdlib_itertools"))]
mod iteration;
#[cfg(not(feature = "stdlib_itertools"))]
mod itertools;
#[cfg(any(test, not(feature = "stdlib_path")))]
mod path;

// Each family compiles only for the satellite sources (and differential
// tests) that call it, so every exported item has a consumer.
#[cfg(any(not(feature = "stdlib_http"), not(feature = "stdlib_itertools")))]
pub use attrs::*;
#[cfg(any(
    not(feature = "stdlib_http"),
    not(feature = "stdlib_itertools"),
    not(feature = "stdlib_path")
))]
pub use core::*;
#[cfg(not(feature = "stdlib_itertools"))]
pub use ext_state::*;
#[cfg(not(feature = "stdlib_itertools"))]
pub use iteration::*;
#[cfg(not(feature = "stdlib_itertools"))]
pub use itertools::*;
#[cfg(any(test, not(feature = "stdlib_path")))]
pub use path::*;
