#![cfg(molt_has_net_io)]

use crate::BuiltinExc;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::io::{FromRawFd, RawFd};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(windows)]
use std::os::windows::io::{FromRawSocket, RawSocket};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(unix)]
use crate::libc_compat as libc;
use molt_runtime_core::OwnedRuntimeValue;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};
use webpki_roots::TLS_SERVER_ROOTS;

use super::super::stream::molt_stream_new_with_io_hooks;
use crate::{
    GilReleaseGuard, MoltObject, PyToken, alloc_bytes, dec_ref_bits, exception_pending,
    intern_static_name, is_missing_bits, missing_bits, molt_getattr_builtin, obj_from_bits,
    pending_bits_i64, raise_exception, string_obj_to_owned, to_i64,
};

struct NativeTlsStream {
    stream: NativeTlsEndpoint,
    pending_write: Vec<u8>,
    pending_write_offset: usize,
    closed: bool,
}

#[cfg(molt_has_net_io)]
enum NativeTlsEndpoint {
    ClientTcp(StreamOwned<ClientConnection, TcpStream>),
    #[cfg(unix)]
    ClientUnix(StreamOwned<ClientConnection, UnixStream>),
    ServerTcp(StreamOwned<ServerConnection, TcpStream>),
    #[cfg(unix)]
    ServerUnix(StreamOwned<ServerConnection, UnixStream>),
}

fn tls_is_would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

#[cfg(molt_has_net_io)]
fn tls_flush_pending_bytes(state: &mut NativeTlsStream) -> Result<bool, std::io::Error> {
    while state.pending_write_offset < state.pending_write.len() {
        let written = tls_endpoint_write(
            &mut state.stream,
            &state.pending_write[state.pending_write_offset..],
        )?;
        if written == 0 {
            return Ok(false);
        }
        state.pending_write_offset = state.pending_write_offset.saturating_add(written);
    }
    state.pending_write.clear();
    state.pending_write_offset = 0;
    Ok(true)
}

#[cfg(molt_has_net_io)]
fn tls_endpoint_write(
    endpoint: &mut NativeTlsEndpoint,
    payload: &[u8],
) -> Result<usize, std::io::Error> {
    match endpoint {
        NativeTlsEndpoint::ClientTcp(stream) => stream.write(payload),
        #[cfg(unix)]
        NativeTlsEndpoint::ClientUnix(stream) => stream.write(payload),
        NativeTlsEndpoint::ServerTcp(stream) => stream.write(payload),
        #[cfg(unix)]
        NativeTlsEndpoint::ServerUnix(stream) => stream.write(payload),
    }
}

#[cfg(molt_has_net_io)]
fn tls_endpoint_read(
    endpoint: &mut NativeTlsEndpoint,
    payload: &mut [u8],
) -> Result<usize, std::io::Error> {
    match endpoint {
        NativeTlsEndpoint::ClientTcp(stream) => stream.read(payload),
        #[cfg(unix)]
        NativeTlsEndpoint::ClientUnix(stream) => stream.read(payload),
        NativeTlsEndpoint::ServerTcp(stream) => stream.read(payload),
        #[cfg(unix)]
        NativeTlsEndpoint::ServerUnix(stream) => stream.read(payload),
    }
}

#[cfg(molt_has_net_io)]
fn tls_endpoint_set_nonblocking(
    endpoint: &mut NativeTlsEndpoint,
    nonblocking: bool,
) -> Result<(), std::io::Error> {
    match endpoint {
        NativeTlsEndpoint::ClientTcp(stream) => stream.sock.set_nonblocking(nonblocking),
        #[cfg(unix)]
        NativeTlsEndpoint::ClientUnix(stream) => stream.sock.set_nonblocking(nonblocking),
        NativeTlsEndpoint::ServerTcp(stream) => stream.sock.set_nonblocking(nonblocking),
        #[cfg(unix)]
        NativeTlsEndpoint::ServerUnix(stream) => stream.sock.set_nonblocking(nonblocking),
    }
}

#[cfg(molt_has_net_io)]
fn tls_endpoint_shutdown(endpoint: &mut NativeTlsEndpoint) -> Result<(), std::io::Error> {
    match endpoint {
        NativeTlsEndpoint::ClientTcp(stream) => stream.sock.shutdown(std::net::Shutdown::Both),
        #[cfg(unix)]
        NativeTlsEndpoint::ClientUnix(stream) => stream.sock.shutdown(std::net::Shutdown::Both),
        NativeTlsEndpoint::ServerTcp(stream) => stream.sock.shutdown(std::net::Shutdown::Both),
        #[cfg(unix)]
        NativeTlsEndpoint::ServerUnix(stream) => stream.sock.shutdown(std::net::Shutdown::Both),
    }
}

