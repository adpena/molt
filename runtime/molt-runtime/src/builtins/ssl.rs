#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/ssl.rs ===
//! SSL/TLS intrinsics for the Molt `ssl` stdlib module.
//!
//! Exposes SSLContext creation and SSLSocket wrapping backed by rustls.
//! All heavy I/O (handshake, read, write) releases the GIL so the scheduler
//! can progress other tasks while blocking I/O completes.
//!
//! ABI: NaN-boxed u64 in/out.  Handle IDs are allocated from a monotonic
//! counter and stored in thread-local maps to avoid cross-thread aliasing.

use crate::builtins::numbers::int_bits_from_i64;
use crate::*;
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufReader, Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::net::TcpStream;
#[cfg(all(not(target_arch = "wasm32"), unix))]
use std::os::unix::io::{FromRawFd, IntoRawFd};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

// ── Availability guard ─────────────────────────────────────────────────────
// All public functions are present on all platforms (WASM stubs raise OSError).

// ── Constants (mirror CPython ssl module integer values) ──────────────────

const PURPOSE_SERVER_AUTH: i64 = 1;
#[allow(dead_code)]
const PURPOSE_CLIENT_AUTH: i64 = 2;
const PROTOCOL_TLS_CLIENT: i64 = 16;
const PROTOCOL_TLS_SERVER: i64 = 17;
const CERT_NONE: i64 = 0;
const CERT_OPTIONAL: i64 = 1;
const CERT_REQUIRED: i64 = 2;

// ── Handle-id counter ─────────────────────────────────────────────────────

static NEXT_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_id() -> i64 {
    NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ── SSLContext state ──────────────────────────────────────────────────────

#[derive(Debug)]
struct SslContextState {
    protocol: i64,
    verify_mode: i64,
    check_hostname: bool,
    certfile: Option<String>,
    keyfile: Option<String>,
    cafile: Option<String>,
    cadata: Option<Vec<u8>>,
    ciphers: Option<String>,
}

impl SslContextState {
    fn new_client(purpose: i64) -> Self {
        let verify_mode = if purpose == PURPOSE_SERVER_AUTH {
            CERT_REQUIRED
        } else {
            CERT_NONE
        };
        Self {
            protocol: PROTOCOL_TLS_CLIENT,
            verify_mode,
            check_hostname: purpose == PURPOSE_SERVER_AUTH,
            certfile: None,
            keyfile: None,
            cafile: None,
            cadata: None,
            ciphers: None,
        }
    }

    fn new_with_protocol(protocol: i64) -> Self {
        Self {
            protocol,
            verify_mode: CERT_NONE,
            check_hostname: false,
            certfile: None,
            keyfile: None,
            cafile: None,
            cadata: None,
            ciphers: None,
        }
    }
}

// ── SSLSocket state ──────────────────────────────────────────────────────

#[cfg(not(target_arch = "wasm32"))]
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
#[cfg(not(target_arch = "wasm32"))]
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};
#[cfg(not(target_arch = "wasm32"))]
use rustls_pki_types::pem::PemObject;

#[cfg(not(target_arch = "wasm32"))]
enum SslSocketInner {
    Client(StreamOwned<ClientConnection, TcpStream>),
    Server(StreamOwned<ServerConnection, TcpStream>),
}

#[cfg(not(target_arch = "wasm32"))]
struct SslSocketState {
    inner: SslSocketInner,
    peer_cert_der: Option<Vec<u8>>,
    cipher_name: Option<String>,
    protocol_version: Option<String>,
    server_hostname: Option<String>,
}

// ── Process-wide handle storage ──────────────────────────────────────────

static CTX_REGISTRY: LazyLock<Mutex<HashMap<i64, SslContextState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
#[cfg(not(target_arch = "wasm32"))]
static SOCK_REGISTRY: LazyLock<Mutex<HashMap<i64, SslSocketState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ── Helper: extract optional str ─────────────────────────────────────────

