//! Compression stdlib modules for molt-runtime.
//!
//! This crate isolates the heavy C native dependencies (bzip2, flate2, xz2, zip)
//! so they can be excluded from micro/edge builds without recompiling the core runtime.
//!
//! Modules will be moved from `molt-runtime/src/builtins/` in a follow-up step.

// placeholder modules -- actual code will be moved from molt-runtime/src/builtins/
// pub mod bz2;
// pub mod gzip;
// pub mod zlib;
// pub mod lzma;
// pub mod lzma_wasm;
// pub mod compression_common;
// pub mod tarfile;
// pub mod zipfile;
