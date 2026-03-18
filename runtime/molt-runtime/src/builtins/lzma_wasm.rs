use crate::builtins::numbers::int_bits_from_i64;
use crate::*;

// Keep constant values aligned with native targets for API compatibility.
pub(crate) const FORMAT_AUTO: i64 = 0;
pub(crate) const FORMAT_XZ: i64 = 1;
pub(crate) const FORMAT_ALONE: i64 = 2;
pub(crate) const FORMAT_RAW: i64 = 3;

pub(crate) const CHECK_NONE: i64 = 0;
pub(crate) const CHECK_CRC32: i64 = 1;
pub(crate) const CHECK_CRC64: i64 = 4;
pub(crate) const CHECK_SHA256: i64 = 10;

pub(crate) const PRESET_DEFAULT: i64 = 6;
pub(crate) const PRESET_EXTREME: i64 = 1 << 31;

fn lzma_unavailable(_py: &PyToken<'_>) -> u64 {
    raise_exception::<u64>(
        _py,
        "RuntimeError",
        "lzma intrinsics are unavailable on wasm32-wasip1",
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_auto() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, FORMAT_AUTO) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_xz() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, FORMAT_XZ) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_alone() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, FORMAT_ALONE) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_format_raw() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, FORMAT_RAW) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_none() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CHECK_NONE) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_crc32() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CHECK_CRC32) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_crc64() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CHECK_CRC64) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_check_sha256() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CHECK_SHA256) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_preset_default() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, PRESET_DEFAULT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_preset_extreme() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, PRESET_EXTREME) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compress(
    _data_bits: u64,
    _format_bits: u64,
    _check_bits: u64,
    _preset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompress(
    _data_bits: u64,
    _format_bits: u64,
    _memlimit_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_new(
    _format_bits: u64,
    _check_bits: u64,
    _preset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_compress(_handle_bits: u64, _data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_flush(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_compressor_drop(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_new(_format_bits: u64, _memlimit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_decompress(
    _handle_bits: u64,
    _data_bits: u64,
    _max_length_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_eof(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_needs_input(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_unused_data(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_decompressor_drop(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_open(
    _filename_bits: u64,
    _mode_bits: u64,
    _format_bits: u64,
    _check_bits: u64,
    _preset_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_read(_handle_bits: u64, _size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_write(_handle_bits: u64, _data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { lzma_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_close(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lzma_file_drop(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}