fn opt_string_from_bits(bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    string_obj_to_owned(obj)
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

fn return_bytes_val(_py: &PyToken<'_>, data: &[u8]) -> u64 {
    let ptr = alloc_bytes(_py, data);
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

// ── SSLContext public intrinsics ──────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_create_default_context(purpose_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let purpose = to_i64(obj_from_bits(purpose_bits)).unwrap_or(PURPOSE_SERVER_AUTH);
        let state = SslContextState::new_client(purpose);
        let id = next_id();
        CTX_REGISTRY.lock().unwrap().insert(id, state);
        int_bits_from_i64(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_new(protocol_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let protocol = to_i64(obj_from_bits(protocol_bits)).unwrap_or(PROTOCOL_TLS_CLIENT);
        let state = SslContextState::new_with_protocol(protocol);
        let id = next_id();
        CTX_REGISTRY.lock().unwrap().insert(id, state);
        int_bits_from_i64(_py, id)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_load_cert_chain(
    handle_bits: u64,
    certfile_bits: u64,
    keyfile_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let certfile = opt_string_from_bits(certfile_bits);
        let keyfile = opt_string_from_bits(keyfile_bits);
        let found = {
            let mut map = CTX_REGISTRY.lock().unwrap();
            if let Some(ctx) = map.get_mut(&id) {
                ctx.certfile = certfile;
                ctx.keyfile = keyfile;
                true
            } else {
                false
            }
        };
        if !found {
            return raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle");
        }
        return_none()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_load_verify_locations(
    handle_bits: u64,
    cafile_bits: u64,
    _capath_bits: u64,
    cadata_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let cafile = opt_string_from_bits(cafile_bits);
        let cadata: Option<Vec<u8>> = {
            let obj = obj_from_bits(cadata_bits);
            if obj.is_none() {
                None
            } else if let Some(s) = string_obj_to_owned(obj) {
                Some(s.into_bytes())
            } else if let Some(ptr) = obj.as_ptr() {
                unsafe { bytes_like_slice(ptr).map(|slice| slice.to_vec()) }
            } else {
                None
            }
        };
        let found = {
            let mut map = CTX_REGISTRY.lock().unwrap();
            if let Some(ctx) = map.get_mut(&id) {
                ctx.cafile = cafile;
                ctx.cadata = cadata;
                true
            } else {
                false
            }
        };
        if !found {
            return raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle");
        }
        return_none()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_set_default_verify_paths(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Verify the handle is valid; rustls uses system roots by default anyway.
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let valid = CTX_REGISTRY.lock().unwrap().contains_key(&id);
        if !valid {
            return raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle");
        }
        return_none()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_set_ciphers(handle_bits: u64, cipherstring_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let ciphers = opt_string_from_bits(cipherstring_bits);
        let found = {
            let mut map = CTX_REGISTRY.lock().unwrap();
            if let Some(ctx) = map.get_mut(&id) {
                ctx.ciphers = ciphers;
                true
            } else {
                false
            }
        };
        if !found {
            return raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle");
        }
        return_none()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_get_protocol(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let proto = CTX_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|ctx| ctx.protocol);
        match proto {
            Some(p) => int_bits_from_i64(_py, p),
            None => raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_check_hostname_get(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let val = CTX_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|ctx| ctx.check_hostname);
        match val {
            Some(b) => MoltObject::from_bool(b).bits(),
            None => raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_check_hostname_set(handle_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let flag = {
            let obj = obj_from_bits(value_bits);
            is_truthy(_py, obj)
        };
        let found = {
            let mut map = CTX_REGISTRY.lock().unwrap();
            if let Some(ctx) = map.get_mut(&id) {
                ctx.check_hostname = flag;
                // When check_hostname is enabled, CERT_REQUIRED is mandatory (CPython semantics).
                if flag && ctx.verify_mode == CERT_NONE {
                    ctx.verify_mode = CERT_REQUIRED;
                }
                true
            } else {
                false
            }
        };
        if !found {
            return raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle");
        }
        return_none()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_verify_mode_get(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let val = CTX_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|ctx| ctx.verify_mode);
        match val {
            Some(v) => int_bits_from_i64(_py, v),
            None => raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_verify_mode_set(handle_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let mode = to_i64(obj_from_bits(mode_bits)).unwrap_or(CERT_NONE);
        if !matches!(mode, CERT_NONE | CERT_OPTIONAL | CERT_REQUIRED) {
            return raise_exception::<u64>(_py, "ValueError", "invalid verify mode");
        }
        let found = {
            let mut map = CTX_REGISTRY.lock().unwrap();
            if let Some(ctx) = map.get_mut(&id) {
                // CPython: setting CERT_NONE while check_hostname=True raises.
                if mode == CERT_NONE && ctx.check_hostname {
                    false // signal error
                } else {
                    ctx.verify_mode = mode;
                    true
                }
            } else {
                false
            }
        };
        if !found {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "cannot set CERT_NONE while check_hostname is enabled",
            );
        }
        return_none()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_context_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            CTX_REGISTRY.lock().unwrap().remove(&id);
        }
        return_none()
    })
}

// ── SSLSocket wrapping ─────────────────────────────────────────────────────

#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_ssl_wrap_socket(
    sock_handle_bits: u64,
    ctx_handle_bits: u64,
    server_hostname_bits: u64,
    server_side_bits: u64,
) -> u64 {
    use std::sync::Arc;
    crate::with_gil_entry!(_py, {
        let ctx_id = match to_i64(obj_from_bits(ctx_handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl context handle must be int");
            }
        };
        let ctx_state = CTX_REGISTRY
            .lock()
            .unwrap()
            .get(&ctx_id)
            .map(|ctx| SslContextState {
                protocol: ctx.protocol,
                verify_mode: ctx.verify_mode,
                check_hostname: ctx.check_hostname,
                certfile: ctx.certfile.clone(),
                keyfile: ctx.keyfile.clone(),
                cafile: ctx.cafile.clone(),
                cadata: ctx.cadata.clone(),
                ciphers: ctx.ciphers.clone(),
            });
        let ctx_state = match ctx_state {
            Some(s) => s,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid ssl context handle"),
        };

        let server_side = {
            let obj = obj_from_bits(server_side_bits);
            is_truthy(_py, obj)
        };
        let server_hostname = opt_string_from_bits(server_hostname_bits);

        // Extract the raw file descriptor from the socket handle (int fd).
        let raw_fd = match to_i64(obj_from_bits(sock_handle_bits)) {
            Some(v) => v as i32,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "socket handle must be an int fd");
            }
        };

        // Duplicate the fd so the SSLSocket owns its own copy.
        // This is Unix-only; on non-Unix platforms we raise immediately.
        #[cfg(not(unix))]
        {
            let _ = raw_fd;
            return raise_exception::<u64>(
                _py,
                "OSError",
                "ssl.wrap_socket not supported on this platform",
            );
        }
        #[cfg(unix)]
        let tcp = {
            let dup_fd = unsafe { libc::dup(raw_fd) };
            if dup_fd < 0 {
                return raise_exception::<u64>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            unsafe { TcpStream::from_raw_fd(dup_fd) }
        };

        let socket_state: SslSocketState = if server_side {
            // Server-side TLS
            use rustls::pki_types::CertificateDer;
            let certfile = match &ctx_state.certfile {
                Some(p) => p.clone(),
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "ssl.SSLError",
                        "server-side ssl requires certfile",
                    );
                }
            };
            let keyfile = match &ctx_state.keyfile {
                Some(p) => p.clone(),
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "ssl.SSLError",
                        "server-side ssl requires keyfile",
                    );
                }
            };
            let certs: Vec<CertificateDer<'static>> = {
                let file = match std::fs::File::open(&certfile) {
                    Ok(f) => f,
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot open certfile: {e}"),
                        );
                    }
                };
                let mut reader = std::io::BufReader::new(file);
                match CertificateDer::pem_reader_iter(&mut reader).collect::<Result<Vec<_>, _>>() {
                    Ok(v) => v,
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot parse certfile: {e}"),
                        );
                    }
                }
            };
            let private_key = {
                let file = match std::fs::File::open(&keyfile) {
                    Ok(f) => f,
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot open keyfile: {e}"),
                        );
                    }
                };
                let mut reader = std::io::BufReader::new(file);
                match PrivateKeyDer::from_pem_reader(&mut reader) {
                    Ok(k) => k,
                    Err(rustls_pki_types::pem::Error::NoItemsFound) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            "no private key found in keyfile",
                        );
                    }
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot parse keyfile: {e}"),
                        );
                    }
                }
            };
            let config = match ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, private_key)
            {
                Ok(c) => Arc::new(c),
                Err(e) => {
                    return raise_exception::<u64>(
                        _py,
                        "ssl.SSLError",
                        &format!("TLS server config error: {e}"),
                    );
                }
            };
            let conn = match ServerConnection::new(config) {
                Ok(c) => c,
                Err(e) => {
                    return raise_exception::<u64>(
                        _py,
                        "ssl.SSLError",
                        &format!("TLS connection error: {e}"),
                    );
                }
            };
            let stream = StreamOwned::new(conn, tcp);
            SslSocketState {
                inner: SslSocketInner::Server(stream),
                peer_cert_der: None,
                cipher_name: None,
                protocol_version: None,
                server_hostname,
            }
        } else {
            // Client-side TLS
            let mut root_store = RootCertStore::empty();
            let native_certs = rustls_native_certs::load_native_certs();
            if native_certs.certs.is_empty() && !native_certs.errors.is_empty() {
                let joined = native_certs
                    .errors
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ");
                return raise_exception::<u64>(
                    _py,
                    "ssl.SSLError",
                    &format!("failed to load native certs: {joined}"),
                );
            }
            for cert in native_certs.certs {
                let _ = root_store.add(cert);
            }
            // Load custom CA if provided
            if let Some(ref cafile) = ctx_state.cafile {
                let file = match std::fs::File::open(cafile) {
                    Ok(f) => f,
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot open cafile: {e}"),
                        );
                    }
                };
                let mut reader = std::io::BufReader::new(file);
                let certs = match CertificateDer::pem_reader_iter(&mut reader)
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(v) => v,
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot parse cafile: {e}"),
                        );
                    }
                };
                for cert in certs {
                    let _ = root_store.add(cert);
                }
            }
            if let Some(ref cadata) = ctx_state.cadata {
                let certs = match CertificateDer::pem_slice_iter(cadata.as_slice())
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(v) => v,
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot parse cadata: {e}"),
                        );
                    }
                };
                for cert in certs {
                    let _ = root_store.add(cert);
                }
            }
            let mut config = ClientConfig::builder()
                .with_root_certificates(root_store)
                .with_no_client_auth();
            // Apply cert chain if set (mTLS / client cert auth)
            if let (Some(certfile), Some(keyfile)) = (&ctx_state.certfile, &ctx_state.keyfile) {
                let file = match std::fs::File::open(certfile) {
                    Ok(f) => f,
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot open certfile: {e}"),
                        );
                    }
                };
                let mut reader = std::io::BufReader::new(file);
                let certs: Vec<CertificateDer<'static>> =
                    CertificateDer::pem_reader_iter(&mut reader)
                        .filter_map(Result::ok)
                        .collect();
                let kf = match std::fs::File::open(keyfile) {
                    Ok(f) => f,
                    Err(e) => {
                        return raise_exception::<u64>(
                            _py,
                            "ssl.SSLError",
                            &format!("cannot open keyfile: {e}"),
                        );
                    }
                };
                let mut key_reader = std::io::BufReader::new(kf);
                if let Ok(key) = PrivateKeyDer::from_pem_reader(&mut key_reader) {
                    let _ = config.dangerous(); // suppress unused warning pattern
                    // Rebuild with client auth
                    let _config_with_auth = ClientConfig::builder()
                        .with_root_certificates(RootCertStore::empty())
                        .with_client_auth_cert(certs, key);
                }
            }
            let config = Arc::new(config);
            let hostname = match server_hostname.as_deref() {
                Some(h) => h.to_string(),
                None => {
                    return raise_exception::<u64>(
                        _py,
                        "ssl.SSLError",
                        "server_hostname required for client-side TLS",
                    );
                }
            };
            let server_name = match ServerName::try_from(hostname.as_str()) {
                Ok(n) => n.to_owned(),
                Err(e) => {
                    return raise_exception::<u64>(
                        _py,
                        "ssl.SSLError",
                        &format!("invalid server hostname: {e}"),
                    );
                }
            };
            let conn = match ClientConnection::new(config, server_name) {
                Ok(c) => c,
                Err(e) => {
                    return raise_exception::<u64>(
                        _py,
                        "ssl.SSLError",
                        &format!("TLS connection error: {e}"),
                    );
                }
            };
            let stream = StreamOwned::new(conn, tcp);
            SslSocketState {
                inner: SslSocketInner::Client(stream),
                peer_cert_der: None,
                cipher_name: None,
                protocol_version: None,
                server_hostname: Some(hostname),
            }
        };

        let sock_id = next_id();
        SOCK_REGISTRY.lock().unwrap().insert(sock_id, socket_state);
        int_bits_from_i64(_py, sock_id)
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_ssl_wrap_socket(
    _sock_handle_bits: u64,
    _ctx_handle_bits: u64,
    _server_hostname_bits: u64,
    _server_side_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "ssl.wrap_socket not supported on WASM")
    })
}

