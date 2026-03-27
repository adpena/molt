//! `molt-runtime-path` -- Path/OS intrinsics for the Molt runtime.
//!
//! Isolates pathlib.Path, PurePath, os.walk, os.scandir, and related shutil
//! extras into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_path` feature flag.  When the feature is disabled the linker
//! can strip all path/OS intrinsic code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

#[cfg(target_arch = "wasm32")]
pub mod libc_compat;

pub mod os_ext;
pub mod pathlib;
