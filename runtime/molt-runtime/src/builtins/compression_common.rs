//! Re-export bridge: delegates to `molt_runtime_compression::compression_common`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_compression` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_compression_streams_buffer_size() -> u64 {
    molt_runtime_compression::compression_common::molt_compression_streams_buffer_size()
}
