use super::channels::has_capability;
use crate::PyToken;
use crate::*;

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(not(target_arch = "wasm32"))]
use socket2::{Domain, Protocol, SockAddr, SockRef, Socket, Type};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(target_arch = "wasm32")]
use std::collections::HashMap;
#[cfg(not(target_arch = "wasm32"))]
use std::ffi::{CStr, CString, OsString};
use std::io::ErrorKind;
#[cfg(target_arch = "wasm32")]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
#[cfg(not(target_arch = "wasm32"))]
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
#[cfg(unix)]
use std::os::fd::BorrowedFd;
#[cfg(not(target_arch = "wasm32"))]
use std::os::raw::{c_int, c_void};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, BorrowedSocket, FromRawSocket, IntoRawSocket, RawSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Mutex, OnceLock};
#[cfg(target_arch = "wasm32")]
use std::sync::{Mutex, OnceLock};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;
#[cfg(target_arch = "wasm32")]
use std::time::Duration;

// --- Sockets ---

#[cfg(not(target_arch = "wasm32"))]
enum MoltSocketKind {
    Closed,
    Pending(Socket),
    TcpStream(mio::net::TcpStream),
    TcpListener(mio::net::TcpListener),
    UdpSocket(mio::net::UdpSocket),
    #[cfg(unix)]
    UnixStream(mio::net::UnixStream),
    #[cfg(unix)]
    UnixListener(mio::net::UnixListener),
    #[cfg(unix)]
    UnixDatagram(mio::net::UnixDatagram),
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct MoltSocketInner {
    kind: MoltSocketKind,
    family: i32,
    #[allow(dead_code)]
    sock_type: i32,
    #[allow(dead_code)]
    proto: i32,
    connect_pending: bool,
}

#[cfg(not(target_arch = "wasm32"))]
struct MoltSocket {
    inner: Mutex<MoltSocketInner>,
    timeout: Mutex<Option<Duration>>,
    closed: AtomicBool,
    refs: AtomicUsize,
}

struct MoltSocketReader {
    socket_bits: u64,
    buffer: Vec<u8>,
    eof: bool,
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(unix)]
type SocketFd = RawFd;
#[cfg(not(target_arch = "wasm32"))]
#[cfg(windows)]
type SocketFd = RawSocket;

#[cfg(not(target_arch = "wasm32"))]
fn socket_fd_map() -> &'static Mutex<HashMap<SocketFd, PtrSlot>> {
    static SOCKET_FD_MAP: OnceLock<Mutex<HashMap<SocketFd, PtrSlot>>> = OnceLock::new();
    SOCKET_FD_MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(not(target_arch = "wasm32"))]
fn trace_socket_recv() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_SOCKET_RECV").as_deref() == Ok("1"))
}

#[cfg(not(target_arch = "wasm32"))]
fn trace_socket_send() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_SOCKET_SEND").as_deref() == Ok("1"))
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_debug_fd(socket_ptr: *mut u8) -> Option<i64> {
    with_socket_mut(socket_ptr, |inner| {
        #[cfg(unix)]
        {
            inner
                .raw_fd()
                .map(|fd| fd as i64)
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))
        }
        #[cfg(windows)]
        {
            inner
                .raw_socket()
                .map(|fd| fd as i64)
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))
        }
    })
    .ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_register_fd(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let guard = socket.inner.lock().unwrap();
    #[cfg(unix)]
    let fd = guard.raw_fd();
    #[cfg(windows)]
    let fd = guard.raw_socket();
    drop(guard);
    if let Some(fd) = fd {
        socket_fd_map()
            .lock()
            .unwrap()
            .insert(fd, PtrSlot(socket_ptr));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_unregister_fd(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let guard = socket.inner.lock().unwrap();
    #[cfg(unix)]
    let fd = guard.raw_fd();
    #[cfg(windows)]
    let fd = guard.raw_socket();
    drop(guard);
    if let Some(fd) = fd {
        socket_fd_map().lock().unwrap().remove(&fd);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_ptr_from_fd(fd: SocketFd) -> Option<*mut u8> {
    socket_fd_map().lock().unwrap().get(&fd).map(|slot| slot.0)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn socket_ptr_from_bits_or_fd(socket_bits: u64) -> *mut u8 {
    let ptr = ptr_from_bits(socket_bits);
    if !ptr.is_null() {
        return ptr;
    }
    if let Some(fd) = to_i64(obj_from_bits(socket_bits)) {
        if fd < 0 {
            return std::ptr::null_mut();
        }
        #[cfg(unix)]
        {
            return socket_ptr_from_fd(fd as RawFd).unwrap_or(std::ptr::null_mut());
        }
        #[cfg(windows)]
        {
            return socket_ptr_from_fd(fd as RawSocket).unwrap_or(std::ptr::null_mut());
        }
    }
    std::ptr::null_mut()
}

#[cfg(not(target_arch = "wasm32"))]
impl MoltSocketInner {
    pub(crate) fn source_mut(&mut self) -> Option<&mut dyn mio::event::Source> {
        match &mut self.kind {
            MoltSocketKind::Closed => None,
            MoltSocketKind::TcpStream(stream) => Some(stream),
            MoltSocketKind::TcpListener(listener) => Some(listener),
            MoltSocketKind::UdpSocket(sock) => Some(sock),
            #[cfg(unix)]
            MoltSocketKind::UnixStream(stream) => Some(stream),
            #[cfg(unix)]
            MoltSocketKind::UnixListener(listener) => Some(listener),
            #[cfg(unix)]
            MoltSocketKind::UnixDatagram(sock) => Some(sock),
            MoltSocketKind::Pending(_) => None,
        }
    }

    #[allow(dead_code)]
    fn is_stream(&self) -> bool {
        match self.kind {
            MoltSocketKind::TcpStream(_)
            | MoltSocketKind::Pending(_)
            | MoltSocketKind::TcpListener(_) => true,
            #[cfg(unix)]
            MoltSocketKind::UnixStream(_) | MoltSocketKind::UnixListener(_) => true,
            _ => false,
        }
    }

    #[cfg(unix)]
    pub(crate) fn raw_fd(&self) -> Option<RawFd> {
        let fd = match &self.kind {
            MoltSocketKind::Pending(sock) => sock.as_raw_fd(),
            MoltSocketKind::TcpStream(sock) => sock.as_raw_fd(),
            MoltSocketKind::TcpListener(sock) => sock.as_raw_fd(),
            MoltSocketKind::UdpSocket(sock) => sock.as_raw_fd(),
            MoltSocketKind::UnixStream(sock) => sock.as_raw_fd(),
            MoltSocketKind::UnixListener(sock) => sock.as_raw_fd(),
            MoltSocketKind::UnixDatagram(sock) => sock.as_raw_fd(),
            MoltSocketKind::Closed => return None,
        };
        Some(fd)
    }

    #[cfg(windows)]
    pub(crate) fn raw_socket(&self) -> Option<RawSocket> {
        let sock = match &self.kind {
            MoltSocketKind::Pending(sock) => sock.as_raw_socket(),
            MoltSocketKind::TcpStream(sock) => sock.as_raw_socket(),
            MoltSocketKind::TcpListener(sock) => sock.as_raw_socket(),
            MoltSocketKind::UdpSocket(sock) => sock.as_raw_socket(),
            MoltSocketKind::Closed => return None,
        };
        Some(sock)
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn with_socket_mut<R, F>(socket_ptr: *mut u8, f: F) -> Result<R, std::io::Error>
where
    F: FnOnce(&mut MoltSocketInner) -> Result<R, std::io::Error>,
{
    if socket_ptr.is_null() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            "invalid socket",
        ));
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    if socket.closed.load(AtomicOrdering::Relaxed) {
        return Err(std::io::Error::new(
            ErrorKind::NotConnected,
            "socket is closed",
        ));
    }
    let mut guard = socket.inner.lock().unwrap();
    f(&mut guard)
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_timeout(socket_ptr: *mut u8) -> Option<Duration> {
    if socket_ptr.is_null() {
        return None;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let guard = socket.timeout.lock().unwrap();
    *guard
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_set_timeout(socket_ptr: *mut u8, timeout: Option<Duration>) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let mut guard = socket.timeout.lock().unwrap();
    *guard = timeout;
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
struct WasmSocketMeta {
    family: i32,
    sock_type: i32,
    proto: i32,
    timeout: Option<Duration>,
    connect_pending: bool,
}

#[cfg(target_arch = "wasm32")]
fn wasm_socket_meta_map() -> &'static Mutex<HashMap<i64, WasmSocketMeta>> {
    static MAP: OnceLock<Mutex<HashMap<i64, WasmSocketMeta>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(target_arch = "wasm32")]
fn wasm_socket_meta_insert(handle: i64, meta: WasmSocketMeta) {
    wasm_socket_meta_map().lock().unwrap().insert(handle, meta);
}

#[cfg(target_arch = "wasm32")]
fn wasm_socket_meta_remove(handle: i64) {
    wasm_socket_meta_map().lock().unwrap().remove(&handle);
}

#[cfg(target_arch = "wasm32")]
fn with_wasm_socket_meta_mut<R, F>(handle: i64, f: F) -> Result<R, String>
where
    F: FnOnce(&mut WasmSocketMeta) -> R,
{
    let mut guard = wasm_socket_meta_map().lock().unwrap();
    let Some(meta) = guard.get_mut(&handle) else {
        return Err("socket closed".to_string());
    };
    Ok(f(meta))
}

#[cfg(target_arch = "wasm32")]
fn wasm_socket_family(handle: i64) -> Result<i32, String> {
    let guard = wasm_socket_meta_map().lock().unwrap();
    guard
        .get(&handle)
        .map(|meta| meta.family)
        .ok_or_else(|| "socket closed".to_string())
}

#[cfg(target_arch = "wasm32")]
fn socket_timeout(handle: i64) -> Option<Duration> {
    let guard = wasm_socket_meta_map().lock().unwrap();
    guard.get(&handle).and_then(|meta| meta.timeout)
}

#[cfg(target_arch = "wasm32")]
fn socket_set_timeout(handle: i64, timeout: Option<Duration>) -> Result<(), String> {
    with_wasm_socket_meta_mut(handle, |meta| {
        meta.timeout = timeout;
    })
}

#[cfg(target_arch = "wasm32")]
fn socket_connect_pending(handle: i64) -> bool {
    let guard = wasm_socket_meta_map().lock().unwrap();
    guard
        .get(&handle)
        .map(|meta| meta.connect_pending)
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
fn socket_set_connect_pending(handle: i64, pending: bool) -> Result<(), String> {
    with_wasm_socket_meta_mut(handle, |meta| {
        meta.connect_pending = pending;
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn socket_mark_closed(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    socket.closed.store(true, AtomicOrdering::Relaxed);
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn socket_ref_inc(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    socket.refs.fetch_add(1, AtomicOrdering::AcqRel);
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn socket_ref_dec(_py: &PyToken<'_>, socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    if socket.refs.fetch_sub(1, AtomicOrdering::AcqRel) != 1 {
        return;
    }
    if !socket.closed.load(AtomicOrdering::Relaxed) {
        runtime_state(_py)
            .io_poller()
            .deregister_socket(_py, socket_ptr);
        socket.closed.store(true, AtomicOrdering::Relaxed);
        let mut guard = socket.inner.lock().unwrap();
        guard.kind = MoltSocketKind::Closed;
    }
    release_ptr(socket_ptr);
    unsafe {
        drop(Box::from_raw(socket_ptr as *mut MoltSocket));
    }
}

pub(crate) enum SendData {
    Borrowed(*const u8, usize),
    Owned(Vec<u8>),
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn io_wait_release_socket(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = unsafe { header_from_obj_ptr(future_ptr) };
    let payload_bytes = unsafe {
        (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>())
    };
    if payload_bytes < std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    let socket_bits = unsafe { *payload_ptr };
    let socket_ptr = socket_ptr_from_bits_or_fd(socket_bits);
    if !socket_ptr.is_null() {
        socket_ref_dec(_py, socket_ptr);
    }
    if payload_bytes >= 2 * std::mem::size_of::<u64>() {
        let events_bits = unsafe { *payload_ptr.add(1) };
        dec_ref_bits(_py, events_bits);
    }
    if payload_bytes >= 3 * std::mem::size_of::<u64>() {
        let timeout_bits = unsafe { *payload_ptr.add(2) };
        dec_ref_bits(_py, timeout_bits);
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn io_wait_release_socket(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = unsafe { header_from_obj_ptr(future_ptr) };
    let payload_bytes = unsafe {
        (*header)
            .size
            .saturating_sub(std::mem::size_of::<MoltHeader>())
    };
    if payload_bytes < 2 * std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    let events_bits = unsafe { *payload_ptr.add(1) };
    dec_ref_bits(_py, events_bits);
    if payload_bytes >= 3 * std::mem::size_of::<u64>() {
        let timeout_bits = unsafe { *payload_ptr.add(2) };
        dec_ref_bits(_py, timeout_bits);
    }
}

pub(crate) fn send_data_from_bits(bits: u64) -> Result<SendData, String> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("send expects bytes-like object".to_string());
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
            let len = bytes_len(ptr);
            let data = bytes_data(ptr);
            return Ok(SendData::Borrowed(data, len));
        }
        if type_id == TYPE_ID_MEMORYVIEW {
            if let Some(slice) = memoryview_bytes_slice(ptr) {
                return Ok(SendData::Borrowed(slice.as_ptr(), slice.len()));
            }
            if let Some(vec) = memoryview_collect_bytes(ptr) {
                return Ok(SendData::Owned(vec));
            }
        }
    }
    Err("send expects bytes-like object".to_string())
}

#[cfg(target_arch = "wasm32")]
fn socket_handle_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<i64, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Err("invalid socket".to_string());
    }
    if let Some(val) = to_i64(obj) {
        if val < 0 {
            return Err("invalid socket".to_string());
        }
        return Ok(val);
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("socket handle must be int, not {obj_type}"))
}

#[cfg(target_arch = "wasm32")]
fn errno_from_rc(rc: i32) -> i32 {
    if rc < 0 {
        -rc
    } else {
        0
    }
}

#[cfg(target_arch = "wasm32")]
fn would_block_errno(errno: i32) -> bool {
    errno == libc::EAGAIN || errno == libc::EWOULDBLOCK
}

pub(crate) fn require_capability<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    caps: &[&str],
    label: &str,
) -> Result<(), T> {
    if caps.iter().any(|cap| has_capability(_py, cap)) {
        Ok(())
    } else {
        let msg = format!("missing {label} capability");
        Err(raise_exception::<T>(_py, "PermissionError", &msg))
    }
}

pub(crate) fn require_time_wall_capability<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
) -> Result<(), T> {
    require_capability(_py, &["time.wall", "time"], "time.wall")
}

pub(crate) fn require_net_capability<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    caps: &[&str],
) -> Result<(), T> {
    require_capability(_py, caps, "net")
}

pub(crate) fn require_process_capability<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    caps: &[&str],
) -> Result<(), T> {
    require_capability(_py, caps, "process")
}

enum SocketReaderPull {
    Pending,
    Eof,
    Data,
}

unsafe fn socket_reader_pull(_py: &PyToken<'_>, reader: &mut MoltSocketReader) -> Result<SocketReaderPull, u64> {
    if reader.eof {
        return Ok(SocketReaderPull::Eof);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let socket_ptr = socket_ptr_from_bits_or_fd(reader.socket_bits);
        if socket_ptr.is_null() {
            return Err(raise_exception::<u64>(_py, "TypeError", "invalid socket handle"));
        }
        let mut buf = vec![0u8; 4096];
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let ret = unsafe {
                libc::recv(
                    libc_socket(fd),
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len(),
                    0,
                )
            };
            if ret >= 0 {
                Ok(ret as usize)
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(0) => {
                reader.eof = true;
                Ok(SocketReaderPull::Eof)
            }
            Ok(n) => {
                reader.buffer.extend_from_slice(&buf[..n]);
                Ok(SocketReaderPull::Data)
            }
            Err(err) => {
                let raw = err.raw_os_error();
                let would_block = err.kind() == ErrorKind::WouldBlock
                    || matches!(raw, Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK);
                if would_block {
                    Ok(SocketReaderPull::Pending)
                } else {
                    Err(raise_os_error::<u64>(_py, err, "recv"))
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let handle = match socket_handle_from_bits(_py, reader.socket_bits) {
            Ok(val) => val,
            Err(msg) => return Err(raise_exception::<u64>(_py, "TypeError", &msg)),
        };
        let mut buf = vec![0u8; 4096];
        let rc = unsafe { crate::molt_socket_recv_host(handle, buf.as_mut_ptr() as u32, buf.len() as u32, 0) };
        if rc >= 0 {
            let n = rc as usize;
            if n == 0 {
                reader.eof = true;
                return Ok(SocketReaderPull::Eof);
            }
            reader.buffer.extend_from_slice(&buf[..n]);
            return Ok(SocketReaderPull::Data);
        }
        let errno = errno_from_rc(rc);
        if would_block_errno(errno) {
            Ok(SocketReaderPull::Pending)
        } else {
            Err(raise_os_error_errno::<u64>(_py, errno as i64, "recv"))
        }
    }
}

fn socket_reader_take(_py: &PyToken<'_>, reader: &mut MoltSocketReader, count: usize) -> u64 {
    let n = count.min(reader.buffer.len());
    let ptr = alloc_bytes(_py, &reader.buffer[..n]);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    reader.buffer.drain(..n);
    MoltObject::from_ptr(ptr).bits()
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid socket handle from `molt_socket_new`/`molt_socket_clone`.
pub unsafe extern "C" fn molt_socket_reader_new(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let clone_bits = molt_socket_clone(sock_bits);
        if obj_from_bits(clone_bits).is_none() {
            return MoltObject::none().bits();
        }
        let reader = Box::new(MoltSocketReader {
            socket_bits: clone_bits,
            buffer: Vec::new(),
            eof: false,
        });
        bits_from_ptr(Box::into_raw(reader) as *mut u8)
    })
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_drop(reader_bits: u64) {
    crate::with_gil_entry!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return;
        }
        let reader = Box::from_raw(reader_ptr as *mut MoltSocketReader);
        molt_socket_drop(reader.socket_bits);
    })
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_at_eof(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::from_bool(true).bits();
        }
        let reader = &*(reader_ptr as *mut MoltSocketReader);
        MoltObject::from_bool(reader.eof && reader.buffer.is_empty()).bits()
    })
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_read(reader_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let reader = &mut *(reader_ptr as *mut MoltSocketReader);
        let n = to_i64(obj_from_bits(n_bits)).unwrap_or(-1);
        if n == 0 {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        if n < 0 {
            loop {
                if reader.eof {
                    return socket_reader_take(_py, reader, reader.buffer.len());
                }
                match socket_reader_pull(_py, reader) {
                    Ok(SocketReaderPull::Pending) => return pending_bits_i64() as u64,
                    Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                    Err(bits) => return bits,
                }
            }
        }
        if !reader.buffer.is_empty() {
            return socket_reader_take(_py, reader, n as usize);
        }
        if reader.eof {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        loop {
            match socket_reader_pull(_py, reader) {
                Ok(SocketReaderPull::Pending) => return pending_bits_i64() as u64,
                Ok(SocketReaderPull::Eof) => {
                    if reader.buffer.is_empty() {
                        let ptr = alloc_bytes(_py, &[]);
                        if ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(ptr).bits();
                    }
                    return socket_reader_take(_py, reader, n as usize);
                }
                Ok(SocketReaderPull::Data) => {
                    if !reader.buffer.is_empty() {
                        return socket_reader_take(_py, reader, n as usize);
                    }
                }
                Err(bits) => return bits,
            }
        }
    })
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_readline(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let reader = &mut *(reader_ptr as *mut MoltSocketReader);
        loop {
            if let Some(idx) = reader.buffer.iter().position(|&b| b == b'\n') {
                return socket_reader_take(_py, reader, idx + 1);
            }
            if reader.eof {
                return socket_reader_take(_py, reader, reader.buffer.len());
            }
            match socket_reader_pull(_py, reader) {
                Ok(SocketReaderPull::Pending) => return pending_bits_i64() as u64,
                Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                Err(bits) => return bits,
            }
        }
    })
}

fn host_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<Option<String>, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(Some(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| "host bytes must be utf-8".to_string())?;
                return Ok(Some(text.to_string()));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("host must be str, bytes, or None, not {obj_type}"))
}

fn port_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<u16, String> {
    let obj = obj_from_bits(bits);
    if let Some(port) = to_i64(obj) {
        if port < 0 || port > u16::MAX as i64 {
            return Err("port out of range".to_string());
        }
        return Ok(port as u16);
    }
    if let Some(text) = string_obj_to_owned(obj) {
        let port = text
            .parse::<u16>()
            .map_err(|_| "port must be int".to_string())?;
        return Ok(port);
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("port must be int or str, not {obj_type}"))
}

fn service_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<Option<String>, String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(None);
    }
    if let Some(port) = to_i64(obj) {
        if port < 0 || port > u16::MAX as i64 {
            return Err("port out of range".to_string());
        }
        return Ok(Some(port.to_string()));
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(Some(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| "service bytes must be utf-8".to_string())?;
                return Ok(Some(text.to_string()));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, bits));
    Err(format!("service must be int or str, not {obj_type}"))
}

#[cfg(unix)]
fn unix_path_from_bits(_py: &PyToken<'_>, addr_bits: u64) -> Result<std::path::PathBuf, String> {
    let obj = obj_from_bits(addr_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok(std::path::PathBuf::from(text));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                use std::os::unix::ffi::OsStringExt;
                let path = std::ffi::OsString::from_vec(bytes.to_vec());
                return Ok(std::path::PathBuf::from(path));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, addr_bits));
    Err(format!("a bytes-like object is required, not '{obj_type}'"))
}

