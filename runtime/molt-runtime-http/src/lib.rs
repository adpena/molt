//! `molt-runtime-http` -- HTTP, urllib, socketserver, cookies, ctypes, and
//! logging configuration intrinsics for the Molt runtime.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_http` feature flag.  When the feature is disabled the linker
//! can strip all HTTP/logging intrinsic code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod functions_http;
pub mod functions_logging;
