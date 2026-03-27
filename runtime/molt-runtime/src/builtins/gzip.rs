//! Re-export bridge: delegates to `molt_runtime_compression::gzip`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_compression` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_compress(
    data_bits: u64,
    compresslevel_bits: u64,
    mtime_bits: u64,
) -> u64 {
    molt_runtime_compression::gzip::molt_gzip_compress(data_bits, compresslevel_bits, mtime_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_decompress(data_bits: u64) -> u64 {
    molt_runtime_compression::gzip::molt_gzip_decompress(data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_open(
    filename_bits: u64,
    mode_bits: u64,
    compresslevel_bits: u64,
) -> u64 {
    molt_runtime_compression::gzip::molt_gzip_open(filename_bits, mode_bits, compresslevel_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_runtime_compression::gzip::molt_gzip_read(handle_bits, size_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_write(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_compression::gzip::molt_gzip_write(handle_bits, data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_flush(handle_bits: u64) -> u64 {
    molt_runtime_compression::gzip::molt_gzip_flush(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_close(handle_bits: u64) -> u64 {
    molt_runtime_compression::gzip::molt_gzip_close(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gzip_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::gzip::molt_gzip_drop(handle_bits)
}
