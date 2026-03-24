//! Re-export bridge: delegates to `molt_runtime_compression::tarfile`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_compression` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_open(name_bits: u64, mode_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_open(name_bits, mode_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_getnames(handle_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_getnames(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_getmembers(handle_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_getmembers(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_extractall(handle_bits: u64, path_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_extractall(handle_bits, path_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_extract(handle_bits: u64, member_bits: u64, path_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_extract(handle_bits, member_bits, path_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_extractfile(handle_bits: u64, member_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_extractfile(handle_bits, member_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_add(handle_bits: u64, name_bits: u64, arcname_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_add(handle_bits, name_bits, arcname_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_close(handle_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_close(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_drop(handle_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tarfile_is_tarfile(name_bits: u64) -> u64 {
    molt_runtime_compression::tarfile::molt_tarfile_is_tarfile(name_bits)
}