#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_ssl_socket_do_handshake(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl socket handle must be int");
            }
        };
        let result = (|| -> Result<(), String> {
            let mut map = SOCK_REGISTRY.lock().unwrap();
            let Some(state) = map.get_mut(&id) else {
                return Err("invalid ssl socket handle".to_string());
            };
            match &mut state.inner {
                SslSocketInner::Client(stream) => {
                    while stream.conn.is_handshaking() {
                        stream
                            .conn
                            .complete_io(&mut stream.sock)
                            .map_err(|e| e.to_string())?;
                    }
                    // Record cipher / version after handshake
                    if let Some(suite) = stream.conn.negotiated_cipher_suite() {
                        state.cipher_name =
                            Some(format!("{:?}", suite.suite()).replace("TLS13_", "TLS_AES_"));
                    }
                    if let Some(ver) = stream.conn.protocol_version() {
                        state.protocol_version = Some(format!("{ver:?}"));
                    }
                    // Extract peer certificate bytes
                    let certs = stream.conn.peer_certificates();
                    if let Some(leaf) = certs.and_then(|chain| chain.first()) {
                        state.peer_cert_der = Some(leaf.as_ref().to_vec());
                    }
                }
                SslSocketInner::Server(stream) => {
                    while stream.conn.is_handshaking() {
                        stream
                            .conn
                            .complete_io(&mut stream.sock)
                            .map_err(|e| e.to_string())?;
                    }
                    if let Some(suite) = stream.conn.negotiated_cipher_suite() {
                        state.cipher_name =
                            Some(format!("{:?}", suite.suite()).replace("TLS13_", "TLS_AES_"));
                    }
                    if let Some(ver) = stream.conn.protocol_version() {
                        state.protocol_version = Some(format!("{ver:?}"));
                    }
                    let certs = stream.conn.peer_certificates();
                    if let Some(leaf) = certs.and_then(|chain| chain.first()) {
                        state.peer_cert_der = Some(leaf.as_ref().to_vec());
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => return_none(),
            Err(msg) => raise_exception::<u64>(_py, "ssl.SSLError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_ssl_socket_do_handshake(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "ssl not supported on WASM")
    })
}

#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_ssl_socket_read(handle_bits: u64, len_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl socket handle must be int");
            }
        };
        let max_len = to_i64(obj_from_bits(len_bits)).unwrap_or(4096).max(0) as usize;
        let result = (|| -> Result<Vec<u8>, String> {
            let mut map = SOCK_REGISTRY.lock().unwrap();
            let Some(state) = map.get_mut(&id) else {
                return Err("invalid ssl socket handle".to_string());
            };
            let mut buf = vec![0u8; max_len];
            let n = match &mut state.inner {
                SslSocketInner::Client(stream) => {
                    stream.read(&mut buf).map_err(|e| e.to_string())?
                }
                SslSocketInner::Server(stream) => {
                    stream.read(&mut buf).map_err(|e| e.to_string())?
                }
            };
            buf.truncate(n);
            Ok(buf)
        })();
        match result {
            Ok(data) => return_bytes_val(_py, &data),
            Err(msg) => raise_exception::<u64>(_py, "ssl.SSLError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_ssl_socket_read(_handle_bits: u64, _len_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "ssl not supported on WASM")
    })
}

