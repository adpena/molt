//! `molt-runtime-math` — Math/numeric module group for the Molt runtime.
//!
//! Isolates the `math`, `cmath`, `fractions`, `colorsys`, `random`, and
//! `decimal` Python modules into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_math` feature flag.  When the feature is disabled the linker
//! can strip all math code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod colorsys;
pub mod cmath_mod;
pub mod fractions;