#[cfg(not(target_arch = "wasm32"))]
fn sockaddr_from_bits(_py: &PyToken<'_>, addr_bits: u64, family: i32) -> Result<SockAddr, String> {
    if family == libc::AF_UNIX {
        #[cfg(unix)]
        {
            let path = unix_path_from_bits(_py, addr_bits)?;
            return SockAddr::unix(path).map_err(|err| err.to_string());
        }
        #[cfg(not(unix))]
        {
            return Err("AF_UNIX is unsupported".to_string());
        }
    }
    let obj = obj_from_bits(addr_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("address must be tuple".to_string());
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return Err("address must be tuple".to_string());
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 2 {
            return Err("address must be (host, port)".to_string());
        }
        let host = host_from_bits(_py, elems[0])?;
        let port = port_from_bits(_py, elems[1])?;
        if family == libc::AF_INET {
            let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
            let ip = host
                .parse::<Ipv4Addr>()
                .or_else(|_| {
                    (host.as_str(), port)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut iter| {
                            iter.find_map(|addr| match addr.ip() {
                                IpAddr::V4(v4) => Some(v4),
                                _ => None,
                            })
                        })
                        .ok_or(())
                })
                .map_err(|_| "invalid IPv4 address".to_string())?;
            return Ok(SockAddr::from(SocketAddr::new(IpAddr::V4(ip), port)));
        }
        if family == libc::AF_INET6 {
            let host = host.unwrap_or_else(|| "::".to_string());
            let mut flowinfo = 0u32;
            let mut scope_id = 0u32;
            if elems.len() >= 3 {
                flowinfo = to_i64(obj_from_bits(elems[2])).unwrap_or(0) as u32;
            }
            if elems.len() >= 4 {
                scope_id = to_i64(obj_from_bits(elems[3])).unwrap_or(0) as u32;
            }
            let ip = host
                .parse::<Ipv6Addr>()
                .or_else(|_| {
                    (host.as_str(), port)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut iter| {
                            iter.find_map(|addr| match addr.ip() {
                                IpAddr::V6(v6) => Some(v6),
                                _ => None,
                            })
                        })
                        .ok_or(())
                })
                .map_err(|_| "invalid IPv6 address".to_string())?;
            let addr = SocketAddr::V6(std::net::SocketAddrV6::new(ip, port, flowinfo, scope_id));
            return Ok(SockAddr::from(addr));
        }
    }
    Err("unsupported address family".to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn sockaddr_to_bits(_py: &PyToken<'_>, addr: &SockAddr) -> u64 {
    if let Some(sockaddr) = addr.as_socket() {
        match sockaddr {
            SocketAddr::V4(v4) => {
                let host = v4.ip().to_string();
                let host_ptr = alloc_string(_py, host.as_bytes());
                if host_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let host_bits = MoltObject::from_ptr(host_ptr).bits();
                let port_bits = MoltObject::from_int(v4.port() as i64).bits();
                let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits]);
                dec_ref_bits(_py, host_bits);
                if tuple_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
            SocketAddr::V6(v6) => {
                let host = v6.ip().to_string();
                let host_ptr = alloc_string(_py, host.as_bytes());
                if host_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let host_bits = MoltObject::from_ptr(host_ptr).bits();
                let port_bits = MoltObject::from_int(v6.port() as i64).bits();
                let flow_bits = MoltObject::from_int(v6.flowinfo() as i64).bits();
                let scope_bits = MoltObject::from_int(v6.scope_id() as i64).bits();
                let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits, flow_bits, scope_bits]);
                dec_ref_bits(_py, host_bits);
                if tuple_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            }
        }
    } else {
        #[cfg(unix)]
        {
            if let Some(path) = addr.as_pathname() {
                let text = path.to_string_lossy();
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            } else {
                MoltObject::none().bits()
            }
        }
        #[cfg(not(unix))]
        {
            MoltObject::none().bits()
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn encode_sockaddr(_py: &PyToken<'_>, addr_bits: u64, family: i32) -> Result<Vec<u8>, String> {
    if family == libc::AF_UNIX {
        return Err("AF_UNIX is unsupported".to_string());
    }
    let obj = obj_from_bits(addr_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err("address must be tuple".to_string());
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return Err("address must be tuple".to_string());
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 2 {
            return Err("address must be (host, port)".to_string());
        }
        let host = host_from_bits(_py, elems[0])?;
        let port = port_from_bits(_py, elems[1])?;
        let mut out = Vec::new();
        out.extend_from_slice(&(family as u16).to_le_bytes());
        out.extend_from_slice(&port.to_le_bytes());
        if family == libc::AF_INET {
            let host = host.unwrap_or_else(|| "0.0.0.0".to_string());
            let ip = host
                .parse::<Ipv4Addr>()
                .map_err(|_| "invalid IPv4 address".to_string())?;
            out.extend_from_slice(&ip.octets());
            return Ok(out);
        }
        if family == libc::AF_INET6 {
            let host = host.unwrap_or_else(|| "::".to_string());
            let ip = host
                .parse::<Ipv6Addr>()
                .map_err(|_| "invalid IPv6 address".to_string())?;
            let mut flowinfo = 0u32;
            let mut scope_id = 0u32;
            if elems.len() >= 3 {
                flowinfo = to_i64(obj_from_bits(elems[2])).unwrap_or(0) as u32;
            }
            if elems.len() >= 4 {
                scope_id = to_i64(obj_from_bits(elems[3])).unwrap_or(0) as u32;
            }
            out.extend_from_slice(&flowinfo.to_le_bytes());
            out.extend_from_slice(&scope_id.to_le_bytes());
            out.extend_from_slice(&ip.octets());
            return Ok(out);
        }
        Err("unsupported address family".to_string())
    }
}

#[cfg(target_arch = "wasm32")]
fn decode_sockaddr(_py: &PyToken<'_>, buf: &[u8]) -> Result<u64, String> {
    if buf.len() < 4 {
        return Err("invalid sockaddr".to_string());
    }
    let family = u16::from_le_bytes([buf[0], buf[1]]) as i32;
    let port = u16::from_le_bytes([buf[2], buf[3]]);
    if family == libc::AF_INET {
        if buf.len() < 8 {
            return Err("invalid IPv4 sockaddr".to_string());
        }
        let mut octets = [0u8; 4];
        octets.copy_from_slice(&buf[4..8]);
        let host = Ipv4Addr::from(octets).to_string();
        let host_ptr = alloc_string(_py, host.as_bytes());
        if host_ptr.is_null() {
            return Ok(MoltObject::none().bits());
        }
        let host_bits = MoltObject::from_ptr(host_ptr).bits();
        let port_bits = MoltObject::from_int(port as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits]);
        dec_ref_bits(_py, host_bits);
        if tuple_ptr.is_null() {
            Ok(MoltObject::none().bits())
        } else {
            Ok(MoltObject::from_ptr(tuple_ptr).bits())
        }
    } else if family == libc::AF_INET6 {
        if buf.len() < 28 {
            return Err("invalid IPv6 sockaddr".to_string());
        }
        let flowinfo = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let scope_id = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&buf[12..28]);
        let host = Ipv6Addr::from(octets).to_string();
        let host_ptr = alloc_string(_py, host.as_bytes());
        if host_ptr.is_null() {
            return Ok(MoltObject::none().bits());
        }
        let host_bits = MoltObject::from_ptr(host_ptr).bits();
        let port_bits = MoltObject::from_int(port as i64).bits();
        let flow_bits = MoltObject::from_int(flowinfo as i64).bits();
        let scope_bits = MoltObject::from_int(scope_id as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits, flow_bits, scope_bits]);
        dec_ref_bits(_py, host_bits);
        if tuple_ptr.is_null() {
            Ok(MoltObject::none().bits())
        } else {
            Ok(MoltObject::from_ptr(tuple_ptr).bits())
        }
    } else if family == libc::AF_UNIX {
        Err("AF_UNIX is unsupported".to_string())
    } else {
        Err("unsupported address family".to_string())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn socket_wait_ready(
    _py: &PyToken<'_>,
    socket_ptr: *mut u8,
    events: u32,
) -> Result<(), std::io::Error> {
    let timeout = socket_timeout(socket_ptr);
    #[cfg(unix)]
    {
        let poll_params = with_socket_mut(socket_ptr, |inner| {
            let use_poll = matches!(
                inner.kind,
                MoltSocketKind::UnixStream(_)
                    | MoltSocketKind::UnixListener(_)
                    | MoltSocketKind::UnixDatagram(_)
            );
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            Ok((use_poll, fd))
        });
        match poll_params {
            Ok((true, fd)) => return socket_wait_ready_poll(fd, events, timeout),
            Ok((false, _)) => {}
            Err(err) => return Err(err),
        }
    }
    if let Some(timeout) = timeout {
        if timeout == Duration::ZERO {
            return Err(std::io::Error::from_raw_os_error(libc::EWOULDBLOCK));
        }
        runtime_state(_py)
            .io_poller()
            .wait_blocking(socket_ptr, events, Some(timeout))
            .map(|_| ())
    } else {
        runtime_state(_py)
            .io_poller()
            .wait_blocking(socket_ptr, events, None)
            .map(|_| ())
    }
}

#[cfg(unix)]
fn socket_wait_ready_poll(
    fd: RawFd,
    events: u32,
    timeout: Option<Duration>,
) -> Result<(), std::io::Error> {
    let mut poll_events = 0;
    if (events & IO_EVENT_READ) != 0 {
        poll_events |= libc::POLLIN;
    }
    if (events & IO_EVENT_WRITE) != 0 {
        poll_events |= libc::POLLOUT;
    }
    let timeout_ms = match timeout {
        Some(val) if val == Duration::ZERO => {
            return Err(std::io::Error::from_raw_os_error(libc::EWOULDBLOCK));
        }
        Some(val) => val.as_millis().min(i32::MAX as u128) as i32,
        None => -1,
    };
    let mut poll_fd = libc::pollfd {
        fd,
        events: poll_events,
        revents: 0,
    };
    let rc = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if rc == 0 {
        return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn socket_wait_ready(_py: &PyToken<'_>, handle: i64, events: u32) -> Result<(), std::io::Error> {
    let timeout = socket_timeout(handle);
    if let Some(timeout) = timeout {
        if timeout == Duration::ZERO {
            return Err(std::io::Error::from_raw_os_error(libc::EWOULDBLOCK));
        }
        let ms = timeout.as_millis().min(i64::MAX as u128) as i64;
        let rc = unsafe { crate::molt_socket_wait_host(handle, events, ms) };
        if rc == 0 {
            return Ok(());
        }
        if rc == -(libc::ETIMEDOUT as i32) {
            return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
        }
        return Err(std::io::Error::from_raw_os_error(-rc));
    }
    let rc = unsafe { crate::molt_socket_wait_host(handle, events, -1) };
    if rc == 0 {
        return Ok(());
    }
    if rc == -(libc::ETIMEDOUT as i32) {
        return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
    }
    Err(std::io::Error::from_raw_os_error(-rc))
}

#[cfg(not(target_arch = "wasm32"))]
fn os_string_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<OsString, String> {
    let path = path_from_bits(_py, bits)?;
    Ok(path.into_os_string())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn argv_from_bits(_py: &PyToken<'_>, args_bits: u64) -> Result<Vec<OsString>, String> {
    let obj = obj_from_bits(args_bits);
    if obj.is_none() {
        return Err("args must be a sequence".to_string());
    }
    if let Some(ptr) = obj.as_ptr() {
        let type_id = unsafe { object_type_id(ptr) };
        if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
            let elems = unsafe { seq_vec_ref(ptr) };
            let mut args = Vec::with_capacity(elems.len());
            for &elem in elems.iter() {
                args.push(os_string_from_bits(_py, elem)?);
            }
            return Ok(args);
        }
    }
    Ok(vec![os_string_from_bits(_py, args_bits)?])
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn env_from_bits(
    _py: &PyToken<'_>,
    env_bits: u64,
) -> Result<Option<Vec<(OsString, OsString)>>, String> {
    let obj = obj_from_bits(env_bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(ptr) = obj.as_ptr() else {
        return Err("env must be a dict".to_string());
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DICT {
            return Err("env must be a dict".to_string());
        }
        let order = dict_order(ptr);
        let mut out = Vec::with_capacity(order.len() / 2);
        let mut idx = 0;
        while idx + 1 < order.len() {
            let key_bits = order[idx];
            let val_bits = order[idx + 1];
            out.push((
                os_string_from_bits(_py, key_bits)?,
                os_string_from_bits(_py, val_bits)?,
            ));
            idx += 2;
        }
        Ok(Some(out))
    }
}

#[cfg(unix)]
type LibcSocket = c_int;
#[cfg(windows)]
type LibcSocket = libc::SOCKET;

#[cfg(unix)]
fn libc_socket(fd: RawFd) -> LibcSocket {
    fd
}
#[cfg(windows)]
fn libc_socket(fd: RawSocket) -> LibcSocket {
    fd as LibcSocket
}

#[cfg(not(target_arch = "wasm32"))]
fn connect_raw_socket(fd: SocketFd, sockaddr: &SockAddr) -> std::io::Result<()> {
    let ret = unsafe { libc::connect(libc_socket(fd), sockaddr.as_ptr(), sockaddr.len()) };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn socket_is_acceptor(socket: &Socket) -> bool {
    let fd = socket.as_raw_fd();
    let mut val: c_int = 0;
    let mut len = std::mem::size_of::<c_int>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ACCEPTCONN,
            &mut val as *mut _ as *mut c_void,
            &mut len,
        )
    };
    ret == 0 && val != 0
}

#[cfg(windows)]
fn socket_is_acceptor(_socket: &Socket) -> bool {
    false
}

#[cfg(unix)]
fn socket_relisten(fd: RawFd, backlog: i32) -> std::io::Result<()> {
    let rc = unsafe { libc::listen(fd, backlog) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn socket_relisten(socket: RawSocket, backlog: i32) -> std::io::Result<()> {
    let rc = unsafe { libc::listen(socket, backlog) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn with_sockref<T, F>(fd: RawFd, f: F) -> T
where
    F: FnOnce(SockRef<'_>) -> T,
{
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let sock_ref = SockRef::from(&borrowed);
    f(sock_ref)
}

#[cfg(windows)]
fn with_sockref<T, F>(socket: RawSocket, f: F) -> T
where
    F: FnOnce(SockRef<'_>) -> T,
{
    let borrowed = unsafe { BorrowedSocket::borrow_raw(socket) };
    let sock_ref = SockRef::from(&borrowed);
    f(sock_ref)
}

#[cfg(unix)]
fn take_error_raw(fd: RawFd) -> std::io::Result<Option<std::io::Error>> {
    with_sockref(fd, |sock_ref| sock_ref.take_error())
}

#[cfg(windows)]
fn take_error_raw(socket: RawSocket) -> std::io::Result<Option<std::io::Error>> {
    with_sockref(socket, |sock_ref| sock_ref.take_error())
}

#[cfg(unix)]
fn take_error_mio<T: AsRawFd>(sock: &T) -> std::io::Result<Option<std::io::Error>> {
    take_error_raw(sock.as_raw_fd())
}

#[cfg(windows)]
fn take_error_mio<T: AsRawSocket>(sock: &T) -> std::io::Result<Option<std::io::Error>> {
    take_error_raw(sock.as_raw_socket())
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_socket_new(
    family_bits: u64,
    type_bits: u64,
    proto_bits: u64,
    fileno_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.listen", "net.bind"])
            .is_err()
        {
            return MoltObject::none().bits();
        }
        let family = match to_i64(obj_from_bits(family_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>(_py, "TypeError", "family must be int"),
        };
        let sock_type = match to_i64(obj_from_bits(type_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>(_py, "TypeError", "type must be int"),
        };
        let proto = to_i64(obj_from_bits(proto_bits)).unwrap_or(0) as i32;
        let fileno = if obj_from_bits(fileno_bits).is_none() {
            None
        } else {
            match to_i64(obj_from_bits(fileno_bits)) {
                Some(val) => Some(val),
                None => {
                    return raise_exception::<_>(_py, "TypeError", "fileno must be int or None")
                }
            }
        };
        let domain = match family {
            val if val == libc::AF_INET => Domain::IPV4,
            val if val == libc::AF_INET6 => Domain::IPV6,
            #[cfg(unix)]
            val if val == libc::AF_UNIX => Domain::UNIX,
            _ => {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::EAFNOSUPPORT as i64,
                    "address family not supported",
                );
            }
        };
        #[cfg(unix)]
        let base_type = sock_type & !(SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG);
        #[cfg(not(unix))]
        let base_type = sock_type;
        let socket_type = match base_type {
            val if val == libc::SOCK_STREAM => Type::STREAM,
            val if val == libc::SOCK_DGRAM => Type::DGRAM,
            val if val == libc::SOCK_RAW => Type::from(val),
            _ => {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::EPROTOTYPE as i64,
                    "unsupported socket type",
                );
            }
        };
        let socket = match fileno {
            Some(raw) => unsafe {
                if raw < 0 {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EBADF as i64,
                        "bad file descriptor",
                    );
                }
                #[cfg(unix)]
                {
                    Socket::from_raw_fd(raw as RawFd)
                }
                #[cfg(windows)]
                {
                    Socket::from_raw_socket(raw as RawSocket)
                }
            },
            None => match Socket::new(domain, socket_type, Some(Protocol::from(proto))) {
                Ok(sock) => sock,
                Err(err) => return raise_os_error::<u64>(_py, err, "socket"),
            },
        };
        if let Err(err) = socket.set_nonblocking(true) {
            return raise_os_error::<u64>(_py, err, "socket");
        }
        let timeout = {
            #[cfg(unix)]
            {
                match SOCK_NONBLOCK_FLAG {
                    0 => None,
                    flag => {
                        if (sock_type & flag) != 0 {
                            Some(Duration::ZERO)
                        } else {
                            None
                        }
                    }
                }
            }
            #[cfg(not(unix))]
            {
                None
            }
        };
        let connect_pending = false;
        let kind = if base_type == libc::SOCK_DGRAM {
            #[cfg(unix)]
            if family == libc::AF_UNIX {
                let raw_fd = socket.into_raw_fd();
                let std_sock = unsafe { std::os::unix::net::UnixDatagram::from_raw_fd(raw_fd) };
                if let Err(err) = std_sock.set_nonblocking(true) {
                    return raise_os_error::<u64>(_py, err, "socket");
                }
                MoltSocketKind::UnixDatagram(mio::net::UnixDatagram::from_std(std_sock))
            } else {
                let std_sock: std::net::UdpSocket = socket.into();
                if let Err(err) = std_sock.set_nonblocking(true) {
                    return raise_os_error::<u64>(_py, err, "socket");
                }
                MoltSocketKind::UdpSocket(mio::net::UdpSocket::from_std(std_sock))
            }
            #[cfg(not(unix))]
            {
                let std_sock: std::net::UdpSocket = socket.into();
                if let Err(err) = std_sock.set_nonblocking(true) {
                    return raise_os_error::<u64>(_py, err, "socket");
                }
                MoltSocketKind::UdpSocket(mio::net::UdpSocket::from_std(std_sock))
            }
        } else if let Some(_raw) = fileno {
            let acceptor = socket_is_acceptor(&socket);
            if acceptor {
                #[cfg(unix)]
                if family == libc::AF_UNIX {
                    let raw_fd = socket.into_raw_fd();
                    let std_listener =
                        unsafe { std::os::unix::net::UnixListener::from_raw_fd(raw_fd) };
                    if let Err(err) = std_listener.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::UnixListener(mio::net::UnixListener::from_std(std_listener))
                } else {
                    let std_listener: std::net::TcpListener = socket.into();
                    if let Err(err) = std_listener.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::TcpListener(mio::net::TcpListener::from_std(std_listener))
                }
                #[cfg(not(unix))]
                {
                    let std_listener: std::net::TcpListener = socket.into();
                    if let Err(err) = std_listener.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::TcpListener(mio::net::TcpListener::from_std(std_listener))
                }
            } else {
                #[cfg(unix)]
                if family == libc::AF_UNIX {
                    let raw_fd = socket.into_raw_fd();
                    let std_stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                    if let Err(err) = std_stream.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::UnixStream(mio::net::UnixStream::from_std(std_stream))
                } else {
                    let std_stream: std::net::TcpStream = socket.into();
                    if let Err(err) = std_stream.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream))
                }
                #[cfg(not(unix))]
                {
                    let std_stream: std::net::TcpStream = socket.into();
                    if let Err(err) = std_stream.set_nonblocking(true) {
                        return raise_os_error::<u64>(_py, err, "socket");
                    }
                    MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(std_stream))
                }
            }
        } else {
            MoltSocketKind::Pending(socket)
        };
        let socket = Box::new(MoltSocket {
            inner: Mutex::new(MoltSocketInner {
                kind,
                family,
                sock_type: base_type,
                proto,
                connect_pending,
            }),
            timeout: Mutex::new(timeout),
            closed: AtomicBool::new(false),
            refs: AtomicUsize::new(1),
        });
        let socket_ptr = Box::into_raw(socket) as *mut u8;
        socket_register_fd(socket_ptr);
        bits_from_ptr(socket_ptr)
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_close(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let socket = &*(socket_ptr as *mut MoltSocket);
        if socket.closed.load(AtomicOrdering::Relaxed) {
            return MoltObject::none().bits();
        }
        socket_unregister_fd(socket_ptr);
        runtime_state(_py)
            .io_poller()
            .deregister_socket(_py, socket_ptr);
        socket.closed.store(true, AtomicOrdering::Relaxed);
        {
            let mut guard = socket.inner.lock().unwrap();
            guard.kind = MoltSocketKind::Closed;
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_drop(sock_bits: u64) {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return;
        }
        let socket = &*(socket_ptr as *mut MoltSocket);
        if !socket.closed.load(AtomicOrdering::Relaxed) {
            let _ = molt_socket_close(sock_bits);
        }
        socket_ref_dec(_py, socket_ptr);
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_clone(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        socket_ref_inc(socket_ptr);
        sock_bits
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_clone(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let new_handle = unsafe { crate::molt_socket_clone_host(handle) };
        if new_handle < 0 {
            return raise_os_error_errno::<u64>(_py, (-new_handle) as i64, "socket.clone");
        }
        let meta = {
            let guard = wasm_socket_meta_map().lock().unwrap();
            guard.get(&handle).cloned()
        };
        if let Some(meta) = meta {
            wasm_socket_meta_insert(new_handle, meta);
        }
        MoltObject::from_int(new_handle).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_fileno(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_int(-1).bits();
        }
        let socket = &*(socket_ptr as *mut MoltSocket);
        if socket.closed.load(AtomicOrdering::Relaxed) {
            return MoltObject::from_int(-1).bits();
        }
        let guard = socket.inner.lock().unwrap();
        #[cfg(unix)]
        let fd = match &guard.kind {
            MoltSocketKind::Pending(sock) => sock.as_raw_fd() as i64,
            MoltSocketKind::TcpStream(sock) => sock.as_raw_fd() as i64,
            MoltSocketKind::TcpListener(sock) => sock.as_raw_fd() as i64,
            MoltSocketKind::UdpSocket(sock) => sock.as_raw_fd() as i64,
            #[cfg(unix)]
            MoltSocketKind::UnixStream(sock) => sock.as_raw_fd() as i64,
            #[cfg(unix)]
            MoltSocketKind::UnixListener(sock) => sock.as_raw_fd() as i64,
            #[cfg(unix)]
            MoltSocketKind::UnixDatagram(sock) => sock.as_raw_fd() as i64,
            MoltSocketKind::Closed => -1,
        };
        #[cfg(windows)]
        let fd = match &guard.kind {
            MoltSocketKind::Pending(sock) => sock.as_raw_socket() as i64,
            MoltSocketKind::TcpStream(sock) => sock.as_raw_socket() as i64,
            MoltSocketKind::TcpListener(sock) => sock.as_raw_socket() as i64,
            MoltSocketKind::UdpSocket(sock) => sock.as_raw_socket() as i64,
            MoltSocketKind::Closed => -1,
        };
        MoltObject::from_int(fd).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_gettimeout(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let timeout = socket_timeout(socket_ptr);
        match timeout {
            None => MoltObject::none().bits(),
            Some(val) => MoltObject::from_float(val.as_secs_f64()).bits(),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_settimeout(sock_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(timeout_bits);
        if obj.is_none() {
            socket_set_timeout(socket_ptr, None);
            return MoltObject::none().bits();
        }
        let Some(timeout) = to_f64(obj) else {
            return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
        };
        if !timeout.is_finite() || timeout < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "timeout must be non-negative");
        }
        let duration = if timeout == 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(timeout)
        };
        socket_set_timeout(socket_ptr, Some(duration));
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_setblocking(sock_bits: u64, flag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let flag = obj_from_bits(flag_bits).as_bool().unwrap_or(false);
        if flag {
            socket_set_timeout(socket_ptr, None);
        } else {
            socket_set_timeout(socket_ptr, Some(Duration::ZERO));
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getblocking(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_bool(false).bits();
        }
        let timeout = socket_timeout(socket_ptr);
        let blocking = match timeout {
            None => true,
            Some(val) if val == Duration::ZERO => false,
            Some(_) => true,
        };
        MoltObject::from_bool(blocking).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_bind(sock_bits: u64, addr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.bind", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let family = {
            let socket = &*(socket_ptr as *mut MoltSocket);
            let guard = socket.inner.lock().unwrap();
            guard.family
        };
        let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
            Ok(addr) => addr,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let res = with_socket_mut(socket_ptr, |inner| match &inner.kind {
            MoltSocketKind::Pending(sock) => sock.bind(&sockaddr),
            MoltSocketKind::TcpListener(_) | MoltSocketKind::TcpStream(_) => Err(
                std::io::Error::new(ErrorKind::InvalidInput, "socket already bound"),
            ),
            MoltSocketKind::UdpSocket(sock) => {
                #[cfg(unix)]
                let raw = sock.as_raw_fd();
                #[cfg(windows)]
                let raw = sock.as_raw_socket();
                with_sockref(raw, |sock_ref| sock_ref.bind(&sockaddr))
            }
            #[cfg(unix)]
            MoltSocketKind::UnixStream(_) | MoltSocketKind::UnixListener(_) => Err(
                std::io::Error::new(ErrorKind::InvalidInput, "socket already bound"),
            ),
            #[cfg(unix)]
            MoltSocketKind::UnixDatagram(sock) => {
                #[cfg(unix)]
                let raw = sock.as_raw_fd();
                #[cfg(windows)]
                let raw = sock.as_raw_socket();
                with_sockref(raw, |sock_ref| sock_ref.bind(&sockaddr))
            }
            MoltSocketKind::Closed => Err(std::io::Error::new(
                ErrorKind::NotConnected,
                "socket closed",
            )),
        });
        match res {
            Ok(_) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "bind"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_listen(sock_bits: u64, backlog_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let backlog = to_i64(obj_from_bits(backlog_bits)).unwrap_or(128).max(0) as i32;
        let res = with_socket_mut(socket_ptr, |inner| {
            match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
                MoltSocketKind::Pending(sock) => match sock.listen(backlog) {
                    Ok(_) => {
                        #[cfg(unix)]
                        if inner.family == libc::AF_UNIX {
                            let raw_fd = sock.into_raw_fd();
                            let std_listener =
                                unsafe { std::os::unix::net::UnixListener::from_raw_fd(raw_fd) };
                            std_listener.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::UnixListener(
                                mio::net::UnixListener::from_std(std_listener),
                            );
                        } else {
                            let std_listener: std::net::TcpListener = sock.into();
                            std_listener.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpListener(
                                mio::net::TcpListener::from_std(std_listener),
                            );
                        }
                        #[cfg(not(unix))]
                        {
                            let std_listener: std::net::TcpListener = sock.into();
                            std_listener.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpListener(
                                mio::net::TcpListener::from_std(std_listener),
                            );
                        }
                        Ok(())
                    }
                    Err(err) => {
                        inner.kind = MoltSocketKind::Pending(sock);
                        Err(err)
                    }
                },
                MoltSocketKind::TcpListener(listener) => {
                    let res = {
                        #[cfg(unix)]
                        {
                            socket_relisten(listener.as_raw_fd(), backlog)
                        }
                        #[cfg(windows)]
                        {
                            socket_relisten(listener.as_raw_socket(), backlog)
                        }
                    };
                    inner.kind = MoltSocketKind::TcpListener(listener);
                    res
                }
                #[cfg(unix)]
                MoltSocketKind::UnixListener(listener) => {
                    let res = socket_relisten(listener.as_raw_fd(), backlog);
                    inner.kind = MoltSocketKind::UnixListener(listener);
                    res
                }
                other => {
                    inner.kind = other;
                    Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        "socket not in listenable state",
                    ))
                }
            }
        });
        match res {
            Ok(_) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "listen"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_accept(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        loop {
            let timeout = socket_timeout(socket_ptr);
            let (accepted_kind, addr_bits, family) = {
                let socket = &*(socket_ptr as *mut MoltSocket);
                let mut guard = socket.inner.lock().unwrap();
                match &mut guard.kind {
                    MoltSocketKind::TcpListener(listener) => match listener.accept() {
                        Ok((stream, addr)) => (
                            MoltSocketKind::TcpStream(stream),
                            sockaddr_to_bits(_py, &SockAddr::from(addr)),
                            guard.family,
                        ),
                        Err(err) => {
                            if err.kind() == ErrorKind::WouldBlock {
                                (MoltSocketKind::Closed, 0, guard.family)
                            } else {
                                return raise_os_error::<u64>(_py, err, "accept");
                            }
                        }
                    },
                    #[cfg(unix)]
                    MoltSocketKind::UnixListener(listener) => match listener.accept() {
                        Ok((stream, addr)) => {
                            let addr_bits = if let Some(path) = addr.as_pathname() {
                                let text = path.to_string_lossy();
                                let ptr = alloc_string(_py, text.as_bytes());
                                if ptr.is_null() {
                                    MoltObject::none().bits()
                                } else {
                                    MoltObject::from_ptr(ptr).bits()
                                }
                            } else {
                                MoltObject::none().bits()
                            };
                            (MoltSocketKind::UnixStream(stream), addr_bits, guard.family)
                        }
                        Err(err) => {
                            if err.kind() == ErrorKind::WouldBlock {
                                (MoltSocketKind::Closed, 0, guard.family)
                            } else {
                                return raise_os_error::<u64>(_py, err, "accept");
                            }
                        }
                    },
                    _ => {
                        return raise_os_error_errno::<u64>(
                            _py,
                            libc::EINVAL as i64,
                            "socket not listening",
                        )
                    }
                }
            };
            if addr_bits == 0 {
                if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(_py, wait_err, "accept");
                }
                continue;
            }
            let socket = Box::new(MoltSocket {
                inner: Mutex::new(MoltSocketInner {
                    kind: accepted_kind,
                    family,
                    sock_type: libc::SOCK_STREAM,
                    proto: 0,
                    connect_pending: false,
                }),
                timeout: Mutex::new(timeout),
                closed: AtomicBool::new(false),
                refs: AtomicUsize::new(1),
            });
            let socket_ptr = Box::into_raw(socket) as *mut u8;
            socket_register_fd(socket_ptr);
            let handle_bits = bits_from_ptr(socket_ptr);
            let tuple_ptr = alloc_tuple(_py, &[handle_bits, addr_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_connect(sock_bits: u64, addr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let family = {
            let socket = &*(socket_ptr as *mut MoltSocket);
            let guard = socket.inner.lock().unwrap();
            guard.family
        };
        let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
            Ok(addr) => addr,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let timeout = socket_timeout(socket_ptr);
        let res = with_socket_mut(socket_ptr, |inner| {
            if inner.connect_pending {
                return Ok(true);
            }
            match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
                MoltSocketKind::Pending(sock) => match sock.connect(&sockaddr) {
                    Ok(_) => {
                        #[cfg(unix)]
                        if inner.family == libc::AF_UNIX {
                            let raw_fd = sock.into_raw_fd();
                            let std_stream =
                                unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::UnixStream(
                                mio::net::UnixStream::from_std(std_stream),
                            );
                        } else {
                            let std_stream: std::net::TcpStream = sock.into();
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(
                                std_stream,
                            ));
                        }
                        #[cfg(not(unix))]
                        {
                            let std_stream: std::net::TcpStream = sock.into();
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(
                                std_stream,
                            ));
                        }
                        inner.connect_pending = false;
                        Ok(false)
                    }
                    Err(err) => {
                        if err.kind() == ErrorKind::WouldBlock
                            || err.raw_os_error() == Some(libc::EINPROGRESS)
                        {
                            #[cfg(unix)]
                            if inner.family == libc::AF_UNIX {
                                let raw_fd = sock.into_raw_fd();
                                let std_stream =
                                    unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::UnixStream(
                                    mio::net::UnixStream::from_std(std_stream),
                                );
                            } else {
                                let std_stream: std::net::TcpStream = sock.into();
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::TcpStream(
                                    mio::net::TcpStream::from_std(std_stream),
                                );
                            }
                            #[cfg(not(unix))]
                            {
                                let std_stream: std::net::TcpStream = sock.into();
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::TcpStream(
                                    mio::net::TcpStream::from_std(std_stream),
                                );
                            }
                            inner.connect_pending = true;
                        } else {
                            inner.kind = MoltSocketKind::Pending(sock);
                        }
                        Err(err)
                    }
                },
                MoltSocketKind::UdpSocket(sock) => {
                    #[cfg(unix)]
                    let fd = sock.as_raw_fd();
                    #[cfg(windows)]
                    let fd = sock.as_raw_socket();
                    let res = connect_raw_socket(fd, &sockaddr);
                    inner.kind = MoltSocketKind::UdpSocket(sock);
                    match res {
                        Ok(()) => {
                            inner.connect_pending = false;
                            Ok(false)
                        }
                        Err(err) => Err(err),
                    }
                }
                #[cfg(unix)]
                MoltSocketKind::UnixDatagram(sock) => {
                    let fd = sock.as_raw_fd();
                    let res = connect_raw_socket(fd, &sockaddr);
                    inner.kind = MoltSocketKind::UnixDatagram(sock);
                    match res {
                        Ok(()) => {
                            inner.connect_pending = false;
                            Ok(false)
                        }
                        Err(err) => Err(err),
                    }
                }
                other => {
                    inner.kind = other;
                    Err(std::io::Error::new(
                        ErrorKind::InvalidInput,
                        "socket already connected",
                    ))
                }
            }
        });
        match res {
            Ok(pending) => {
                if pending {
                    if let Err(err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if err.kind() == ErrorKind::TimedOut {
                            return raise_os_error_errno::<u64>(
                                _py,
                                libc::ETIMEDOUT as i64,
                                "timed out",
                            );
                        }
                        if err.kind() == ErrorKind::WouldBlock {
                            return raise_os_error_errno::<u64>(
                                _py,
                                libc::EINPROGRESS as i64,
                                "operation in progress",
                            );
                        }
                        return raise_os_error::<u64>(_py, err, "connect");
                    }
                    let err = with_socket_mut(socket_ptr, |inner| match &inner.kind {
                        MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                        #[cfg(unix)]
                        MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                        _ => Ok(None),
                    });
                    match err {
                        Ok(None) => MoltObject::none().bits(),
                        Ok(Some(err)) => raise_os_error::<u64>(_py, err, "connect"),
                        Err(err) => raise_os_error::<u64>(_py, err, "connect"),
                    }
                } else {
                    MoltObject::none().bits()
                }
            }
            Err(err)
                if err.kind() == ErrorKind::WouldBlock
                    || err.raw_os_error() == Some(libc::EINPROGRESS) =>
            {
                match timeout {
                    Some(val) if val == Duration::ZERO => raise_os_error_errno::<u64>(
                        _py,
                        libc::EINPROGRESS as i64,
                        "operation in progress",
                    ),
                    _ => {
                        if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                            if wait_err.kind() == ErrorKind::TimedOut {
                                return raise_os_error_errno::<u64>(
                                    _py,
                                    libc::ETIMEDOUT as i64,
                                    "timed out",
                                );
                            }
                            return raise_os_error::<u64>(_py, wait_err, "connect");
                        }
                        let err = with_socket_mut(socket_ptr, |inner| match &inner.kind {
                            MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                            #[cfg(unix)]
                            MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                            _ => Ok(None),
                        });
                        match err {
                            Ok(None) => MoltObject::none().bits(),
                            Ok(Some(err)) => raise_os_error::<u64>(_py, err, "connect"),
                            Err(err) => raise_os_error::<u64>(_py, err, "connect"),
                        }
                    }
                }
            }
            Err(err) => raise_os_error::<u64>(_py, err, "connect"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_connect_ex(sock_bits: u64, addr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_int(libc::EBADF as i64).bits();
        }
        let family = {
            let socket = &*(socket_ptr as *mut MoltSocket);
            let guard = socket.inner.lock().unwrap();
            guard.family
        };
        let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
            Ok(addr) => addr,
            Err(_msg) => return MoltObject::from_int(libc::EAFNOSUPPORT as i64).bits(),
        };
        let timeout = socket_timeout(socket_ptr);
        enum ConnectExOutcome {
            Done(i64),
            Pending(i64),
        }
        let res = with_socket_mut(socket_ptr, |inner| {
            if inner.connect_pending {
                let err = match &inner.kind {
                    MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                    #[cfg(unix)]
                    MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                    _ => Ok(None),
                };
                return match err {
                    Ok(None) => {
                        inner.connect_pending = false;
                        Ok(ConnectExOutcome::Done(0))
                    }
                    Ok(Some(err)) => {
                        inner.connect_pending = false;
                        Ok(ConnectExOutcome::Done(
                            err.raw_os_error().unwrap_or(libc::EIO) as i64,
                        ))
                    }
                    Err(err) => Ok(ConnectExOutcome::Done(
                        err.raw_os_error().unwrap_or(libc::EIO) as i64,
                    )),
                };
            }
            match std::mem::replace(&mut inner.kind, MoltSocketKind::Closed) {
                MoltSocketKind::Pending(sock) => match sock.connect(&sockaddr) {
                    Ok(_) => {
                        #[cfg(unix)]
                        if inner.family == libc::AF_UNIX {
                            let raw_fd = sock.into_raw_fd();
                            let std_stream =
                                unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::UnixStream(
                                mio::net::UnixStream::from_std(std_stream),
                            );
                        } else {
                            let std_stream: std::net::TcpStream = sock.into();
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(
                                std_stream,
                            ));
                        }
                        #[cfg(not(unix))]
                        {
                            let std_stream: std::net::TcpStream = sock.into();
                            std_stream.set_nonblocking(true)?;
                            inner.kind = MoltSocketKind::TcpStream(mio::net::TcpStream::from_std(
                                std_stream,
                            ));
                        }
                        inner.connect_pending = false;
                        Ok(ConnectExOutcome::Done(0))
                    }
                    Err(err) => {
                        let errno = err.raw_os_error().unwrap_or(libc::EIO);
                        if err.kind() == ErrorKind::WouldBlock
                            || errno == libc::EINPROGRESS
                            || errno == libc::EALREADY
                        {
                            #[cfg(unix)]
                            if inner.family == libc::AF_UNIX {
                                let raw_fd = sock.into_raw_fd();
                                let std_stream =
                                    unsafe { std::os::unix::net::UnixStream::from_raw_fd(raw_fd) };
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::UnixStream(
                                    mio::net::UnixStream::from_std(std_stream),
                                );
                            } else {
                                let std_stream: std::net::TcpStream = sock.into();
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::TcpStream(
                                    mio::net::TcpStream::from_std(std_stream),
                                );
                            }
                            #[cfg(not(unix))]
                            {
                                let std_stream: std::net::TcpStream = sock.into();
                                std_stream.set_nonblocking(true)?;
                                inner.kind = MoltSocketKind::TcpStream(
                                    mio::net::TcpStream::from_std(std_stream),
                                );
                            }
                            inner.connect_pending = true;
                            Ok(ConnectExOutcome::Pending(errno as i64))
                        } else {
                            inner.kind = MoltSocketKind::Pending(sock);
                            Ok(ConnectExOutcome::Done(errno as i64))
                        }
                    }
                },
                MoltSocketKind::UdpSocket(sock) => {
                    #[cfg(unix)]
                    let fd = sock.as_raw_fd();
                    #[cfg(windows)]
                    let fd = sock.as_raw_socket();
                    let res = connect_raw_socket(fd, &sockaddr);
                    inner.kind = MoltSocketKind::UdpSocket(sock);
                    match res {
                        Ok(()) => Ok(ConnectExOutcome::Done(0)),
                        Err(err) => Ok(ConnectExOutcome::Done(
                            err.raw_os_error().unwrap_or(libc::EIO) as i64,
                        )),
                    }
                }
                #[cfg(unix)]
                MoltSocketKind::UnixDatagram(sock) => {
                    let fd = sock.as_raw_fd();
                    let res = connect_raw_socket(fd, &sockaddr);
                    inner.kind = MoltSocketKind::UnixDatagram(sock);
                    match res {
                        Ok(()) => Ok(ConnectExOutcome::Done(0)),
                        Err(err) => Ok(ConnectExOutcome::Done(
                            err.raw_os_error().unwrap_or(libc::EIO) as i64,
                        )),
                    }
                }
                other => {
                    inner.kind = other;
                    Ok(ConnectExOutcome::Done(libc::EISCONN as i64))
                }
            }
        });
        match res {
            Ok(ConnectExOutcome::Done(val)) => MoltObject::from_int(val).bits(),
            Ok(ConnectExOutcome::Pending(val)) => {
                if matches!(timeout, Some(val) if val == Duration::ZERO) {
                    return MoltObject::from_int(val).bits();
                }
                if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return MoltObject::from_int(libc::ETIMEDOUT as i64).bits();
                    }
                    if wait_err.kind() == ErrorKind::WouldBlock {
                        return MoltObject::from_int(libc::EINPROGRESS as i64).bits();
                    }
                    return MoltObject::from_int(
                        wait_err.raw_os_error().unwrap_or(libc::EIO) as i64
                    )
                    .bits();
                }
                let err = with_socket_mut(socket_ptr, |inner| {
                    let err = match &inner.kind {
                        MoltSocketKind::TcpStream(stream) => take_error_mio(stream),
                        #[cfg(unix)]
                        MoltSocketKind::UnixStream(stream) => take_error_mio(stream),
                        _ => Ok(None),
                    };
                    inner.connect_pending = false;
                    err
                });
                match err {
                    Ok(None) => MoltObject::from_int(0).bits(),
                    Ok(Some(err)) => {
                        MoltObject::from_int(err.raw_os_error().unwrap_or(libc::EIO) as i64).bits()
                    }
                    Err(err) => {
                        MoltObject::from_int(err.raw_os_error().unwrap_or(libc::EIO) as i64).bits()
                    }
                }
            }
            Err(err) => MoltObject::from_int(err.raw_os_error().unwrap_or(libc::EIO) as i64).bits(),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_recv(sock_bits: u64, size_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let size = to_i64(obj_from_bits(size_bits)).unwrap_or(0).max(0) as usize;
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        if size == 0 {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        let mut buf = vec![0u8; size];
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let ret = unsafe {
                    libc::recv(
                        libc_socket(fd),
                        buf.as_mut_ptr() as *mut c_void,
                        buf.len(),
                        flags,
                    )
                };
                if ret >= 0 {
                    Ok(ret as usize)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok(n) => {
                    if trace_socket_recv() {
                        let fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                        eprintln!("molt socket recv: fd={} len={}", fd, n);
                    }
                    let ptr = alloc_bytes(_py, &buf[..n]);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                Err(err) => {
                    let raw = err.raw_os_error();
                    let would_block_raw = matches!(
                        raw,
                        Some(code) if code == libc::EAGAIN || code == libc::EWOULDBLOCK
                    );
                    let would_block = err.kind() == ErrorKind::WouldBlock || would_block_raw;
                    if trace_socket_recv() {
                        let fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                        eprintln!(
                            "molt socket recv error: fd={} kind={:?} raw={raw:?} dontwait={dontwait} msg={}",
                            fd,
                            err.kind(),
                            err
                        );
                    }
                    if would_block {
                        if dontwait || nonblocking {
                            let errno = raw.unwrap_or(libc::EWOULDBLOCK) as i64;
                            return raise_os_error_errno::<u64>(_py, errno, "recv: would block");
                        }
                        if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                            if wait_err.kind() == ErrorKind::TimedOut {
                                return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                            }
                            return raise_os_error::<u64>(_py, wait_err, "recv");
                        }
                        continue;
                    }
                    return raise_os_error::<u64>(_py, err, "recv");
                }
            }
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_recv_into(
    sock_bits: u64,
    buffer_bits: u64,
    size_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_int(0).bits();
        }
        let buffer_obj = obj_from_bits(buffer_bits);
        let buffer_ptr = buffer_obj.as_ptr();
        if buffer_ptr.is_none() {
            return raise_exception::<_>(_py, "TypeError", "recv_into requires a writable buffer");
        }
        let buffer_ptr = buffer_ptr.unwrap();
        let size = to_i64(obj_from_bits(size_bits)).unwrap_or(-1);
        let target_len;
        let mut use_memoryview = false;
        let type_id = unsafe { object_type_id(buffer_ptr) };
        if type_id == TYPE_ID_BYTEARRAY {
            target_len = unsafe { bytearray_len(buffer_ptr) };
        } else if type_id == TYPE_ID_MEMORYVIEW {
            if unsafe { memoryview_readonly(buffer_ptr) } {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recv_into requires a writable buffer",
                );
            }
            target_len = unsafe { memoryview_len(buffer_ptr) };
            use_memoryview = true;
        } else {
            return raise_exception::<_>(_py, "TypeError", "recv_into requires a writable buffer");
        }
        let size = if size < 0 {
            target_len
        } else {
            (size as usize).min(target_len)
        };
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                if use_memoryview {
                    if let Some(slice) = unsafe { memoryview_bytes_slice_mut(buffer_ptr) } {
                        let len = size.min(slice.len());
                        let ret = unsafe {
                            libc::recv(
                                libc_socket(fd),
                                slice.as_mut_ptr() as *mut c_void,
                                len,
                                flags,
                            )
                        };
                        if ret >= 0 {
                            Ok((ret as usize, None))
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    } else {
                        let mut tmp = vec![0u8; size];
                        let ret = unsafe {
                            libc::recv(
                                libc_socket(fd),
                                tmp.as_mut_ptr() as *mut c_void,
                                tmp.len(),
                                flags,
                            )
                        };
                        if ret >= 0 {
                            Ok((ret as usize, Some(tmp)))
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    }
                } else {
                    let buf = bytearray_vec(buffer_ptr);
                    let ret = unsafe {
                        libc::recv(
                            libc_socket(fd),
                            buf.as_mut_ptr() as *mut c_void,
                            size,
                            flags,
                        )
                    };
                    if ret >= 0 {
                        Ok((ret as usize, None))
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                }
            });
            match res {
                Ok((n, tmp)) => {
                    if use_memoryview {
                        if let Some(tmp) = tmp.as_ref() {
                            if let Err(msg) = memoryview_write_bytes(buffer_ptr, &tmp[..n]) {
                                return raise_exception::<u64>(_py, "TypeError", &msg);
                            }
                        }
                    }
                    return MoltObject::from_int(n as i64).bits();
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if dontwait {
                        return raise_os_error::<u64>(_py, err, "recv_into");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        return raise_os_error::<u64>(_py, wait_err, "recv_into");
                    }
                    continue;
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "recv_into"),
            }
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_send(sock_bits: u64, data_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_int(0).bits();
        }
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        if data_len == 0 {
            return MoltObject::from_int(0).bits();
        }
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let ret = unsafe {
                    libc::send(libc_socket(fd), data_ptr as *const c_void, data_len, flags)
                };
                if ret >= 0 {
                    Ok(ret as usize)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok(n) => {
                    if trace_socket_send() {
                        let fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                        eprintln!("molt socket send: fd={} len={} sent={}", fd, data_len, n);
                    }
                    return MoltObject::from_int(n as i64).bits();
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if trace_socket_send() {
                        let fd = socket_debug_fd(socket_ptr).unwrap_or(-1);
                        eprintln!(
                            "molt socket send would_block: fd={} nonblocking={} dontwait={}",
                            fd, nonblocking, dontwait
                        );
                    }
                    if dontwait || nonblocking {
                        return raise_os_error::<u64>(_py, err, "send");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        return raise_os_error::<u64>(_py, wait_err, "send");
                    }
                    continue;
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "send"),
            }
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_sendall(
    sock_bits: u64,
    data_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        let mut offset = 0usize;
        while offset < data_len {
            let slice_ptr = unsafe { data_ptr.add(offset) };
            let slice_len = data_len - offset;
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let ret = unsafe {
                    libc::send(
                        libc_socket(fd),
                        slice_ptr as *const c_void,
                        slice_len,
                        flags,
                    )
                };
                if ret >= 0 {
                    Ok(ret as usize)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok(0) => {
                    return raise_os_error_errno::<u64>(_py, libc::EPIPE as i64, "broken pipe")
                }
                Ok(n) => offset += n,
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if dontwait || nonblocking {
                        return raise_os_error::<u64>(_py, err, "sendall");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        return raise_os_error::<u64>(_py, wait_err, "sendall");
                    }
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "sendall"),
            }
        }
        MoltObject::none().bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_sendto(
    sock_bits: u64,
    data_bits: u64,
    flags_bits: u64,
    addr_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_int(0).bits();
        }
        let family = {
            let socket = &*(socket_ptr as *mut MoltSocket);
            let guard = socket.inner.lock().unwrap();
            guard.family
        };
        let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
            Ok(addr) => addr,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(socket_ptr), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let ret = unsafe {
                    libc::sendto(
                        libc_socket(fd),
                        data_ptr as *const c_void,
                        data_len,
                        flags,
                        sockaddr.as_ptr(),
                        sockaddr.len(),
                    )
                };
                if ret >= 0 {
                    Ok(ret as usize)
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok(n) => return MoltObject::from_int(n as i64).bits(),
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if dontwait || nonblocking {
                        return raise_os_error::<u64>(_py, err, "sendto");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_WRITE) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        return raise_os_error::<u64>(_py, wait_err, "sendto");
                    }
                    continue;
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "sendto"),
            }
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_recvfrom(
    sock_bits: u64,
    size_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let size = to_i64(obj_from_bits(size_bits)).unwrap_or(0).max(0) as usize;
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let mut buf = vec![0u8; size];
        loop {
            let res = with_socket_mut(socket_ptr, |inner| {
                #[cfg(unix)]
                let fd = inner
                    .raw_fd()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                #[cfg(windows)]
                let fd = inner
                    .raw_socket()
                    .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
                let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
                let ret = unsafe {
                    libc::recvfrom(
                        libc_socket(fd),
                        buf.as_mut_ptr() as *mut c_void,
                        buf.len(),
                        flags,
                        &mut storage as *mut _ as *mut libc::sockaddr,
                        &mut len,
                    )
                };
                if ret >= 0 {
                    let addr = unsafe { SockAddr::new(storage, len) };
                    Ok((ret as usize, addr))
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
            match res {
                Ok((n, addr)) => {
                    let data_ptr = alloc_bytes(_py, &buf[..n]);
                    if data_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let data_bits = MoltObject::from_ptr(data_ptr).bits();
                    let addr_bits = sockaddr_to_bits(_py, &addr);
                    let tuple_ptr = alloc_tuple(_py, &[data_bits, addr_bits]);
                    dec_ref_bits(_py, data_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                Err(err) if err.kind() == ErrorKind::WouldBlock => {
                    if dontwait {
                        return raise_os_error::<u64>(_py, err, "recvfrom");
                    }
                    if let Err(wait_err) = socket_wait_ready(_py, socket_ptr, IO_EVENT_READ) {
                        if wait_err.kind() == ErrorKind::TimedOut {
                            return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                        }
                        return raise_os_error::<u64>(_py, wait_err, "recvfrom");
                    }
                    continue;
                }
                Err(err) => return raise_os_error::<u64>(_py, err, "recvfrom"),
            }
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_shutdown(sock_bits: u64, how_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let how = to_i64(obj_from_bits(how_bits)).unwrap_or(2) as i32;
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let ret = unsafe { libc::shutdown(libc_socket(fd), how) };
            if ret == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(_) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "shutdown"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getsockname(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let ret = unsafe {
                libc::getsockname(
                    libc_socket(fd),
                    &mut storage as *mut _ as *mut libc::sockaddr,
                    &mut len,
                )
            };
            if ret == 0 {
                Ok(unsafe { SockAddr::new(storage, len) })
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(addr) => sockaddr_to_bits(_py, &addr),
            Err(err) => raise_os_error::<u64>(_py, err, "getsockname"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getpeername(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let ret = unsafe {
                libc::getpeername(
                    libc_socket(fd),
                    &mut storage as *mut _ as *mut libc::sockaddr,
                    &mut len,
                )
            };
            if ret == 0 {
                Ok(unsafe { SockAddr::new(storage, len) })
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
        match res {
            Ok(addr) => sockaddr_to_bits(_py, &addr),
            Err(err) => raise_os_error::<u64>(_py, err, "getpeername"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_setsockopt(
    sock_bits: u64,
    level_bits: u64,
    opt_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(0) as i32;
        let optname = to_i64(obj_from_bits(opt_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(value_bits);
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            if let Some(val) = to_i64(obj) {
                let val = val as c_int;
                let ret = unsafe {
                    libc::setsockopt(
                        libc_socket(fd),
                        level,
                        optname,
                        &val as *const _ as *const c_void,
                        std::mem::size_of::<c_int>() as libc::socklen_t,
                    )
                };
                if ret == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            } else if let Some(ptr) = obj.as_ptr() {
                let bytes = unsafe { bytes_like_slice_raw(ptr) }.ok_or_else(|| {
                    std::io::Error::new(ErrorKind::InvalidInput, "invalid optval")
                })?;
                let ret = unsafe {
                    libc::setsockopt(
                        libc_socket(fd),
                        level,
                        optname,
                        bytes.as_ptr() as *const c_void,
                        bytes.len() as libc::socklen_t,
                    )
                };
                if ret == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            } else {
                Err(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "invalid optval",
                ))
            }
        });
        match res {
            Ok(_) => MoltObject::none().bits(),
            Err(err) => raise_os_error::<u64>(_py, err, "setsockopt"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getsockopt(
    sock_bits: u64,
    level_bits: u64,
    opt_bits: u64,
    buflen_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(0) as i32;
        let optname = to_i64(obj_from_bits(opt_bits)).unwrap_or(0) as i32;
        let buflen = if obj_from_bits(buflen_bits).is_none() {
            None
        } else {
            Some(to_i64(obj_from_bits(buflen_bits)).unwrap_or(0).max(0) as usize)
        };
        let res = with_socket_mut(socket_ptr, |inner| {
            #[cfg(unix)]
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            #[cfg(windows)]
            let fd = inner
                .raw_socket()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            if let Some(buflen) = buflen {
                let mut buf = vec![0u8; buflen];
                let mut len = buflen as libc::socklen_t;
                let ret = unsafe {
                    libc::getsockopt(
                        libc_socket(fd),
                        level,
                        optname,
                        buf.as_mut_ptr() as *mut c_void,
                        &mut len,
                    )
                };
                if ret == 0 {
                    let ptr = alloc_bytes(_py, &buf[..len as usize]);
                    if ptr.is_null() {
                        Err(std::io::Error::other("allocation failed"))
                    } else {
                        Ok(MoltObject::from_ptr(ptr).bits())
                    }
                } else {
                    Err(std::io::Error::last_os_error())
                }
            } else {
                let mut val: c_int = 0;
                let mut len = std::mem::size_of::<c_int>() as libc::socklen_t;
                let ret = unsafe {
                    libc::getsockopt(
                        libc_socket(fd),
                        level,
                        optname,
                        &mut val as *mut _ as *mut c_void,
                        &mut len,
                    )
                };
                if ret == 0 {
                    Ok(MoltObject::from_int(val as i64).bits())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            }
        });
        match res {
            Ok(bits) => bits,
            Err(err) => raise_os_error::<u64>(_py, err, "getsockopt"),
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_detach(sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let socket_ptr = ptr_from_bits(sock_bits);
        if socket_ptr.is_null() {
            return MoltObject::from_int(-1).bits();
        }
        let socket = &*(socket_ptr as *mut MoltSocket);
        socket_unregister_fd(socket_ptr);
        runtime_state(_py)
            .io_poller()
            .deregister_socket(_py, socket_ptr);
        socket.closed.store(true, AtomicOrdering::Relaxed);
        let raw = {
            let mut guard = socket.inner.lock().unwrap();
            let kind = std::mem::replace(&mut guard.kind, MoltSocketKind::Closed);
            #[cfg(unix)]
            {
                match kind {
                    MoltSocketKind::Pending(sock) => sock.into_raw_fd() as i64,
                    MoltSocketKind::TcpStream(sock) => sock.into_raw_fd() as i64,
                    MoltSocketKind::TcpListener(sock) => sock.into_raw_fd() as i64,
                    MoltSocketKind::UdpSocket(sock) => sock.into_raw_fd() as i64,
                    MoltSocketKind::UnixStream(sock) => sock.into_raw_fd() as i64,
                    MoltSocketKind::UnixListener(sock) => sock.into_raw_fd() as i64,
                    MoltSocketKind::UnixDatagram(sock) => sock.into_raw_fd() as i64,
                    MoltSocketKind::Closed => -1,
                }
            }
            #[cfg(windows)]
            {
                match kind {
                    MoltSocketKind::Pending(sock) => sock.into_raw_socket() as i64,
                    MoltSocketKind::TcpStream(sock) => sock.into_raw_socket() as i64,
                    MoltSocketKind::TcpListener(sock) => sock.into_raw_socket() as i64,
                    MoltSocketKind::UdpSocket(sock) => sock.into_raw_socket() as i64,
                    MoltSocketKind::Closed => -1,
                }
            }
        };
        MoltObject::from_int(raw).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
fn wasm_socket_unavailable<T: ExceptionSentinel>(_py: &PyToken<'_>) -> T {
    raise_exception(_py, "RuntimeError", "socket unsupported on wasm")
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_new(
    _family_bits: u64,
    _type_bits: u64,
    _proto_bits: u64,
    _fileno_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.listen", "net.bind"])
            .is_err()
        {
            return MoltObject::none().bits();
        }
        let family = match to_i64(obj_from_bits(_family_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>(_py, "TypeError", "family must be int"),
        };
        let sock_type = match to_i64(obj_from_bits(_type_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>(_py, "TypeError", "type must be int"),
        };
        let proto = to_i64(obj_from_bits(_proto_bits)).unwrap_or(0) as i32;
        let fileno = if obj_from_bits(_fileno_bits).is_none() {
            -1
        } else {
            match to_i64(obj_from_bits(_fileno_bits)) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(_py, "TypeError", "fileno must be int or None")
                }
            }
        };
        #[cfg(unix)]
        let base_type = sock_type & !(SOCK_NONBLOCK_FLAG | SOCK_CLOEXEC_FLAG);
        #[cfg(not(unix))]
        let base_type = sock_type;
        let timeout = {
            #[cfg(unix)]
            {
                if (sock_type & SOCK_NONBLOCK_FLAG) != 0 {
                    Some(Duration::ZERO)
                } else {
                    None
                }
            }
            #[cfg(not(unix))]
            {
                None
            }
        };
        let handle = unsafe { crate::molt_socket_new_host(family, base_type, proto, fileno) };
        if handle < 0 {
            return raise_os_error_errno::<u64>(_py, (-handle) as i64, "socket");
        }
        wasm_socket_meta_insert(
            handle,
            WasmSocketMeta {
                family,
                sock_type: base_type,
                proto,
                timeout,
                connect_pending: false,
            },
        );
        MoltObject::from_int(handle).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_close(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let rc = unsafe { crate::molt_socket_close_host(handle) };
        wasm_socket_meta_remove(handle);
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, (-rc) as i64, "close");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_drop(_sock_bits: u64) {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return,
        };
        let _ = unsafe { crate::molt_socket_close_host(handle) };
        wasm_socket_meta_remove(handle);
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_fileno(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(-1).bits(),
        };
        MoltObject::from_int(handle).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_gettimeout(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        match socket_timeout(handle) {
            None => MoltObject::none().bits(),
            Some(val) => MoltObject::from_float(val.as_secs_f64()).bits(),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_settimeout(_sock_bits: u64, _timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let obj = obj_from_bits(_timeout_bits);
        if obj.is_none() {
            let _ = socket_set_timeout(handle, None);
            return MoltObject::none().bits();
        }
        let Some(timeout) = to_f64(obj) else {
            return raise_exception::<_>(_py, "TypeError", "timeout must be float or None");
        };
        if !timeout.is_finite() || timeout < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "timeout must be non-negative");
        }
        let duration = if timeout == 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(timeout)
        };
        if let Err(msg) = socket_set_timeout(handle, Some(duration)) {
            return raise_exception::<_>(_py, "RuntimeError", &msg);
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_setblocking(_sock_bits: u64, _flag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let flag = obj_from_bits(_flag_bits).as_bool().unwrap_or(false);
        if flag {
            let _ = socket_set_timeout(handle, None);
        } else {
            let _ = socket_set_timeout(handle, Some(Duration::ZERO));
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getblocking(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_bool(false).bits(),
        };
        let timeout = socket_timeout(handle);
        let blocking = match timeout {
            None => true,
            Some(val) if val == Duration::ZERO => false,
            Some(_) => true,
        };
        MoltObject::from_bool(blocking).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_bind(_sock_bits: u64, _addr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.bind", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let addr = match encode_sockaddr(_py, _addr_bits, family) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let rc = unsafe {
            crate::molt_socket_bind_host(handle, addr.as_ptr() as u32, addr.len() as u32)
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "bind");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_listen(_sock_bits: u64, _backlog_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let backlog = to_i64(obj_from_bits(_backlog_bits)).unwrap_or(0) as i32;
        let rc = unsafe { crate::molt_socket_listen_host(handle, backlog) };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "listen");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_accept(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let timeout = socket_timeout(handle);
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        loop {
            let rc = unsafe {
                crate::molt_socket_accept_host(
                    handle,
                    addr_buf.as_mut_ptr() as u32,
                    addr_buf.len() as u32,
                    (&mut addr_len) as *mut u32 as u32,
                )
            };
            if rc >= 0 {
                let new_handle = rc;
                wasm_socket_meta_insert(
                    new_handle,
                    WasmSocketMeta {
                        family,
                        sock_type: libc::SOCK_STREAM,
                        proto: 0,
                        timeout,
                        connect_pending: false,
                    },
                );
                let addr_bits = match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
                    Ok(bits) => bits,
                    Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
                };
                let handle_bits = MoltObject::from_int(new_handle).bits();
                let tuple_ptr = alloc_tuple(_py, &[handle_bits, addr_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            let errno = errno_from_rc(rc as i32);
            if would_block_errno(errno) {
                if let Some(val) = timeout {
                    if val == Duration::ZERO {
                        return raise_os_error_errno::<u64>(
                            _py,
                            libc::EWOULDBLOCK as i64,
                            "accept would block",
                        );
                    }
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(_py, wait_err, "accept");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "accept");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_connect(_sock_bits: u64, _addr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let addr = match encode_sockaddr(_py, _addr_bits, family) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let timeout = socket_timeout(handle);
        let rc = unsafe {
            crate::molt_socket_connect_host(handle, addr.as_ptr() as u32, addr.len() as u32)
        };
        if rc == 0 {
            let _ = socket_set_connect_pending(handle, false);
            return MoltObject::none().bits();
        }
        let errno = errno_from_rc(rc);
        if errno == libc::EINPROGRESS || errno == libc::EWOULDBLOCK {
            let _ = socket_set_connect_pending(handle, true);
            if matches!(timeout, Some(val) if val == Duration::ZERO) {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::EINPROGRESS as i64,
                    "operation in progress",
                );
            }
            if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                if wait_err.kind() == ErrorKind::TimedOut {
                    return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                }
                return raise_os_error::<u64>(_py, wait_err, "connect");
            }
            let rc = unsafe { crate::molt_socket_connect_ex_host(handle) };
            if rc == 0 {
                let _ = socket_set_connect_pending(handle, false);
                return MoltObject::none().bits();
            }
            let err = errno_from_rc(rc);
            let _ = socket_set_connect_pending(handle, false);
            return raise_os_error_errno::<u64>(_py, err as i64, "connect");
        }
        raise_os_error_errno::<u64>(_py, errno as i64, "connect")
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_connect_ex(_sock_bits: u64, _addr_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(libc::EBADF as i64).bits(),
        };
        let timeout = socket_timeout(handle);
        if socket_connect_pending(handle) {
            let rc = unsafe { crate::molt_socket_connect_ex_host(handle) };
            let errno = errno_from_rc(rc);
            if errno == 0 {
                let _ = socket_set_connect_pending(handle, false);
            }
            return MoltObject::from_int(errno as i64).bits();
        }
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(libc::EAFNOSUPPORT as i64).bits(),
        };
        let addr = match encode_sockaddr(_py, _addr_bits, family) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(libc::EAFNOSUPPORT as i64).bits(),
        };
        let rc = unsafe {
            crate::molt_socket_connect_host(handle, addr.as_ptr() as u32, addr.len() as u32)
        };
        if rc == 0 {
            let _ = socket_set_connect_pending(handle, false);
            return MoltObject::from_int(0).bits();
        }
        let errno = errno_from_rc(rc);
        if errno == libc::EINPROGRESS || errno == libc::EWOULDBLOCK {
            let _ = socket_set_connect_pending(handle, true);
            if matches!(timeout, Some(val) if val == Duration::ZERO) {
                return MoltObject::from_int(errno as i64).bits();
            }
            if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                if wait_err.kind() == ErrorKind::TimedOut {
                    return MoltObject::from_int(libc::ETIMEDOUT as i64).bits();
                }
                if wait_err.kind() == ErrorKind::WouldBlock {
                    return MoltObject::from_int(libc::EINPROGRESS as i64).bits();
                }
                return MoltObject::from_int(wait_err.raw_os_error().unwrap_or(libc::EIO) as i64)
                    .bits();
            }
            let rc = unsafe { crate::molt_socket_connect_ex_host(handle) };
            let err = errno_from_rc(rc);
            let _ = socket_set_connect_pending(handle, false);
            return MoltObject::from_int(err as i64).bits();
        }
        MoltObject::from_int(errno as i64).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_recv(_sock_bits: u64, _size_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let size = to_i64(obj_from_bits(_size_bits)).unwrap_or(0).max(0) as usize;
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        if size == 0 {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let mut buf = vec![0u8; size];
        loop {
            let rc = unsafe {
                crate::molt_socket_recv_host(
                    handle,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    flags,
                )
            };
            if rc >= 0 {
                let n = rc as usize;
                let ptr = alloc_bytes(_py, &buf[..n]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recv: would block");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recv");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recv");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_recv_into(
    _sock_bits: u64,
    _buffer_bits: u64,
    _size_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(0).bits(),
        };
        let buffer_obj = obj_from_bits(_buffer_bits);
        let buffer_ptr = buffer_obj.as_ptr();
        if buffer_ptr.is_none() {
            return raise_exception::<_>(_py, "TypeError", "recv_into requires a writable buffer");
        }
        let buffer_ptr = buffer_ptr.unwrap();
        let size = to_i64(obj_from_bits(_size_bits)).unwrap_or(-1);
        let target_len;
        let mut use_memoryview = false;
        let type_id = unsafe { object_type_id(buffer_ptr) };
        if type_id == TYPE_ID_BYTEARRAY {
            target_len = unsafe { bytearray_len(buffer_ptr) };
        } else if type_id == TYPE_ID_MEMORYVIEW {
            if unsafe { memoryview_readonly(buffer_ptr) } {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "recv_into requires a writable buffer",
                );
            }
            target_len = unsafe { memoryview_len(buffer_ptr) };
            use_memoryview = true;
        } else {
            return raise_exception::<_>(_py, "TypeError", "recv_into requires a writable buffer");
        }
        let size = if size < 0 {
            target_len
        } else {
            (size as usize).min(target_len)
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        loop {
            let rc = if use_memoryview {
                if let Some(slice) = unsafe { memoryview_bytes_slice_mut(buffer_ptr) } {
                    let len = size.min(slice.len());
                    unsafe {
                        crate::molt_socket_recv_host(
                            handle,
                            slice.as_mut_ptr() as u32,
                            len as u32,
                            flags,
                        )
                    }
                } else {
                    let mut tmp = vec![0u8; size];
                    let res = unsafe {
                        crate::molt_socket_recv_host(
                            handle,
                            tmp.as_mut_ptr() as u32,
                            tmp.len() as u32,
                            flags,
                        )
                    };
                    if res >= 0 {
                        if let Err(msg) =
                            unsafe { memoryview_write_bytes(buffer_ptr, &tmp[..res as usize]) }
                        {
                            return raise_exception::<u64>(_py, "TypeError", &msg);
                        }
                    }
                    res
                }
            } else {
                let buf = unsafe { bytearray_vec(buffer_ptr) };
                unsafe {
                    crate::molt_socket_recv_host(
                        handle,
                        buf.as_mut_ptr() as u32,
                        size as u32,
                        flags,
                    )
                }
            };
            if rc >= 0 {
                return MoltObject::from_int(rc as i64).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recv_into");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recv_into");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recv_into");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_send(_sock_bits: u64, _data_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(0).bits(),
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(_data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        if data_len == 0 {
            return MoltObject::from_int(0).bits();
        }
        loop {
            let rc = unsafe {
                crate::molt_socket_send_host(handle, data_ptr as u32, data_len as u32, flags)
            };
            if rc >= 0 {
                return MoltObject::from_int(rc as i64).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "send");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(_py, wait_err, "send");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "send");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_sendall(_sock_bits: u64, _data_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(_data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        let mut sent = 0usize;
        while sent < data_len {
            let rc = unsafe {
                crate::molt_socket_send_host(
                    handle,
                    data_ptr.add(sent) as u32,
                    (data_len - sent) as u32,
                    flags,
                )
            };
            if rc >= 0 {
                sent += rc as usize;
                continue;
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "sendall");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(_py, wait_err, "sendall");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "sendall");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_sendto(
    _sock_bits: u64,
    _data_bits: u64,
    _flags_bits: u64,
    _addr_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(0).bits(),
        };
        let family = match wasm_socket_family(handle) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let addr = match encode_sockaddr(_py, _addr_bits, family) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let send_data = match send_data_from_bits(_data_bits) {
            Ok(data) => data,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let (data_ptr, data_len, owned): (*const u8, usize, Option<Vec<u8>>) = match send_data {
            SendData::Borrowed(ptr, len) => (ptr, len, None),
            SendData::Owned(vec) => {
                let ptr = vec.as_ptr();
                let len = vec.len();
                (ptr, len, Some(vec))
            }
        };
        let _owned_guard = owned;
        if data_len == 0 {
            return MoltObject::from_int(0).bits();
        }
        loop {
            let rc = unsafe {
                crate::molt_socket_sendto_host(
                    handle,
                    data_ptr as u32,
                    data_len as u32,
                    flags,
                    addr.as_ptr() as u32,
                    addr.len() as u32,
                )
            };
            if rc >= 0 {
                return MoltObject::from_int(rc as i64).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "sendto");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_WRITE) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(_py, wait_err, "sendto");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "sendto");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_recvfrom(_sock_bits: u64, _size_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let size = to_i64(obj_from_bits(_size_bits)).unwrap_or(0).max(0) as usize;
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        #[cfg(unix)]
        let dontwait = (flags & libc::MSG_DONTWAIT) != 0;
        #[cfg(not(unix))]
        let dontwait = false;
        if size == 0 {
            let bytes_ptr = alloc_bytes(_py, &[]);
            if bytes_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let tuple_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_ptr(bytes_ptr).bits(),
                    MoltObject::none().bits(),
                ],
            );
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let nonblocking = matches!(socket_timeout(handle), Some(val) if val == Duration::ZERO);
        let mut buf = vec![0u8; size];
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        loop {
            let rc = unsafe {
                crate::molt_socket_recvfrom_host(
                    handle,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    flags,
                    addr_buf.as_mut_ptr() as u32,
                    addr_buf.len() as u32,
                    (&mut addr_len) as *mut u32 as u32,
                )
            };
            if rc >= 0 {
                let bytes_ptr = alloc_bytes(_py, &buf[..rc as usize]);
                if bytes_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let addr_bits = match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
                    Ok(bits) => bits,
                    Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
                };
                let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
                let tuple_ptr = alloc_tuple(_py, &[bytes_bits, addr_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
            let errno = errno_from_rc(rc);
            if would_block_errno(errno) {
                if dontwait || nonblocking {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "recvfrom");
                }
                if let Err(wait_err) = socket_wait_ready(_py, handle, IO_EVENT_READ) {
                    if wait_err.kind() == ErrorKind::TimedOut {
                        return raise_exception::<u64>(_py, "TimeoutError", "timed out");
                    }
                    return raise_os_error::<u64>(_py, wait_err, "recvfrom");
                }
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "recvfrom");
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_shutdown(_sock_bits: u64, _how_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let how = to_i64(obj_from_bits(_how_bits)).unwrap_or(0) as i32;
        let rc = unsafe { crate::molt_socket_shutdown_host(handle, how) };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "shutdown");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getsockname(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        let rc = unsafe {
            crate::molt_socket_getsockname_host(
                handle,
                addr_buf.as_mut_ptr() as u32,
                addr_buf.len() as u32,
                (&mut addr_len) as *mut u32 as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getsockname");
        }
        match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
            Ok(bits) => bits,
            Err(msg) => raise_exception::<_>(_py, "TypeError", &msg),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getpeername(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let mut addr_buf = vec![0u8; 128];
        let mut addr_len: u32 = 0;
        let rc = unsafe {
            crate::molt_socket_getpeername_host(
                handle,
                addr_buf.as_mut_ptr() as u32,
                addr_buf.len() as u32,
                (&mut addr_len) as *mut u32 as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getpeername");
        }
        match decode_sockaddr(_py, &addr_buf[..addr_len as usize]) {
            Ok(bits) => bits,
            Err(msg) => raise_exception::<_>(_py, "TypeError", &msg),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_setsockopt(
    _sock_bits: u64,
    _level_bits: u64,
    _opt_bits: u64,
    _value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let level = to_i64(obj_from_bits(_level_bits)).unwrap_or(0) as i32;
        let optname = to_i64(obj_from_bits(_opt_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(_value_bits);
        let (val_buf, val_len) = if let Some(val) = to_i64(obj) {
            let bytes = (val as i32).to_ne_bytes();
            (bytes.to_vec(), bytes.len())
        } else if let Some(ptr) = obj.as_ptr() {
            let bytes = unsafe { bytes_like_slice_raw(ptr) };
            let Some(bytes) = bytes else {
                return raise_exception::<_>(_py, "TypeError", "invalid optval");
            };
            (bytes.to_vec(), bytes.len())
        } else {
            return raise_exception::<_>(_py, "TypeError", "invalid optval");
        };
        let rc = unsafe {
            crate::molt_socket_setsockopt_host(
                handle,
                level,
                optname,
                val_buf.as_ptr() as u32,
                val_len as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "setsockopt");
        }
        MoltObject::none().bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getsockopt(
    _sock_bits: u64,
    _level_bits: u64,
    _opt_bits: u64,
    _buflen_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::none().bits(),
        };
        let level = to_i64(obj_from_bits(_level_bits)).unwrap_or(0) as i32;
        let optname = to_i64(obj_from_bits(_opt_bits)).unwrap_or(0) as i32;
        let buflen = if obj_from_bits(_buflen_bits).is_none() {
            None
        } else {
            Some(to_i64(obj_from_bits(_buflen_bits)).unwrap_or(0).max(0) as usize)
        };
        let mut out_len: u32 = 0;
        if let Some(buflen) = buflen {
            let mut buf = vec![0u8; buflen];
            let rc = unsafe {
                crate::molt_socket_getsockopt_host(
                    handle,
                    level,
                    optname,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    (&mut out_len) as *mut u32 as u32,
                )
            };
            if rc < 0 {
                return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getsockopt");
            }
            let ptr = alloc_bytes(_py, &buf[..out_len as usize]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let mut buf = vec![0u8; std::mem::size_of::<i32>()];
        let rc = unsafe {
            crate::molt_socket_getsockopt_host(
                handle,
                level,
                optname,
                buf.as_mut_ptr() as u32,
                buf.len() as u32,
                (&mut out_len) as *mut u32 as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getsockopt");
        }
        if out_len as usize >= std::mem::size_of::<i32>() {
            let val = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]) as i64;
            return MoltObject::from_int(val).bits();
        }
        let ptr = alloc_bytes(_py, &buf[..out_len as usize]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_detach(_sock_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = match socket_handle_from_bits(_py, _sock_bits) {
            Ok(val) => val,
            Err(_) => return MoltObject::from_int(-1).bits(),
        };
        let rc = unsafe { crate::molt_socket_detach_host(handle) };
        wasm_socket_meta_remove(handle);
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, (-rc) as i64, "detach");
        }
        MoltObject::from_int(rc).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socketpair(family_bits: u64, type_bits: u64, proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.listen", "net.bind"])
            .is_err()
        {
            return MoltObject::none().bits();
        }
        let family = if obj_from_bits(family_bits).is_none() {
            #[cfg(unix)]
            {
                libc::AF_UNIX
            }
            #[cfg(not(unix))]
            {
                libc::AF_INET
            }
        } else {
            match to_i64(obj_from_bits(family_bits)) {
                Some(val) => val as i32,
                None => raise_exception::<_>(_py, "TypeError", "family must be int or None"),
            }
        };
        let sock_type = if obj_from_bits(type_bits).is_none() {
            libc::SOCK_STREAM
        } else {
            match to_i64(obj_from_bits(type_bits)) {
                Some(val) => val as i32,
                None => raise_exception::<_>(_py, "TypeError", "type must be int or None"),
            }
        };
        let proto = if obj_from_bits(proto_bits).is_none() {
            0
        } else {
            match to_i64(obj_from_bits(proto_bits)) {
                Some(val) => val as i32,
                None => raise_exception::<_>(_py, "TypeError", "proto must be int or None"),
            }
        };
        #[cfg(unix)]
        {
            if family != libc::AF_UNIX {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::EAFNOSUPPORT as i64,
                    "socketpair family",
                );
            }
            let mut fds = [0 as libc::c_int; 2];
            let ret = libc::socketpair(family, sock_type, proto, fds.as_mut_ptr());
            if ret != 0 {
                return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "socketpair");
            }
            let left_bits = molt_socket_new(
                MoltObject::from_int(family as i64).bits(),
                MoltObject::from_int(sock_type as i64).bits(),
                MoltObject::from_int(proto as i64).bits(),
                MoltObject::from_int(fds[0] as i64).bits(),
            );
            let right_bits = molt_socket_new(
                MoltObject::from_int(family as i64).bits(),
                MoltObject::from_int(sock_type as i64).bits(),
                MoltObject::from_int(proto as i64).bits(),
                MoltObject::from_int(fds[1] as i64).bits(),
            );
            let tuple_ptr = alloc_tuple(_py, &[left_bits, right_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
        #[cfg(windows)]
        {
            // TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned): implement a native Windows socketpair using WSAPROTOCOL_INFO or AF_UNIX to avoid loopback TCP overhead.
            if family != libc::AF_INET && family != libc::AF_INET6 {
                return raise_os_error_errno::<u64>(
                    _py,
                    libc::EAFNOSUPPORT as i64,
                    "socketpair family",
                );
            }
            let loopback = if family == libc::AF_INET6 {
                "[::1]:0"
            } else {
                "127.0.0.1:0"
            };
            let listener = match std::net::TcpListener::bind(loopback) {
                Ok(l) => l,
                Err(err) => return raise_os_error::<u64>(_py, err, "socketpair"),
            };
            let addr = match listener.local_addr() {
                Ok(addr) => addr,
                Err(err) => return raise_os_error::<u64>(_py, err, "socketpair"),
            };
            let client = match std::net::TcpStream::connect(addr) {
                Ok(stream) => stream,
                Err(err) => return raise_os_error::<u64>(_py, err, "socketpair"),
            };
            let (server, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(err) => return raise_os_error::<u64>(_py, err, "socketpair"),
            };
            let left_fd = client.into_raw_socket();
            let right_fd = server.into_raw_socket();
            let left_bits = molt_socket_new(
                MoltObject::from_int(family as i64).bits(),
                MoltObject::from_int(sock_type as i64).bits(),
                MoltObject::from_int(proto as i64).bits(),
                MoltObject::from_int(left_fd as i64).bits(),
            );
            let right_bits = molt_socket_new(
                MoltObject::from_int(family as i64).bits(),
                MoltObject::from_int(sock_type as i64).bits(),
                MoltObject::from_int(proto as i64).bits(),
                MoltObject::from_int(right_fd as i64).bits(),
            );
            let tuple_ptr = alloc_tuple(_py, &[left_bits, right_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socketpair(_family_bits: u64, _type_bits: u64, _proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.listen", "net.bind"])
            .is_err()
        {
            return MoltObject::none().bits();
        }
        let family = if obj_from_bits(_family_bits).is_none() {
            #[cfg(unix)]
            {
                libc::AF_UNIX
            }
            #[cfg(not(unix))]
            {
                libc::AF_INET
            }
        } else {
            match to_i64(obj_from_bits(_family_bits)) {
                Some(val) => val as i32,
                None => return raise_exception::<_>(_py, "TypeError", "family must be int"),
            }
        };
        let sock_type = match to_i64(obj_from_bits(_type_bits)) {
            Some(val) => val as i32,
            None => return raise_exception::<_>(_py, "TypeError", "type must be int"),
        };
        let proto = to_i64(obj_from_bits(_proto_bits)).unwrap_or(0) as i32;
        let mut left: u64 = 0;
        let mut right: u64 = 0;
        let rc = unsafe {
            crate::molt_socket_socketpair_host(
                family,
                sock_type,
                proto,
                (&mut left) as *mut u64 as u32,
                (&mut right) as *mut u64 as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "socketpair");
        }
        wasm_socket_meta_insert(
            left as i64,
            WasmSocketMeta {
                family,
                sock_type,
                proto,
                timeout: None,
                connect_pending: false,
            },
        );
        wasm_socket_meta_insert(
            right as i64,
            WasmSocketMeta {
                family,
                sock_type,
                proto,
                timeout: None,
                connect_pending: false,
            },
        );
        let left_bits = MoltObject::from_int(left as i64).bits();
        let right_bits = MoltObject::from_int(right as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[left_bits, right_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getaddrinfo(
    host_bits: u64,
    port_bits: u64,
    family_bits: u64,
    type_bits: u64,
    proto_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.bind", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let host = match host_from_bits(_py, host_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let service = match service_from_bits(_py, port_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
        let sock_type = to_i64(obj_from_bits(type_bits)).unwrap_or(0) as i32;
        let proto = to_i64(obj_from_bits(proto_bits)).unwrap_or(0) as i32;
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;

        let host_cstr = host
            .as_ref()
            .and_then(|val| CString::new(val.as_str()).ok());
        let service_cstr = service
            .as_ref()
            .and_then(|val| CString::new(val.as_str()).ok());
        if host.is_some() && host_cstr.is_none() {
            return raise_exception::<u64>(_py, "TypeError", "host contains NUL byte");
        }
        if service.is_some() && service_cstr.is_none() {
            return raise_exception::<u64>(_py, "TypeError", "service contains NUL byte");
        }
        let mut hints: libc::addrinfo = std::mem::zeroed();
        hints.ai_family = family;
        hints.ai_socktype = sock_type;
        hints.ai_protocol = proto;
        hints.ai_flags = flags;

        let mut res: *mut libc::addrinfo = std::ptr::null_mut();
        let err = libc::getaddrinfo(
            host_cstr
                .as_ref()
                .map(|s| s.as_ptr())
                .unwrap_or(std::ptr::null()),
            service_cstr
                .as_ref()
                .map(|s| s.as_ptr())
                .unwrap_or(std::ptr::null()),
            &hints as *const libc::addrinfo,
            &mut res as *mut *mut libc::addrinfo,
        );
        if err != 0 {
            let msg = CStr::from_ptr(libc::gai_strerror(err))
                .to_string_lossy()
                .to_string();
            let msg = format!("[Errno {err}] {msg}");
            return raise_os_error_errno::<u64>(_py, err as i64, &msg);
        }

        let builder_bits = molt_list_builder_new(MoltObject::from_int(0).bits());
        if builder_bits == 0 {
            libc::freeaddrinfo(res);
            return MoltObject::none().bits();
        }
        let mut cur = res;
        while !cur.is_null() {
            let ai = &*cur;
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let len = ai.ai_addrlen;
            unsafe {
                std::ptr::copy_nonoverlapping(
                    ai.ai_addr as *const u8,
                    &mut storage as *mut _ as *mut u8,
                    len as usize,
                );
            }
            let sockaddr = unsafe { SockAddr::new(storage, len) };
            let sockaddr_bits = sockaddr_to_bits(_py, &sockaddr);
            let canon_bits = if !ai.ai_canonname.is_null() {
                let name = CStr::from_ptr(ai.ai_canonname).to_string_lossy();
                let ptr = alloc_string(_py, name.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            } else {
                MoltObject::none().bits()
            };
            let family_bits = MoltObject::from_int(ai.ai_family as i64).bits();
            let sock_type_bits = MoltObject::from_int(ai.ai_socktype as i64).bits();
            let proto_bits = MoltObject::from_int(ai.ai_protocol as i64).bits();
            let tuple_ptr = alloc_tuple(
                _py,
                &[
                    family_bits,
                    sock_type_bits,
                    proto_bits,
                    canon_bits,
                    sockaddr_bits,
                ],
            );
            if tuple_ptr.is_null() {
                dec_ref_bits(_py, canon_bits);
                dec_ref_bits(_py, sockaddr_bits);
                break;
            }
            let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
            unsafe {
                molt_list_builder_append(builder_bits, tuple_bits);
            }
            dec_ref_bits(_py, canon_bits);
            dec_ref_bits(_py, sockaddr_bits);
            cur = ai.ai_next;
        }
        libc::freeaddrinfo(res);
        unsafe { molt_list_builder_finish_owned(builder_bits) }
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getaddrinfo(
    _host_bits: u64,
    _port_bits: u64,
    _family_bits: u64,
    _type_bits: u64,
    _proto_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net.bind", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let host = match host_from_bits(_py, _host_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let service = match service_from_bits(_py, _port_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let family = to_i64(obj_from_bits(_family_bits)).unwrap_or(0) as i32;
        let sock_type = to_i64(obj_from_bits(_type_bits)).unwrap_or(0) as i32;
        let proto = to_i64(obj_from_bits(_proto_bits)).unwrap_or(0) as i32;
        let flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        let host_bytes = host.as_ref().map(|val| val.as_bytes()).unwrap_or(&[]);
        let service_bytes = service.as_ref().map(|val| val.as_bytes()).unwrap_or(&[]);
        if host_bytes.iter().any(|b| *b == 0) {
            return raise_exception::<_>(_py, "TypeError", "host contains NUL byte");
        }
        if service_bytes.iter().any(|b| *b == 0) {
            return raise_exception::<_>(_py, "TypeError", "service contains NUL byte");
        }
        let mut cap = 4096usize;
        let mut buf = vec![0u8; cap];
        let mut out_len: u32 = 0;
        let mut fallback_numeric = false;
        loop {
            let rc = unsafe {
                crate::molt_socket_getaddrinfo_host(
                    host_bytes.as_ptr() as u32,
                    host_bytes.len() as u32,
                    service_bytes.as_ptr() as u32,
                    service_bytes.len() as u32,
                    family,
                    sock_type,
                    proto,
                    flags,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    (&mut out_len) as *mut u32 as u32,
                )
            };
            if rc == 0 {
                break;
            }
            let errno = errno_from_rc(rc);
            if errno == libc::ENOSYS {
                fallback_numeric = true;
                break;
            }
            if errno == libc::ENOMEM && out_len as usize > cap {
                cap = out_len as usize;
                buf.resize(cap, 0);
                continue;
            }
            return raise_os_error_errno::<u64>(_py, errno as i64, "getaddrinfo");
        }
        if fallback_numeric {
            let ip = if let Some(host) = host.as_ref() {
                match host.parse::<IpAddr>() {
                    Ok(ip) => ip,
                    Err(_) => {
                        return raise_os_error_errno::<u64>(
                            _py,
                            libc::EAI_NONAME as i64,
                            "name or service not known",
                        )
                    }
                }
            } else if family == libc::AF_INET6 {
                IpAddr::V6(Ipv6Addr::UNSPECIFIED)
            } else {
                IpAddr::V4(Ipv4Addr::UNSPECIFIED)
            };
            let port = match service.as_ref() {
                Some(val) => match val.parse::<u16>() {
                    Ok(port) => port,
                    Err(_) => {
                        return raise_exception::<_>(_py, "TypeError", "service must be int");
                    }
                },
                None => 0,
            };
            let sockaddr = match ip {
                IpAddr::V4(ip) => {
                    let addr = Ipv4Addr::from(ip.octets());
                    let host = addr.to_string();
                    let host_ptr = alloc_string(_py, host.as_bytes());
                    if host_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let host_bits = MoltObject::from_ptr(host_ptr).bits();
                    let port_bits = MoltObject::from_int(port as i64).bits();
                    let tuple_ptr = alloc_tuple(_py, &[host_bits, port_bits]);
                    dec_ref_bits(_py, host_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
                IpAddr::V6(ip) => {
                    let addr = Ipv6Addr::from(ip.octets());
                    let host = addr.to_string();
                    let host_ptr = alloc_string(_py, host.as_bytes());
                    if host_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let host_bits = MoltObject::from_ptr(host_ptr).bits();
                    let port_bits = MoltObject::from_int(port as i64).bits();
                    let flow_bits = MoltObject::from_int(0).bits();
                    let scope_bits = MoltObject::from_int(0).bits();
                    let tuple_ptr =
                        alloc_tuple(_py, &[host_bits, port_bits, flow_bits, scope_bits]);
                    dec_ref_bits(_py, host_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(tuple_ptr).bits()
                }
            };
            let family_bits = MoltObject::from_int(family as i64).bits();
            let sock_type_bits = MoltObject::from_int(sock_type as i64).bits();
            let proto_bits = MoltObject::from_int(proto as i64).bits();
            let tuple_ptr = alloc_tuple(
                _py,
                &[
                    family_bits,
                    sock_type_bits,
                    proto_bits,
                    MoltObject::none().bits(),
                    sockaddr,
                ],
            );
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let list_ptr = alloc_list(_py, &[MoltObject::from_ptr(tuple_ptr).bits()]);
            if list_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(tuple_ptr).bits());
                return MoltObject::none().bits();
            }
            dec_ref_bits(_py, MoltObject::from_ptr(tuple_ptr).bits());
            return MoltObject::from_ptr(list_ptr).bits();
        }
        let data = &buf[..out_len as usize];
        if data.len() < 4 {
            return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
        }
        let mut offset = 0usize;
        let count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        offset += 4;
        let mut out: Vec<u64> = Vec::with_capacity(count);
        for _ in 0..count {
            if offset + 12 > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let family = i32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let sock_type = i32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let proto = i32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            offset += 12;
            if offset + 4 > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let canon_len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;
            if offset + canon_len > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let canon_bits = if canon_len > 0 {
                let ptr = alloc_string(_py, &data[offset..offset + canon_len]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            } else {
                MoltObject::none().bits()
            };
            offset += canon_len;
            if offset + 4 > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let addr_len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;
            if offset + addr_len > data.len() {
                return raise_exception::<_>(_py, "RuntimeError", "invalid addrinfo payload");
            }
            let sockaddr_bits = match decode_sockaddr(_py, &data[offset..offset + addr_len]) {
                Ok(bits) => bits,
                Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
            };
            offset += addr_len;
            let family_bits = MoltObject::from_int(family as i64).bits();
            let sock_type_bits = MoltObject::from_int(sock_type as i64).bits();
            let proto_bits = MoltObject::from_int(proto as i64).bits();
            let tuple_ptr = alloc_tuple(
                _py,
                &[
                    family_bits,
                    sock_type_bits,
                    proto_bits,
                    canon_bits,
                    sockaddr_bits,
                ],
            );
            dec_ref_bits(_py, canon_bits);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            out.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list(_py, &out);
        if list_ptr.is_null() {
            for bits in out {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        for bits in out {
            dec_ref_bits(_py, bits);
        }
        list_bits
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getnameinfo(addr_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(addr_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "sockaddr must be tuple");
        };
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return raise_exception::<_>(_py, "TypeError", "sockaddr must be tuple");
        }
        let elems = seq_vec_ref(ptr);
        let family = if elems.len() >= 4 {
            libc::AF_INET6
        } else {
            libc::AF_INET
        };
        let sockaddr = match sockaddr_from_bits(_py, addr_bits, family) {
            Ok(addr) => addr,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mut host_buf = vec![0u8; libc::NI_MAXHOST as usize + 1];
        let mut serv_buf = vec![0u8; libc::NI_MAXSERV as usize + 1];
        let ret = libc::getnameinfo(
            sockaddr.as_ptr(),
            sockaddr.len(),
            host_buf.as_mut_ptr() as *mut libc::c_char,
            host_buf.len() as libc::socklen_t,
            serv_buf.as_mut_ptr() as *mut libc::c_char,
            serv_buf.len() as libc::socklen_t,
            flags,
        );
        if ret != 0 {
            let msg = CStr::from_ptr(libc::gai_strerror(ret))
                .to_string_lossy()
                .to_string();
            let msg = format!("[Errno {ret}] {msg}");
            return raise_os_error_errno::<u64>(_py, ret as i64, &msg);
        }
        let host = CStr::from_ptr(host_buf.as_ptr() as *const libc::c_char).to_string_lossy();
        let serv = CStr::from_ptr(serv_buf.as_ptr() as *const libc::c_char).to_string_lossy();
        let host_ptr = alloc_string(_py, host.as_bytes());
        if host_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let serv_ptr = alloc_string(_py, serv.as_bytes());
        if serv_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(host_ptr).bits());
            return MoltObject::none().bits();
        }
        let host_bits = MoltObject::from_ptr(host_ptr).bits();
        let serv_bits = MoltObject::from_ptr(serv_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[host_bits, serv_bits]);
        dec_ref_bits(_py, host_bits);
        dec_ref_bits(_py, serv_bits);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getnameinfo(_addr_bits: u64, _flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _flags = to_i64(obj_from_bits(_flags_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(_addr_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "sockaddr must be tuple");
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
            return raise_exception::<_>(_py, "TypeError", "sockaddr must be tuple");
        }
        let elems = unsafe { seq_vec_ref(ptr) };
        if elems.len() < 2 {
            return raise_exception::<_>(_py, "TypeError", "sockaddr must be (host, port)");
        }
        let host = match host_from_bits(_py, elems[0]) {
            Ok(Some(val)) => val,
            Ok(None) => return raise_exception::<_>(_py, "TypeError", "host cannot be None"),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let port = match port_from_bits(_py, elems[1]) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let host_ptr = alloc_string(_py, host.as_bytes());
        if host_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let serv_ptr = alloc_string(_py, port.to_string().as_bytes());
        if serv_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(host_ptr).bits());
            return MoltObject::none().bits();
        }
        let host_bits = MoltObject::from_ptr(host_ptr).bits();
        let serv_bits = MoltObject::from_ptr(serv_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[host_bits, serv_bits]);
        dec_ref_bits(_py, host_bits);
        dec_ref_bits(_py, serv_bits);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_gethostname() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut buf = vec![0u8; 256];
        let ret = libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len());
        if ret != 0 {
            return raise_os_error::<u64>(_py, std::io::Error::last_os_error(), "gethostname");
        }
        if let Some(pos) = buf.iter().position(|b| *b == 0) {
            buf.truncate(pos);
        }
        let ptr = alloc_string(_py, &buf);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_gethostname() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut buf = vec![0u8; 256];
        let mut out_len: u32 = 0;
        let mut rc = unsafe {
            crate::molt_socket_gethostname_host(
                buf.as_mut_ptr() as u32,
                buf.len() as u32,
                (&mut out_len) as *mut u32 as u32,
            )
        };
        if rc == -(libc::ENOMEM as i32) && out_len as usize > buf.len() {
            buf.resize(out_len as usize, 0);
            rc = unsafe {
                crate::molt_socket_gethostname_host(
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    (&mut out_len) as *mut u32 as u32,
                )
            };
        }
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "gethostname");
        }
        let len = out_len as usize;
        let ptr = alloc_string(_py, &buf[..len]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getservbyname(name_bits: u64, proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match host_from_bits(_py, name_bits) {
            Ok(Some(val)) => val,
            Ok(None) => {
                return raise_exception::<_>(_py, "TypeError", "service name cannot be None")
            }
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let proto = match host_from_bits(_py, proto_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let name_cstr = CString::new(name).map_err(|_| ()).ok();
        if name_cstr.is_none() {
            return raise_exception::<_>(_py, "TypeError", "service name contains NUL byte");
        }
        let proto_cstr = proto
            .as_ref()
            .and_then(|val| CString::new(val.as_str()).ok());
        if proto.is_some() && proto_cstr.is_none() {
            return raise_exception::<_>(_py, "TypeError", "proto contains NUL byte");
        }
        let serv = libc::getservbyname(
            name_cstr.as_ref().unwrap().as_ptr(),
            proto_cstr
                .as_ref()
                .map(|s| s.as_ptr())
                .unwrap_or(std::ptr::null()),
        );
        if serv.is_null() {
            return raise_os_error_errno::<u64>(_py, libc::ENOENT as i64, "service not found");
        }
        let port = libc::ntohs((*serv).s_port as u16) as i64;
        MoltObject::from_int(port).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getservbyname(_name_bits: u64, _proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match host_from_bits(_py, _name_bits) {
            Ok(Some(val)) => val,
            Ok(None) => {
                return raise_exception::<_>(_py, "TypeError", "service name cannot be None")
            }
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let proto = match host_from_bits(_py, _proto_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let name_bytes = name.as_bytes();
        let proto_bytes = proto.as_ref().map(|val| val.as_bytes()).unwrap_or(&[]);
        if name_bytes.iter().any(|b| *b == 0) {
            return raise_exception::<_>(_py, "TypeError", "service name contains NUL byte");
        }
        if proto_bytes.iter().any(|b| *b == 0) {
            return raise_exception::<_>(_py, "TypeError", "proto contains NUL byte");
        }
        let rc = unsafe {
            crate::molt_socket_getservbyname_host(
                name_bytes.as_ptr() as u32,
                name_bytes.len() as u32,
                proto_bytes.as_ptr() as u32,
                proto_bytes.len() as u32,
            )
        };
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getservbyname");
        }
        MoltObject::from_int(rc as i64).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_getservbyport(port_bits: u64, proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let port = match to_i64(obj_from_bits(port_bits)) {
            Some(val) if val >= 0 && val <= u16::MAX as i64 => val as u16,
            _ => return raise_exception::<_>(_py, "TypeError", "port must be int"),
        };
        let proto = match host_from_bits(_py, proto_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let proto_cstr = proto
            .as_ref()
            .and_then(|val| CString::new(val.as_str()).ok());
        if proto.is_some() && proto_cstr.is_none() {
            return raise_exception::<_>(_py, "TypeError", "proto contains NUL byte");
        }
        let serv = libc::getservbyport(
            libc::htons(port) as i32,
            proto_cstr
                .as_ref()
                .map(|s| s.as_ptr())
                .unwrap_or(std::ptr::null()),
        );
        if serv.is_null() {
            return raise_os_error_errno::<u64>(_py, libc::ENOENT as i64, "service not found");
        }
        let name = CStr::from_ptr((*serv).s_name).to_string_lossy();
        let ptr = alloc_string(_py, name.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_getservbyport(_port_bits: u64, _proto_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let port = match to_i64(obj_from_bits(_port_bits)) {
            Some(val) if val >= 0 && val <= u16::MAX as i64 => val as u16,
            _ => return raise_exception::<_>(_py, "TypeError", "port must be int"),
        };
        let proto = match host_from_bits(_py, _proto_bits) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let proto_bytes = proto.as_ref().map(|val| val.as_bytes()).unwrap_or(&[]);
        if proto_bytes.iter().any(|b| *b == 0) {
            return raise_exception::<_>(_py, "TypeError", "proto contains NUL byte");
        }
        let mut buf = vec![0u8; 256];
        let mut out_len: u32 = 0;
        let mut rc = unsafe {
            crate::molt_socket_getservbyport_host(
                port as i32,
                proto_bytes.as_ptr() as u32,
                proto_bytes.len() as u32,
                buf.as_mut_ptr() as u32,
                buf.len() as u32,
                (&mut out_len) as *mut u32 as u32,
            )
        };
        if rc == -(libc::ENOMEM as i32) && out_len as usize > buf.len() {
            buf.resize(out_len as usize, 0);
            rc = unsafe {
                crate::molt_socket_getservbyport_host(
                    port as i32,
                    proto_bytes.as_ptr() as u32,
                    proto_bytes.len() as u32,
                    buf.as_mut_ptr() as u32,
                    buf.len() as u32,
                    (&mut out_len) as *mut u32 as u32,
                )
            };
        }
        if rc < 0 {
            return raise_os_error_errno::<u64>(_py, errno_from_rc(rc) as i64, "getservbyport");
        }
        let ptr = alloc_string(_py, &buf[..out_len as usize]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_inet_pton(family_bits: u64, address_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
        let addr = match host_from_bits(_py, address_bits) {
            Ok(Some(val)) => val,
            Ok(None) => return raise_exception::<_>(_py, "TypeError", "address cannot be None"),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if family == libc::AF_INET {
            let ip: Ipv4Addr = match addr.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EINVAL as i64,
                        "invalid IPv4 address",
                    )
                }
            };
            let ptr = alloc_bytes(_py, &ip.octets());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        if family == libc::AF_INET6 {
            let ip: Ipv6Addr = match addr.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EINVAL as i64,
                        "invalid IPv6 address",
                    )
                }
            };
            let ptr = alloc_bytes(_py, &ip.octets());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        raise_exception::<_>(_py, "ValueError", "unsupported address family")
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_inet_pton(_family_bits: u64, _address_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let family = to_i64(obj_from_bits(_family_bits)).unwrap_or(0) as i32;
        let addr = match host_from_bits(_py, _address_bits) {
            Ok(Some(val)) => val,
            Ok(None) => return raise_exception::<_>(_py, "TypeError", "address cannot be None"),
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if family == libc::AF_INET {
            let ip: Ipv4Addr = match addr.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EINVAL as i64,
                        "invalid IPv4 address",
                    )
                }
            };
            let ptr = alloc_bytes(_py, &ip.octets());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        if family == libc::AF_INET6 {
            let ip: Ipv6Addr = match addr.parse() {
                Ok(ip) => ip,
                Err(_) => {
                    return raise_os_error_errno::<u64>(
                        _py,
                        libc::EINVAL as i64,
                        "invalid IPv6 address",
                    )
                }
            };
            let ptr = alloc_bytes(_py, &ip.octets());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        raise_exception::<_>(_py, "ValueError", "unsupported address family")
    })
}

#[cfg(not(target_arch = "wasm32"))]
/// # Safety
/// Caller must pass valid socket handles and runtime-encoded arguments.
#[no_mangle]
pub unsafe extern "C" fn molt_socket_inet_ntop(family_bits: u64, packed_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let family = to_i64(obj_from_bits(family_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(packed_bits);
        let data = if let Some(ptr) = obj.as_ptr() {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let slice = std::slice::from_raw_parts(bytes_data(ptr), len);
                slice.to_vec()
            } else if type_id == TYPE_ID_MEMORYVIEW {
                if let Some(slice) = memoryview_bytes_slice(ptr) {
                    slice.to_vec()
                } else if let Some(vec) = memoryview_collect_bytes(ptr) {
                    vec
                } else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "packed address must be bytes-like",
                    );
                }
            } else {
                return raise_exception::<_>(_py, "TypeError", "packed address must be bytes-like");
            }
        } else {
            return raise_exception::<_>(_py, "TypeError", "packed address must be bytes-like");
        };
        if family == libc::AF_INET {
            if data.len() != 4 {
                return raise_exception::<_>(_py, "ValueError", "invalid IPv4 packed length");
            }
            let addr = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
            let text = addr.to_string();
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        if family == libc::AF_INET6 {
            if data.len() != 16 {
                return raise_exception::<_>(_py, "ValueError", "invalid IPv6 packed length");
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&data[..16]);
            let addr = Ipv6Addr::from(octets);
            let text = addr.to_string();
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        raise_exception::<_>(_py, "ValueError", "unsupported address family")
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_inet_ntop(_family_bits: u64, _packed_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let family = to_i64(obj_from_bits(_family_bits)).unwrap_or(0) as i32;
        let obj = obj_from_bits(_packed_bits);
        let data = if let Some(ptr) = obj.as_ptr() {
            let bytes = unsafe { bytes_like_slice_raw(ptr) };
            let Some(bytes) = bytes else {
                return raise_exception::<_>(_py, "TypeError", "packed must be bytes-like");
            };
            bytes.to_vec()
        } else {
            return raise_exception::<_>(_py, "TypeError", "packed must be bytes-like");
        };
        if family == libc::AF_INET {
            if data.len() != 4 {
                return raise_exception::<_>(_py, "ValueError", "invalid IPv4 packed length");
            }
            let mut octets = [0u8; 4];
            octets.copy_from_slice(&data[..4]);
            let addr = Ipv4Addr::from(octets);
            let text = addr.to_string();
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        if family == libc::AF_INET6 {
            if data.len() != 16 {
                return raise_exception::<_>(_py, "ValueError", "invalid IPv6 packed length");
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&data[..16]);
            let addr = Ipv6Addr::from(octets);
            let text = addr.to_string();
            let ptr = alloc_string(_py, text.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        raise_exception::<_>(_py, "ValueError", "unsupported address family")
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_socket_has_ipv6() -> u64 {
    crate::with_gil_entry!(_py, {
        let supported = std::net::TcpListener::bind("[::1]:0").is_ok();
        MoltObject::from_bool(supported).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_socket_has_ipv6() -> u64 {
    crate::with_gil_entry!(_py, {
        let supported = unsafe { crate::molt_socket_has_ipv6_host() };
        MoltObject::from_bool(supported != 0).bits()
    })
}
