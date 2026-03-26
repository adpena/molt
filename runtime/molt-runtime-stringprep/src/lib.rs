#![allow(dead_code, unused_imports)]
//! `molt-runtime-stringprep` — RFC 3454 StringPrep intrinsics for the Molt runtime.
//!
//! Isolates the stringprep table membership and mapping intrinsics into a
//! dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_stringprep` feature flag.  When the feature is disabled the linker
//! can strip all stringprep intrinsic code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod stringprep;
