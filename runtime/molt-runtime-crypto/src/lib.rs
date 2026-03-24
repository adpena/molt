//! `molt-runtime-crypto` — Crypto module group for the Molt runtime.
//!
//! Isolates the `hashlib`, `hmac`, and `secrets` Python modules along with
//! their 8 optional native dependencies (sha3, blake2, md4, ripemd, hmac,
//! pbkdf2, scrypt, subtle) into a dedicated crate.
//!
//! This crate is an optional dependency of `molt-runtime`, gated behind the
//! `stdlib_crypto` feature flag.  When the feature is disabled the linker
//! can strip all crypto code from the final binary.

/// FFI bridge to molt-runtime internal functions (resolved at link time).
pub mod bridge;

pub mod hashlib;
pub mod hmac;
pub mod secrets;
