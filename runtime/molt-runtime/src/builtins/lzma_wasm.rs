use crate::BuiltinExc;
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
        BuiltinExc::RuntimeError,
        "lzma intrinsics are unavailable on wasm32-wasip1",
    )
}

pub(crate) fn molt_lzma_format_auto_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, FORMAT_AUTO) })
}

pub(crate) fn molt_lzma_format_xz_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, FORMAT_XZ) })
}

pub(crate) fn molt_lzma_format_alone_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, FORMAT_ALONE) })
}

pub(crate) fn molt_lzma_format_raw_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, FORMAT_RAW) })
}

pub(crate) fn molt_lzma_check_none_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, CHECK_NONE) })
}

pub(crate) fn molt_lzma_check_crc32_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, CHECK_CRC32) })
}

pub(crate) fn molt_lzma_check_crc64_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, CHECK_CRC64) })
}

pub(crate) fn molt_lzma_check_sha256_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, CHECK_SHA256) })
}

pub(crate) fn molt_lzma_preset_default_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, PRESET_DEFAULT) })
}

pub(crate) fn molt_lzma_preset_extreme_attached_body(_py: &crate::PyToken<'_>) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { int_bits_from_i64(_py, PRESET_EXTREME) })
}

pub(crate) fn molt_lzma_compress_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _data_bits: u64,
    _format_bits: u64,
    _check_bits: u64,
    _preset_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_decompress_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _data_bits: u64,
    _format_bits: u64,
    _memlimit_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_compressor_new_attached_body(
    _py: &crate::PyToken<'_>,
    _format_bits: u64,
    _check_bits: u64,
    _preset_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_compressor_compress_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
    _data_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_compressor_flush_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_decompressor_new_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _format_bits: u64,
    _memlimit_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_decompressor_decompress_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
    _data_bits: u64,
    _max_length_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_decompressor_eof_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_decompressor_needs_input_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_decompressor_unused_data_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_file_open_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _format_bits: u64,
    _check_bits: u64,
    _preset_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_file_read_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
    _size_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_file_write_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
    _data_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { lzma_unavailable(_py) })
}

pub(crate) fn molt_lzma_file_close_attached_body(
    _py: &crate::PyToken<'_>,
    _error_class: u64,
    _handle_bits: u64,
) -> u64 {
    crate::with_attached_entry_nopanic!(_py, _py, { MoltObject::none().bits() })
}

include!("../attached_exports_generated/builtins/lzma_wasm.rs");
