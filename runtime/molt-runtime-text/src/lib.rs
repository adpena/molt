#![allow(dead_code, unused_imports)]
//! `molt-runtime-text` — Text/encoding intrinsics for the Molt runtime.
//!
//! Isolates the unicodedata and html intrinsics into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_text` feature flag.  When the feature is disabled the linker
//! can strip all text/encoding intrinsic code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod unicodedata_mod;
pub mod html;