#[cfg(molt_has_net_io)]
fn tls_wrap_endpoint_native(mut endpoint: NativeTlsEndpoint) -> *mut u8 {
    if tls_endpoint_set_nonblocking(&mut endpoint, true).is_err() {
        return std::ptr::null_mut();
    }
    let ctx_ptr = Box::into_raw(Box::new(Mutex::new(NativeTlsStream {
        stream: endpoint,
        pending_write: Vec::new(),
        pending_write_offset: 0,
        closed: false,
    }))) as *mut u8;
    let stream_ptr = molt_stream_new_with_io_hooks(
        tls_stream_send_native_hook as *const () as usize,
        tls_stream_recv_native_hook as *const () as usize,
        tls_stream_close_native_hook as *const () as usize,
        ctx_ptr,
    );
    if stream_ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ctx_ptr as *mut Mutex<NativeTlsStream>));
        }
    }
    stream_ptr
}

#[cfg(molt_has_net_io)]
extern "C" fn tls_stream_send_native_hook(ctx: *mut u8, data_ptr: *const u8, len: usize) -> i64 {
    if ctx.is_null() {
        return pending_bits_i64();
    }
    if data_ptr.is_null() && len != 0 {
        return MoltObject::none().bits() as i64;
    }
    let payload = unsafe { std::slice::from_raw_parts(data_ptr, len) };
    let mutex = unsafe { &*(ctx as *mut Mutex<NativeTlsStream>) };
    let mut state = mutex.lock().unwrap();
    if state.closed {
        return MoltObject::none().bits() as i64;
    }
    if !state.pending_write.is_empty() {
        match tls_flush_pending_bytes(&mut state) {
            Ok(true) => {}
            Ok(false) => return pending_bits_i64(),
            Err(err) if tls_is_would_block(&err) => return pending_bits_i64(),
            Err(_) => {
                state.closed = true;
                return MoltObject::none().bits() as i64;
            }
        }
    }
    if payload.is_empty() {
        return 0;
    }
    match tls_endpoint_write(&mut state.stream, payload) {
        Ok(written) if written == payload.len() => 0,
        Ok(written) => {
            state.pending_write.clear();
            state.pending_write.extend_from_slice(&payload[written..]);
            state.pending_write_offset = 0;
            pending_bits_i64()
        }
        Err(err) if tls_is_would_block(&err) => pending_bits_i64(),
        Err(_) => {
            state.closed = true;
            MoltObject::none().bits() as i64
        }
    }
}

#[cfg(molt_has_net_io)]
extern "C" fn tls_stream_recv_native_hook(ctx: *mut u8) -> i64 {
    if ctx.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let mutex = unsafe { &*(ctx as *mut Mutex<NativeTlsStream>) };
    let mut state = mutex.lock().unwrap();
    if state.closed {
        return MoltObject::none().bits() as i64;
    }
    let mut buf = [0u8; 64 * 1024];
    match tls_endpoint_read(&mut state.stream, &mut buf) {
        Ok(0) => {
            state.closed = true;
            MoltObject::none().bits() as i64
        }
        Ok(n) => {
            let ptr = alloc_bytes(&crate::GilGuard::new().token(), &buf[..n]);
            if ptr.is_null() {
                MoltObject::none().bits() as i64
            } else {
                MoltObject::from_ptr(ptr).bits() as i64
            }
        }
        Err(err) if tls_is_would_block(&err) => pending_bits_i64(),
        Err(_) => {
            state.closed = true;
            MoltObject::none().bits() as i64
        }
    }
}

#[cfg(molt_has_net_io)]
extern "C" fn tls_stream_close_native_hook(ctx: *mut u8) {
    if ctx.is_null() {
        return;
    }
    let mutex = unsafe { Box::from_raw(ctx as *mut Mutex<NativeTlsStream>) };
    let mut state = mutex.lock().unwrap();
    state.closed = true;
    let _ = tls_endpoint_shutdown(&mut state.stream);
}

