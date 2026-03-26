//! molt-runtime-itertools: Python `itertools` module implementation.
//!
//! Extracted from molt-runtime to allow tree-shaking when not needed.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

/// The itertools implementation.
pub mod itertools;
