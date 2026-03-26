#![allow(dead_code, unused_imports)]
//! `molt-runtime-logging` -- logging intrinsics for the Molt runtime.
//!
//! Isolates the `logging` Python module (LogRecord, Formatter, Handler,
//! StreamHandler, Logger, Manager, and level utilities) into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_logging_ext` feature flag.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod logging_ext;
