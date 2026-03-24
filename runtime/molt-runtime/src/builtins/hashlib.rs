//! Re-export bridge: delegates to `molt_runtime_crypto::hashlib`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_crypto` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_new(name_bits: u64, data_bits: u64, options_bits: u64) -> u64 {
    molt_runtime_crypto::hashlib::molt_hash_new(name_bits, data_bits, options_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_update(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_crypto::hashlib::molt_hash_update(handle_bits, data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_copy(handle_bits: u64) -> u64 {
    molt_runtime_crypto::hashlib::molt_hash_copy(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_digest(handle_bits: u64, length_bits: u64) -> u64 {
    molt_runtime_crypto::hashlib::molt_hash_digest(handle_bits, length_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_drop(handle_bits: u64) -> u64 {
    molt_runtime_crypto::hashlib::molt_hash_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pbkdf2_hmac(name_bits: u64, password_bits: u64, salt_bits: u64, iterations_bits: u64, dklen_bits: u64) -> u64 {
    molt_runtime_crypto::hashlib::molt_pbkdf2_hmac(name_bits, password_bits, salt_bits, iterations_bits, dklen_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_scrypt(password_bits: u64, salt_bits: u64, n_bits: u64, r_bits: u64, p_bits: u64, maxmem_bits: u64, dklen_bits: u64) -> u64 {
    molt_runtime_crypto::hashlib::molt_scrypt(password_bits, salt_bits, n_bits, r_bits, p_bits, maxmem_bits, dklen_bits)
}
