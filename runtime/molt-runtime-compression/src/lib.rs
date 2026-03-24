//! Compression stdlib modules for molt-runtime.
//!
//! This crate isolates the heavy C native dependencies (bzip2, flate2, xz2, zip)
//! so they can be excluded from micro/edge builds without recompiling the core runtime.

pub mod bridge;
pub mod compression_common;
pub mod bz2;
pub mod gzip;
pub mod zlib;
#[cfg(not(target_arch = "wasm32"))]
pub mod lzma;
pub mod lzma_wasm;
pub mod tarfile;
pub mod zipfile;
