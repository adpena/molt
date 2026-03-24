//! Re-export bridge: delegates to `molt_runtime_compression::bz2`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_compression` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compress(data_bits: u64, compresslevel_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_compress(data_bits, compresslevel_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompress(data_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_decompress(data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_new(compresslevel_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_compressor_new(compresslevel_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_compress(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_compressor_compress(handle_bits, data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_flush(handle_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_compressor_flush(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_compressor_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_new() -> u64 {
    molt_runtime_compression::bz2::molt_bz2_decompressor_new()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_decompress(handle_bits: u64, data_bits: u64, max_length_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_decompressor_decompress(handle_bits, data_bits, max_length_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_eof(handle_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_decompressor_eof(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_needs_input(handle_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_decompressor_needs_input(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_decompressor_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_unused_data(handle_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_decompressor_unused_data(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_open(filename_bits: u64, mode_bits: u64, compresslevel_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_file_open(filename_bits, mode_bits, compresslevel_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_file_read(handle_bits, size_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_file_write(handle_bits, data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_close(handle_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_file_close(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::bz2::molt_bz2_file_drop(handle_bits)
}