#[cfg(molt_has_net_io)]
fn tls_client_wrap_stream_native(tcp: TcpStream, server_name: &str) -> *mut u8 {
    let mut roots = RootCertStore::empty();
    roots.extend(TLS_SERVER_ROOTS.iter().cloned());
    let config = Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let server_name: ServerName<'static> = match ServerName::try_from(server_name.to_owned()) {
        Ok(value) => value,
        Err(_) => return std::ptr::null_mut(),
    };
    let stream = {
        let _release = GilReleaseGuard::suspend();
        let _ = tcp.set_nodelay(true);
        let conn = match ClientConnection::new(config, server_name) {
            Ok(value) => value,
            Err(_) => return std::ptr::null_mut(),
        };
        let mut stream = StreamOwned::new(conn, tcp);
        while stream.conn.is_handshaking() {
            match stream.conn.complete_io(&mut stream.sock) {
                Ok(_) => {}
                Err(_) => return std::ptr::null_mut(),
            }
        }
        stream
    };
    tls_wrap_endpoint_native(NativeTlsEndpoint::ClientTcp(stream))
}

#[cfg(all(molt_has_net_io, unix))]
fn tls_client_wrap_unix_stream_native(unix: UnixStream, server_name: &str) -> *mut u8 {
    let mut roots = RootCertStore::empty();
    roots.extend(TLS_SERVER_ROOTS.iter().cloned());
    let config = Arc::new(
        ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let server_name: ServerName<'static> = match ServerName::try_from(server_name.to_owned()) {
        Ok(value) => value,
        Err(_) => return std::ptr::null_mut(),
    };
    let stream = {
        let _release = GilReleaseGuard::suspend();
        let conn = match ClientConnection::new(config, server_name) {
            Ok(value) => value,
            Err(_) => return std::ptr::null_mut(),
        };
        let mut stream = StreamOwned::new(conn, unix);
        while stream.conn.is_handshaking() {
            match stream.conn.complete_io(&mut stream.sock) {
                Ok(_) => {}
                Err(_) => return std::ptr::null_mut(),
            }
        }
        stream
    };
    tls_wrap_endpoint_native(NativeTlsEndpoint::ClientUnix(stream))
}

#[cfg(molt_has_net_io)]
#[derive(Debug)]
pub(super) enum TlsConnectError {
    PendingPython,
    Io(std::io::Error),
}

fn tls_resolver_protocol_error(py: &PyToken<'_>, message: &str) -> TlsConnectError {
    if !exception_pending(py) {
        raise_exception::<u64>(py, BuiltinExc::RuntimeError, message);
    }
    TlsConnectError::PendingPython
}

fn tls_addrinfo_sequence<'a, 'py>(
    py: &'a PyToken<'py>,
    bits: u64,
) -> Result<crate::object::seq_access::PinnedSequenceSnapshot<'a, 'py>, TlsConnectError> {
    let Some(pointer) = obj_from_bits(bits).as_ptr() else {
        return Err(tls_resolver_protocol_error(
            py,
            "TLS resolver returned an invalid sequence",
        ));
    };
    let kind = unsafe { crate::object_type_id(pointer) };
    if kind != crate::TYPE_ID_LIST && kind != crate::TYPE_ID_TUPLE {
        return Err(tls_resolver_protocol_error(
            py,
            "TLS resolver returned an invalid sequence",
        ));
    }
    unsafe {
        crate::object::seq_access::snapshot(py, pointer, "TLS address snapshot allocation failed")
    }
    .ok_or(TlsConnectError::PendingPython)
}

