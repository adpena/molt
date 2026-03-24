//! Re-export bridge: delegates to `molt_runtime_compression::zlib`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_compression` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_deflate_raw(data_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_deflate_raw(data_bits, level_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inflate_raw(data_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_inflate_raw(data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compress(data_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_compress(data_bits, level_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompress(data_bits: u64, wbits_bits: u64, bufsize_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_decompress(data_bits, wbits_bits, bufsize_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_crc32(data_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_crc32(data_bits, value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_adler32(data_bits: u64, value_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_adler32(data_bits, value_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_max_wbits() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_max_wbits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_def_mem_level() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_def_mem_level()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_def_buf_size() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_def_buf_size()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_default_compression() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_default_compression()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_best_speed() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_best_speed()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_best_compression() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_best_compression()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_no_compression() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_no_compression()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_filtered() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_filtered()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_huffman_only() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_huffman_only()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_default_strategy() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_default_strategy()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_finish() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_finish()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_no_flush() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_no_flush()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_sync_flush() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_sync_flush()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_z_full_flush() -> u64 {
    molt_runtime_compression::zlib::molt_zlib_z_full_flush()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_new(level_bits: u64, _method_bits: u64, wbits_bits: u64, _memlevel_bits: u64, _strategy_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_compressobj_new(level_bits, _method_bits, wbits_bits, _memlevel_bits, _strategy_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_compress(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_compressobj_compress(handle_bits, data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_flush(handle_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_compressobj_flush(handle_bits, mode_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_compressobj_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_compressobj_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_new(wbits_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_decompressobj_new(wbits_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_decompress(handle_bits: u64, data_bits: u64, max_length_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_decompressobj_decompress(handle_bits, data_bits, max_length_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_flush(handle_bits: u64, _length_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_decompressobj_flush(handle_bits, _length_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_eof(handle_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_decompressobj_eof(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_unconsumed_tail(handle_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_decompressobj_unconsumed_tail(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zlib_decompressobj_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::zlib::molt_zlib_decompressobj_drop(handle_bits)
}
