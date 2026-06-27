#![allow(dead_code, unused_imports)]
//! `molt-runtime-text` — Text/encoding intrinsics for the Molt runtime.
//!
//! Isolates text codec facts plus the unicodedata and html intrinsics into a
//! dedicated crate.
//!
//! `molt-runtime` always depends on the small codec registry. The heavier html
//! and unicodedata implementations remain gated behind `stdlib_text` so minimal
//! builds keep the canonical codec facts without pulling the stdlib text surface.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
#[cfg(feature = "stdlib_text")]
pub mod bridge;
#[cfg(test)]
#[path = "../../molt-runtime-core/src/bridge_test_stubs.rs"]
mod bridge_test_stubs;

pub mod codec_registry;
#[cfg(feature = "stdlib_text")]
pub mod html;
#[cfg(feature = "stdlib_text")]
pub mod unicodedata_mod;