/// Consume the resolver's owned result before any blocking TCP operation.
fn tls_addresses_from_result(
    py: &PyToken<'_>,
    result: OwnedRuntimeValue<'_>,
) -> Result<Vec<SocketAddr>, TlsConnectError> {
    if exception_pending(py) {
        return Err(TlsConnectError::PendingPython);
    }
    let rows = tls_addrinfo_sequence(py, result.bits())?;
    let mut addresses = Vec::with_capacity(rows.len());
    for &row in rows.iter() {
        let row = tls_addrinfo_sequence(py, row)?;
        if row.len() != 5 || to_i64(obj_from_bits(row[1])) != Some(i64::from(libc::SOCK_STREAM)) {
            return Err(tls_resolver_protocol_error(
                py,
                "TLS resolver returned an invalid TCP row",
            ));
        }
        let family = to_i64(obj_from_bits(row[0])).and_then(|value| i32::try_from(value).ok());
        let address = tls_addrinfo_sequence(py, row[4])?;
        let numeric = address
            .first()
            .and_then(|&bits| string_obj_to_owned(obj_from_bits(bits)))
            .and_then(|host| host.parse::<IpAddr>().ok());
        // The canonical producer emits numeric addresses. Refuse a malformed
        // hostname before the shared sockaddr converter can resolve it again.
        let numeric_family = match numeric {
            Some(IpAddr::V4(_)) if address.len() == 2 => libc::AF_INET,
            Some(IpAddr::V6(_))
                if address.len() == 4
                    && address[2..].iter().all(|&bits| {
                        to_i64(obj_from_bits(bits))
                            .and_then(|value| u32::try_from(value).ok())
                            .is_some()
                    }) =>
            {
                libc::AF_INET6
            }
            _ => {
                return Err(tls_resolver_protocol_error(
                    py,
                    "TLS resolver returned a nonnumeric address",
                ));
            }
        };
        if family != Some(numeric_family) {
            return Err(tls_resolver_protocol_error(
                py,
                "TLS resolver returned a mismatched address family",
            ));
        }
        let socket_address =
            crate::async_rt::sockets::sockaddr_from_bits(py, row[4], numeric_family)
                .map_err(|message| tls_resolver_protocol_error(py, &message))?
                .as_socket()
                .ok_or_else(|| {
                    tls_resolver_protocol_error(py, "TLS resolver returned a non-IP address")
                })?;
        addresses.push(socket_address);
    }
    if addresses.is_empty() {
        return Err(tls_resolver_protocol_error(
            py,
            "TLS resolver returned no TCP addresses",
        ));
    }
    Ok(addresses)
}

/// Resolve before connecting, as CPython 3.12.15 asyncio/base_events.py's
/// create_connection does (https://github.com/python/cpython/blob/v3.12.15/Lib/asyncio/base_events.py).
/// Reusing the canonical intrinsic preserves gaierror/EAI_SYSTEM and its
/// NetResolve policy in addition to the caller's SslConnect admission.
/// Cold setup now owns temporary Python addrinfo rows and a Rust SocketAddr
/// vector (plus scoped row snapshots), unlike std's opaque resolver. These
/// Python owners retire before TCP connect; the Rust vector supplies a numeric
/// SocketAddr slice through connect and performs no second DNS resolution.
#[cfg(molt_has_net_io)]
pub(super) fn tls_client_connect_native(
    py: &PyToken<'_>,
    host_bits: u64,
    port: u16,
    server_name: &str,
) -> Result<*mut u8, TlsConnectError> {
    let result = unsafe {
        crate::async_rt::sockets_net::molt_socket_getaddrinfo(
            host_bits,
            MoltObject::from_int(i64::from(port)).bits(),
            MoltObject::from_int(i64::from(libc::AF_UNSPEC)).bits(),
            MoltObject::from_int(i64::from(libc::SOCK_STREAM)).bits(),
            MoltObject::from_int(0).bits(),
            MoltObject::from_int(0).bits(),
        )
    };
    let result = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), result) };
    let addresses = tls_addresses_from_result(py, result)?;
    let tcp = {
        let _release = GilReleaseGuard::suspend();
        TcpStream::connect(addresses.as_slice()).map_err(TlsConnectError::Io)?
    };
    let wrapped = tls_client_wrap_stream_native(tcp, server_name);
    if wrapped.is_null() {
        if exception_pending(py) {
            Err(TlsConnectError::PendingPython)
        } else {
            Err(TlsConnectError::Io(std::io::Error::other(
                "asyncio TLS client connection failed",
            )))
        }
    } else {
        Ok(wrapped)
    }
}

#[cfg(molt_has_net_io)]
type TlsServerConfigCache = Mutex<HashMap<(String, String), Arc<ServerConfig>>>;

#[cfg(molt_has_net_io)]
fn tls_server_config_cache() -> &'static Mutex<HashMap<(String, String), Arc<ServerConfig>>> {
    static CACHE: OnceLock<TlsServerConfigCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(molt_has_net_io)]
