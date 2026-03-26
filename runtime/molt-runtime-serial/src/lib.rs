//! molt-runtime-serial: serialization + datetime + encoding module group
//!
//! Extracted from molt-runtime to allow tree-shaking the serialization,
//! datetime, and binary-encoding code when not needed.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod base64_mod;
pub mod binascii;
pub mod configparser;
pub mod csv;
pub mod datetime;

/// Fallback for local UTC offset when libc localtime_r is unavailable.
pub fn molt_time_local_offset_fallback(_secs: i64) -> i64 { 0 }
pub mod decimal;
pub mod email;
pub mod structs;
pub mod zipfile;
