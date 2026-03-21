//! Stub implementation of bz2 intrinsics when the `mod_compression` feature is
//! disabled.  Every public symbol is present so that `generated.rs` / `registry.rs`
//! always resolve, but the heavy `bzip2` crate is not linked.

#[allow(unused_imports)]
use crate::builtins::numbers::int_bits_from_i64;
use crate::*;

fn bz2_unavailable(_py: &PyToken<'_>) -> u64 {
    raise_exception::<u64>(
        _py,
        "RuntimeError",
        "bz2 module requires the 'mod_compression' feature",
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compress(_data_bits: u64, _compresslevel_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompress(_data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_new(_compresslevel_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_compress(_handle_bits: u64, _data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_flush(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_compressor_drop(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_new() -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_decompress(
    _handle_bits: u64,
    _data_bits: u64,
    _max_length_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_eof(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_needs_input(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_drop(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_decompressor_unused_data(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_open(
    _filename_bits: u64,
    _mode_bits: u64,
    _compresslevel_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_read(_handle_bits: u64, _size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_write(_handle_bits: u64, _data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { bz2_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_close(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bz2_file_drop(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}