fn tls_server_load_config(certfile: &str, keyfile: &str) -> Result<Arc<ServerConfig>, ()> {
    let cache_key = (certfile.to_string(), keyfile.to_string());
    {
        let cache = tls_server_config_cache().lock().unwrap();
        if let Some(config) = cache.get(&cache_key) {
            return Ok(config.clone());
        }
    }

    let cert_file = File::open(certfile).map_err(|_| ())?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ())?;
    if certs.is_empty() {
        return Err(());
    }

    let key_file = File::open(keyfile).map_err(|_| ())?;
    let mut key_reader = BufReader::new(key_file);
    let private_key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|_| ())?
        .ok_or(())?;

    let config = Arc::new(
        ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, private_key)
            .map_err(|_| ())?,
    );

    let mut cache = tls_server_config_cache().lock().unwrap();
    cache.insert(cache_key, config.clone());
    Ok(config)
}

#[cfg(molt_has_net_io)]
fn tls_server_wrap_stream_native(tcp: TcpStream, config: Arc<ServerConfig>) -> *mut u8 {
    let stream = {
        let _release = GilReleaseGuard::suspend();
        let _ = tcp.set_nodelay(true);
        let conn = match ServerConnection::new(config) {
            Ok(value) => value,
            Err(_) => return std::ptr::null_mut(),
        };
        let mut stream = StreamOwned::new(conn, tcp);
        while stream.conn.is_handshaking() {
            match stream.conn.complete_io(&mut stream.sock) {
                Ok(_) => {}
                Err(_) => return std::ptr::null_mut(),
            }
        }
        stream
    };
    tls_wrap_endpoint_native(NativeTlsEndpoint::ServerTcp(stream))
}

#[cfg(all(molt_has_net_io, unix))]
fn tls_server_wrap_unix_stream_native(unix: UnixStream, config: Arc<ServerConfig>) -> *mut u8 {
    let stream = {
        let _release = GilReleaseGuard::suspend();
        let conn = match ServerConnection::new(config) {
            Ok(value) => value,
            Err(_) => return std::ptr::null_mut(),
        };
        let mut stream = StreamOwned::new(conn, unix);
        while stream.conn.is_handshaking() {
            match stream.conn.complete_io(&mut stream.sock) {
                Ok(_) => {}
                Err(_) => return std::ptr::null_mut(),
            }
        }
        stream
    };
    tls_wrap_endpoint_native(NativeTlsEndpoint::ServerUnix(stream))
}

#[cfg(all(molt_has_net_io, unix))]
fn tls_fd_socket_domain(raw_fd: RawFd) -> Option<i32> {
    let mut addr: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockname(
            raw_fd,
            (&mut addr as *mut libc::sockaddr_storage).cast::<libc::sockaddr>(),
            &mut len,
        )
    };
    if rc == 0 {
        Some(i32::from(addr.ss_family))
    } else {
        None
    }
}

#[cfg(all(molt_has_net_io, unix))]
pub(super) fn tls_server_from_fd_native(raw_fd: i64, certfile: &str, keyfile: &str) -> *mut u8 {
    if raw_fd < 0 || raw_fd > i64::from(i32::MAX) {
        return std::ptr::null_mut();
    }
    let Ok(config) = tls_server_load_config(certfile, keyfile) else {
        return std::ptr::null_mut();
    };
    let fd = raw_fd as RawFd;
    match tls_fd_socket_domain(fd) {
        Some(libc::AF_UNIX) => {
            let unix = unsafe { UnixStream::from_raw_fd(fd) };
            tls_server_wrap_unix_stream_native(unix, config)
        }
        Some(libc::AF_INET) | Some(libc::AF_INET6) => {
            let tcp = unsafe { TcpStream::from_raw_fd(fd) };
            tls_server_wrap_stream_native(tcp, config)
        }
        _ => std::ptr::null_mut(),
    }
}

#[cfg(all(molt_has_net_io, windows))]
pub(super) fn tls_server_from_fd_native(raw_fd: i64, certfile: &str, keyfile: &str) -> *mut u8 {
    if raw_fd < 0 {
        return std::ptr::null_mut();
    }
    let Ok(config) = tls_server_load_config(certfile, keyfile) else {
        return std::ptr::null_mut();
    };
    let tcp = unsafe { TcpStream::from_raw_socket(raw_fd as RawSocket) };
    tls_server_wrap_stream_native(tcp, config)
}