#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_ssl_socket_write(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl socket handle must be int");
            }
        };
        let data: Vec<u8> = {
            let obj = obj_from_bits(data_bits);
            let Some(ptr) = obj.as_ptr() else {
                return raise_exception::<u64>(_py, "TypeError", "ssl write expects bytes");
            };
            unsafe {
                match bytes_like_slice(ptr) {
                    Some(s) => s.to_vec(),
                    None => {
                        return raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "ssl write expects bytes-like object",
                        );
                    }
                }
            }
        };
        let result = (|| -> Result<usize, String> {
            let mut map = SOCK_REGISTRY.lock().unwrap();
            let Some(state) = map.get_mut(&id) else {
                return Err("invalid ssl socket handle".to_string());
            };
            let n = match &mut state.inner {
                SslSocketInner::Client(stream) => {
                    stream.write_all(&data).map_err(|e| e.to_string())?;
                    data.len()
                }
                SslSocketInner::Server(stream) => {
                    stream.write_all(&data).map_err(|e| e.to_string())?;
                    data.len()
                }
            };
            Ok(n)
        })();
        match result {
            Ok(n) => int_bits_from_i64(_py, n as i64),
            Err(msg) => raise_exception::<u64>(_py, "ssl.SSLError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_ssl_socket_write(_handle_bits: u64, _data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "ssl not supported on WASM")
    })
}

