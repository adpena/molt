//! Re-export bridge: delegates to `molt_runtime_compression::zipfile`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_compression` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_open(path_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_compression::zipfile::molt_zipfile_open(path_bits, mode_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_close(handle_bits: u64) -> u64 {
    molt_runtime_compression::zipfile::molt_zipfile_close(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_writestr(handle_bits: u64, name_bits: u64, data_bits: u64, method_bits: u64) -> u64 {
    molt_runtime_compression::zipfile::molt_zipfile_writestr(handle_bits, name_bits, data_bits, method_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_namelist(handle_bits: u64) -> u64 {
    molt_runtime_compression::zipfile::molt_zipfile_namelist(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zipfile_read(handle_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_compression::zipfile::molt_zipfile_read(handle_bits, name_bits)
}