#[cfg(molt_has_net_io)]
pub(super) fn tls_server_ssl_attr_string(
    _py: &PyToken<'_>,
    ssl_bits: u64,
    slot: &AtomicU64,
    name: &'static [u8],
) -> Result<Option<String>, u64> {
    let attr_name_bits = intern_static_name(_py, slot, name);
    let missing = missing_bits(_py);
    let attr_bits = molt_getattr_builtin(ssl_bits, attr_name_bits, missing);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, attr_bits) || obj_from_bits(attr_bits).is_none() {
        return Ok(None);
    }
    let Some(value) = string_obj_to_owned(obj_from_bits(attr_bits)) else {
        dec_ref_bits(_py, attr_bits);
        return Err(raise_exception::<u64>(
            _py,
            BuiltinExc::TypeError,
            "ssl context cert/key attributes must be str or None",
        ));
    };
    dec_ref_bits(_py, attr_bits);
    if value.is_empty() {
        return Err(raise_exception::<u64>(
            _py,
            BuiltinExc::ValueError,
            "ssl cert/key paths cannot be empty",
        ));
    }
    Ok(Some(value))
}

#[cfg(all(molt_has_net_io, unix))]
pub(super) fn tls_client_from_fd_native(raw_fd: i64, server_name: &str) -> *mut u8 {
    if raw_fd < 0 || raw_fd > i64::from(i32::MAX) {
        return std::ptr::null_mut();
    }
    let fd = raw_fd as RawFd;
    match tls_fd_socket_domain(fd) {
        Some(libc::AF_UNIX) => {
            let unix = unsafe { UnixStream::from_raw_fd(fd) };
            tls_client_wrap_unix_stream_native(unix, server_name)
        }
        Some(libc::AF_INET) | Some(libc::AF_INET6) => {
            let tcp = unsafe { TcpStream::from_raw_fd(fd) };
            tls_client_wrap_stream_native(tcp, server_name)
        }
        _ => std::ptr::null_mut(),
    }
}

