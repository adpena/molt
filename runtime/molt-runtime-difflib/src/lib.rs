#![allow(dead_code, unused_imports)]
//! `molt-runtime-difflib` -- difflib intrinsics for the Molt runtime.
//!
//! Isolates the `difflib` Python module (SequenceMatcher, unified_diff,
//! context_diff, ndiff, get_close_matches) into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_difflib` feature flag.  When the feature is disabled the linker
//! can strip all difflib code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod difflib;
