//! Re-export bridge: delegates to `molt_runtime_crypto::hmac`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_crypto` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_new(key_bits: u64, msg_bits: u64, name_bits: u64, options_bits: u64) -> u64 {
    molt_runtime_crypto::hmac::molt_hmac_new(key_bits, msg_bits, name_bits, options_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_update(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_crypto::hmac::molt_hmac_update(handle_bits, data_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_copy(handle_bits: u64) -> u64 {
    molt_runtime_crypto::hmac::molt_hmac_copy(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_digest(handle_bits: u64) -> u64 {
    molt_runtime_crypto::hmac::molt_hmac_digest(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_drop(handle_bits: u64) -> u64 {
    molt_runtime_crypto::hmac::molt_hmac_drop(handle_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_compare_digest(a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_crypto::hmac::molt_compare_digest(a_bits, b_bits)
}
