#![cfg(molt_has_net_io)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
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
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};
use webpki_roots::TLS_SERVER_ROOTS;

use super::super::stream::molt_stream_new_with_io_hooks;
use crate::{
    GilReleaseGuard, MoltObject, PyToken, alloc_bytes, dec_ref_bits, exception_pending,
    intern_static_name, is_missing_bits, missing_bits, molt_getattr_builtin, obj_from_bits,
    pending_bits_i64, raise_exception, string_obj_to_owned,
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
        let _release = GilReleaseGuard::new();
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
        let _release = GilReleaseGuard::new();
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
pub(super) fn tls_client_connect_native(
    host: &str,
    port: u16,
    server_name: &str,
) -> Result<*mut u8, std::io::Error> {
    let tcp = {
        let _release = GilReleaseGuard::new();
        TcpStream::connect((host, port))?
    };
    let wrapped = tls_client_wrap_stream_native(tcp, server_name);
    if wrapped.is_null() {
        Err(std::io::Error::other(
            "asyncio TLS client connection failed",
        ))
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
        let _release = GilReleaseGuard::new();
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
        let _release = GilReleaseGuard::new();
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
            "TypeError",
            "ssl context cert/key attributes must be str or None",
        ));
    };
    dec_ref_bits(_py, attr_bits);
    if value.is_empty() {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
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
