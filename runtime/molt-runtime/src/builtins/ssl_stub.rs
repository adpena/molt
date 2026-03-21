//! Stub implementation of SSL/TLS intrinsics when the `mod_tls` feature is
//! disabled.  All symbols are present so `generated.rs` / `registry.rs` always
//! resolve.  Constants are provided (they are pure integers), but any operation
//! that would require the `rustls` crate raises a runtime error.

#[allow(unused_imports)]
use crate::builtins::numbers::int_bits_from_i64;
use crate::*;

// ── Constants (same values as the real module) ───────────────────────────────

const PROTOCOL_TLS_CLIENT: i64 = 16;
const PROTOCOL_TLS_SERVER: i64 = 17;
const CERT_NONE: i64 = 0;
const CERT_OPTIONAL: i64 = 1;
const CERT_REQUIRED: i64 = 2;

fn ssl_unavailable(_py: &PyToken<'_>) -> u64 {
    raise_exception::<u64>(
        _py,
        "RuntimeError",
        "ssl module requires the 'mod_tls' feature",
    )
}

fn return_none() -> u64 {
    MoltObject::none().bits()
}

fn return_str(_py: &PyToken<'_>, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ── Context functions ────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_create_default_context(_purpose_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_new(_protocol_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_load_cert_chain(
    _handle_bits: u64,
    _certfile_bits: u64,
    _keyfile_bits: u64,
    _password_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_load_verify_locations(
    _handle_bits: u64,
    _cafile_bits: u64,
    _capath_bits: u64,
    _cadata_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_set_default_verify_paths(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_set_ciphers(
    _handle_bits: u64,
    _cipherstring_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_get_protocol(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_check_hostname_get(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_check_hostname_set(
    _handle_bits: u64,
    _value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_verify_mode_get(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_verify_mode_set(
    _handle_bits: u64,
    _mode_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_drop(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { return_none() })
}

// ── Socket functions ─────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_wrap_socket(
    _sock_handle_bits: u64,
    _ctx_handle_bits: u64,
    _server_hostname_bits: u64,
    _server_side_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_do_handshake(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_read(_handle_bits: u64, _len_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_write(_handle_bits: u64, _data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_getpeercert(
    _handle_bits: u64,
    _binary_form_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_cipher(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_version(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_unwrap(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { ssl_unavailable(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_close(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { return_none() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_drop(_handle_bits: u64) -> u64 {
    molt_ssl_socket_close(_handle_bits)
}

// ── Constants ────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_protocol_tls_client() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, PROTOCOL_TLS_CLIENT) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_protocol_tls_server() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, PROTOCOL_TLS_SERVER) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_cert_none() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CERT_NONE) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_cert_optional() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CERT_OPTIONAL) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_cert_required() -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, CERT_REQUIRED) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_has_sni() -> u64 {
    // Report false when TLS is not available.
    crate::with_gil_entry!(_py, { MoltObject::from_bool(false).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_openssl_version() -> u64 {
    crate::with_gil_entry!(_py, {
        return_str(_py, "ssl unavailable (mod_tls feature disabled)")
    })
}
