//! molt-runtime-serial: serialization + datetime module group (csv, json, datetime, configparser)
//!
//! Extracted from molt-runtime to allow tree-shaking the serialization and
//! datetime code when not needed.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod csv;

// TODO: migrate from molt-runtime/src/builtins/
// pub mod json;
// // 