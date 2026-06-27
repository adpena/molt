#![allow(dead_code, unused_imports)]
//! `molt-runtime-ipaddress` — IP address intrinsics for the Molt runtime.
//!
//! Isolates the ipaddress module intrinsics (IPv4/IPv6 addresses, networks,
//! interfaces) into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_ipaddress` feature flag.  When the feature is disabled the linker
//! can strip all ipaddress intrinsic code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;
#[cfg(test)]
#[path = "../../molt-runtime-core/src/bridge_test_stubs.rs"]
mod bridge_test_stubs;

pub mod intrinsics_generated;
pub mod ipaddress;