#[cfg(all(molt_has_net_io, windows))]
pub(super) fn tls_client_from_fd_native(raw_fd: i64, server_name: &str) -> *mut u8 {
    if raw_fd < 0 {
        return std::ptr::null_mut();
    }
    let tcp = unsafe { TcpStream::from_raw_socket(raw_fd as RawSocket) };
    tls_client_wrap_stream_native(tcp, server_name)
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use crate::test_support::RuntimeTestTransaction;
    use crate::{alloc_list, alloc_string, alloc_tuple, clear_exception, inc_ref_bits};

    fn addrinfo_row(py: &PyToken<'_>, family: i32, host: &str, scope: Option<(u32, u32)>) -> u64 {
        let host = alloc_string(py, host.as_bytes());
        assert!(!host.is_null());
        let host = MoltObject::from_ptr(host).bits();
        let mut address = vec![host, MoltObject::from_int(443).bits()];
        if let Some((flow, scope)) = scope {
            address.extend([
                MoltObject::from_int(i64::from(flow)).bits(),
                MoltObject::from_int(i64::from(scope)).bits(),
            ]);
        }
        let address = alloc_tuple(py, &address);
        dec_ref_bits(py, host);
        assert!(!address.is_null());
        let address = MoltObject::from_ptr(address).bits();
        let row = alloc_tuple(
            py,
            &[
                MoltObject::from_int(i64::from(family)).bits(),
                MoltObject::from_int(i64::from(libc::SOCK_STREAM)).bits(),
                MoltObject::from_int(i64::from(libc::IPPROTO_TCP)).bits(),
                MoltObject::none().bits(),
                address,
            ],
        );
        dec_ref_bits(py, address);
        assert!(!row.is_null());
        MoltObject::from_ptr(row).bits()
    }

    #[test]
    fn tls_addrinfo_adapter_preserves_numeric_order_ipv6_scope_and_ownership() {
        let _guard = RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let v4 = addrinfo_row(py, libc::AF_INET, "127.0.0.1", None);
            let v6 = addrinfo_row(py, libc::AF_INET6, "fe80::1", Some((0x10002, 9)));
            let result = alloc_list(py, &[v4, v6]);
            assert!(!result.is_null());
            dec_ref_bits(py, v4);
            dec_ref_bits(py, v6);
            let bits = MoltObject::from_ptr(result).bits();
            let refs = || unsafe { (*crate::header_from_obj_ptr(result)).ref_count_snapshot() };
            let baseline = refs();
            inc_ref_bits(py, bits); // The adapter consumes one distinct owned result.
            let result = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), bits) };
            let addresses = tls_addresses_from_result(py, result).unwrap();
            assert_eq!(refs(), baseline);
            assert_eq!(
                addresses,
                vec![
                    "127.0.0.1:443".parse::<SocketAddr>().unwrap(),
                    SocketAddr::V6(std::net::SocketAddrV6::new(
                        "fe80::1".parse().unwrap(),
                        443,
                        0x10002,
                        9
                    )),
                ]
            );
            assert!(!exception_pending(py));
            dec_ref_bits(py, bits);
        });
    }

    #[test]
    fn tls_resolver_pending_keeps_exact_class_errno_and_exception_owner() {
        let _guard = RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (name, code) in [
                (crate::BuiltinExc::SocketGaiError, 9),
                (crate::BuiltinExc::OSError, 9),
                (crate::BuiltinExc::BlockingIOError, 11),
            ] {
                let class = crate::builtins::exceptions::exception_class_bits(py, name);
                crate::builtins::exceptions::raise_os_error_errno_with_class::<u64>(
                    py,
                    class,
                    code,
                    "resolver detail",
                );
                let original = crate::molt_exception_last_pending();
                let result = unsafe {
                    OwnedRuntimeValue::from_owned_bits(py.core_token(), MoltObject::none().bits())
                };
                assert!(matches!(
                    tls_addresses_from_result(py, result),
                    Err(TlsConnectError::PendingPython)
                ));
                assert_eq!(
                    crate::builtins::exceptions::exception_last_bits_noinc(py),
                    Some(original)
                );
                assert_eq!(
                    crate::builtins::exceptions::exception_class(py, original)
                        .unwrap()
                        .bits(),
                    class
                );
                // Pending ownership is proven above. Ordinary descriptor lookup
                // requires a clear error slot; `original` retains the exception.
                clear_exception(py);
                let errno_name = crate::attr_name_bits_from_bytes(py, b"errno").unwrap();
                let errno =
                    crate::molt_getattr_builtin(original, errno_name, MoltObject::none().bits());
                assert!(!exception_pending(py));
                assert_eq!(to_i64(obj_from_bits(errno)), Some(code));
                dec_ref_bits(py, errno_name);
                dec_ref_bits(py, errno);
                dec_ref_bits(py, original);
            }
        });
    }

    #[test]
    fn tls_resolver_missing_or_nonnumeric_results_are_explicit_protocol_errors() {
        let _guard = RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let bad = addrinfo_row(py, libc::AF_INET, "must-not-resolve.invalid", None);
            let nonnumeric = alloc_list(py, &[bad]);
            dec_ref_bits(py, bad);
            assert!(!nonnumeric.is_null());
            let empty = alloc_list(py, &[]);
            assert!(!empty.is_null());
            for result in [
                MoltObject::none().bits(),
                MoltObject::from_ptr(empty).bits(),
                MoltObject::from_ptr(nonnumeric).bits(),
            ] {
                let result = unsafe { OwnedRuntimeValue::from_owned_bits(py.core_token(), result) };
                assert!(matches!(
                    tls_addresses_from_result(py, result),
                    Err(TlsConnectError::PendingPython)
                ));
                let error = crate::molt_exception_last_pending();
                let class = crate::builtins::exceptions::exception_class(py, error).unwrap();
                assert_eq!(
                    class.bits(),
                    crate::builtins::exceptions::exception_class_bits(py, BuiltinExc::RuntimeError)
                );
                clear_exception(py);
                dec_ref_bits(py, error);
            }
        });
    }

    #[test]
    fn tls_connect_requires_canonical_net_resolve_in_addition_to_ssl_connect() {
        RuntimeTestTransaction::with_explicit_capabilities("ssl.connect", |py| {
            let host = alloc_string(py, b"127.0.0.1");
            assert!(!host.is_null());
            let host = MoltObject::from_ptr(host).bits();
            let result = unsafe {
                super::super::molt_asyncio_tls_client_connect_new(
                    host,
                    MoltObject::from_int(443).bits(),
                    MoltObject::none().bits(),
                )
            };
            dec_ref_bits(py, host);
            assert!(obj_from_bits(result).is_none());
            let error = crate::molt_exception_last_pending();
            assert_eq!(
                crate::type_name(py, obj_from_bits(error)),
                "PermissionError"
            );
            let message = crate::builtins::exceptions::format_exception_message(
                py,
                obj_from_bits(error).as_ptr().unwrap(),
            );
            assert!(message.contains("net.resolve"), "{message}");
            assert!(
                !message.contains("ssl.connect"),
                "SSL admission should have passed: {message}"
            );
            clear_exception(py);
            dec_ref_bits(py, error);
        });
    }
}
