#![allow(dead_code, unused_imports)]
//! `molt-stdlib-graphlib` -- graphlib intrinsics for the Molt runtime.
//!
//! Isolates the `graphlib` Python module (`TopologicalSorter`) into a
//! dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_graphlib` feature flag.  When the feature is disabled the linker
//! can strip all graphlib code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;
#[cfg(test)]
#[path = "../../molt-runtime-core/src/bridge_test_stubs.rs"]
mod bridge_test_stubs;

pub mod graphlib;
pub mod intrinsics_generated;
