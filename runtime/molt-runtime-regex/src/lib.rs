//! `molt-runtime-regex` -- Regex/re module group for the Molt runtime.
//!
//! Isolates the `re` Python module intrinsics into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_regex` feature flag.  When the feature is disabled the linker
//! can strip all regex code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod regex;