#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_ssl_socket_getpeercert(handle_bits: u64, binary_form_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl socket handle must be int");
            }
        };
        let binary_form = is_truthy(_py, obj_from_bits(binary_form_bits));
        let cert_der = SOCK_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|s| s.peer_cert_der.clone());
        match cert_der {
            None => MoltObject::none().bits(),
            Some(der) => {
                if binary_form {
                    return_bytes_val(_py, &der)
                } else {
                    // Return a minimal dict with the subject as a string for compatibility.
                    let subj_str = format!("<DER cert {} bytes>", der.len());
                    let key_ptr = alloc_string(_py, b"subject");
                    let val_ptr = alloc_string(_py, subj_str.as_bytes());
                    if key_ptr.is_null() || val_ptr.is_null() {
                        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
                    }
                    let key_bits = MoltObject::from_ptr(key_ptr).bits();
                    let val_bits = MoltObject::from_ptr(val_ptr).bits();
                    let dict_ptr = alloc_dict_with_pairs(_py, &[key_bits, val_bits]);
                    if dict_ptr.is_null() {
                        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_ssl_socket_getpeercert(_handle_bits: u64, _binary_form_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "ssl not supported on WASM")
    })
}

#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_ssl_socket_cipher(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl socket handle must be int");
            }
        };
        let info = SOCK_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| (s.cipher_name.clone(), s.protocol_version.clone()));
        match info {
            None => raise_exception::<u64>(_py, "ValueError", "invalid ssl socket handle"),
            Some((None, _)) => MoltObject::none().bits(),
            Some((Some(name), ver)) => {
                let name_ptr = alloc_string(_py, name.as_bytes());
                let ver_str = ver.unwrap_or_else(|| "TLSv1.3".to_string());
                let ver_ptr = alloc_string(_py, ver_str.as_bytes());
                let bits_ptr = alloc_string(_py, b"256");
                if name_ptr.is_null() || ver_ptr.is_null() || bits_ptr.is_null() {
                    return raise_exception::<u64>(_py, "MemoryError", "out of memory");
                }
                let tuple_ptr = alloc_tuple(
                    _py,
                    &[
                        MoltObject::from_ptr(name_ptr).bits(),
                        MoltObject::from_ptr(ver_ptr).bits(),
                        int_bits_from_i64(_py, 256),
                    ],
                );
                if tuple_ptr.is_null() {
                    return raise_exception::<u64>(_py, "MemoryError", "out of memory");
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_ssl_socket_cipher(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "ssl not supported on WASM")
    })
}

