#![allow(dead_code, unused_imports)]
//! `molt-runtime-zoneinfo` — IANA timezone intrinsics for the Molt runtime.
//!
//! Isolates the zoneinfo module intrinsics (TZif parsing, UTC offset,
//! DST detection) into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_zoneinfo` feature flag.  When the feature is disabled the linker
//! can strip all zoneinfo intrinsic code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod zoneinfo;
