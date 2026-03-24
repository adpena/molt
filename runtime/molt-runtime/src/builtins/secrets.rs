//! Re-export bridge: delegates to `molt_runtime_crypto::secrets`.
//!
//! The canonical implementation lives in the extracted `molt_runtime_crypto` crate.
//! This file provides `#[unsafe(no_mangle)]` entry points so the linker
//! exports them with the expected symbol names.

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_token_bytes(nbytes_bits: u64) -> u64 {
    molt_runtime_crypto::secrets::molt_secrets_token_bytes(nbytes_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_token_hex(nbytes_bits: u64) -> u64 {
    molt_runtime_crypto::secrets::molt_secrets_token_hex(nbytes_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_token_urlsafe(nbytes_bits: u64) -> u64 {
    molt_runtime_crypto::secrets::molt_secrets_token_urlsafe(nbytes_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_randbits(k_bits: u64) -> u64 {
    molt_runtime_crypto::secrets::molt_secrets_randbits(k_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_compare_digest(a_bits: u64, b_bits: u64) -> u64 {
    molt_runtime_crypto::secrets::molt_secrets_compare_digest(a_bits, b_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_choice(seq_bits: u64) -> u64 {
    molt_runtime_crypto::secrets::molt_secrets_choice(seq_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_below(upper_bits: u64) -> u64 {
    molt_runtime_crypto::secrets::molt_secrets_below(upper_bits)
}