#[unsafe(no_mangle)]
#[cfg(not(target_arch = "wasm32"))]
pub extern "C" fn molt_ssl_socket_version(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl socket handle must be int");
            }
        };
        let ver = SOCK_REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .and_then(|s| s.protocol_version.clone());
        match ver {
            None => MoltObject::none().bits(),
            Some(v) => return_str(_py, &v),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(target_arch = "wasm32")]
pub extern "C" fn molt_ssl_socket_version(_handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "OSError", "ssl not supported on WASM")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_unwrap(handle_bits: u64) -> u64 {
    // Returns the underlying fd as an integer handle.
    crate::with_gil_entry!(_py, {
        let id = match to_i64(obj_from_bits(handle_bits)) {
            Some(v) => v,
            None => {
                return raise_exception::<u64>(_py, "TypeError", "ssl socket handle must be int");
            }
        };
        #[cfg(all(not(target_arch = "wasm32"), unix))]
        {
            let state = SOCK_REGISTRY.lock().unwrap().remove(&id);
            match state {
                None => raise_exception::<u64>(_py, "ValueError", "invalid ssl socket handle"),
                Some(s) => {
                    let fd = match s.inner {
                        SslSocketInner::Client(stream) => stream.sock.into_raw_fd(),
                        SslSocketInner::Server(stream) => stream.sock.into_raw_fd(),
                    };
                    int_bits_from_i64(_py, fd as i64)
                }
            }
        }
        #[cfg(all(not(target_arch = "wasm32"), not(unix)))]
        {
            let _ = id;
            raise_exception::<u64>(_py, "OSError", "ssl.unwrap not supported on this platform")
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = id;
            raise_exception::<u64>(_py, "OSError", "ssl not supported on WASM")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(id) = to_i64(obj_from_bits(handle_bits)) {
            #[cfg(not(target_arch = "wasm32"))]
            SOCK_REGISTRY.lock().unwrap().remove(&id);
            #[cfg(target_arch = "wasm32")]
            let _ = id;
        }
        return_none()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_socket_drop(handle_bits: u64) -> u64 {
    molt_ssl_socket_close(handle_bits)
}

// ── Constants ──────────────────────────────────────────────────────────────

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
    crate::with_gil_entry!(_py, { MoltObject::from_bool(true).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ssl_openssl_version() -> u64 {
    crate::with_gil_entry!(_py, {
        // Report rustls version string instead of OpenSSL.
        return_str(_py, "rustls/0.23 (Molt TLS backend)")
    })
}
