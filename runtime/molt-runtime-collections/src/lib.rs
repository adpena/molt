//! `molt-runtime-collections` -- Collections and container intrinsics for the Molt runtime.
//!
//! Isolates OrderedDict, defaultdict, deque, Counter, ChainMap, namedtuple
//! validation, and argparse intrinsics into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_collections` feature flag.  When the feature is disabled the linker
//! can strip all collection/argparse intrinsic code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod argparse;
pub mod collections_ext;
