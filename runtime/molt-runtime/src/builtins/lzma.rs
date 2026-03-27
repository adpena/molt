//! Re-export bridge: delegates to `molt_runtime_compression::lzma`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_compression` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_auto() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_format_auto()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_xz() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_format_xz()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_alone() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_format_alone()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_raw() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_format_raw()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_none() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_check_none()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_crc32() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_check_crc32()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_crc64() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_check_crc64()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_sha256() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_check_sha256()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_preset_default() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_preset_default()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_preset_extreme() -> u64 {
    molt_runtime_compression::lzma::molt_lzma_preset_extreme()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compress(
    data_bits: u64,
    format_bits: u64,
    check_bits: u64,
    preset_bits: u64,
) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_compress(
        data_bits,
        format_bits,
        check_bits,
        preset_bits,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompress(
    data_bits: u64,
    format_bits: u64,
    _memlimit_bits: u64,
) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_decompress(data_bits, format_bits, _memlimit_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_new(
    format_bits: u64,
    check_bits: u64,
    preset_bits: u64,
) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_compressor_new(format_bits, check_bits, preset_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_compress(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_compressor_compress(handle_bits, data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_flush(handle_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_compressor_flush(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_compressor_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_new(format_bits: u64, _memlimit_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_decompressor_new(format_bits, _memlimit_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_decompress(
    handle_bits: u64,
    data_bits: u64,
    max_length_bits: u64,
) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_decompressor_decompress(
        handle_bits,
        data_bits,
        max_length_bits,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_eof(handle_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_decompressor_eof(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_needs_input(handle_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_decompressor_needs_input(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_unused_data(handle_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_decompressor_unused_data(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_decompressor_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_open(
    filename_bits: u64,
    mode_bits: u64,
    format_bits: u64,
    check_bits: u64,
    preset_bits: u64,
) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_file_open(
        filename_bits,
        mode_bits,
        format_bits,
        check_bits,
        preset_bits,
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_read(handle_bits: u64, size_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_file_read(handle_bits, size_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_file_write(handle_bits, data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_close(handle_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_file_close(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::lzma::molt_lzma_file_drop(handle_bits)
}
