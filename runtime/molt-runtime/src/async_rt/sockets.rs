use super::channels::has_capability;
use crate::PyToken;
use crate::*;

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};
#[cfg(molt_has_net_io)]
use socket2::{Domain, Protocol, SockAddr, SockAddrStorage, SockRef, Socket, Type};
use std::collections::HashMap;
#[cfg(all(molt_has_net_io, not(unix)))]
use std::collections::VecDeque;
use std::ffi::{CStr, CString, OsString};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
#[cfg(molt_has_net_io)]
use std::net::{SocketAddr, ToSocketAddrs};
#[cfg(unix)]
use std::os::fd::BorrowedFd;
use std::os::raw::{c_int, c_void};
#[cfg(unix)]
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, BorrowedSocket, FromRawSocket, IntoRawSocket, RawSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// --- Sockets ---

#[cfg(molt_has_net_io)]
pub(super) enum MoltSocketKind {
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

#[cfg(molt_has_net_io)]
pub(super) struct MoltSocketInner {
    pub(super) kind: MoltSocketKind,
    pub(super) family: i32,
    #[allow(dead_code)]
    pub(super) sock_type: i32,
    #[allow(dead_code)]
    pub(super) proto: i32,
    pub(super) connect_pending: bool,
}

#[cfg(molt_has_net_io)]
pub(super) struct MoltSocket {
    pub(super) inner: Mutex<MoltSocketInner>,
    pub(super) timeout: Mutex<Option<Duration>>,
    pub(super) closed: AtomicBool,
    pub(super) refs: AtomicUsize,
}

pub(super) struct MoltSocketReader {
    pub(super) socket_bits: u64,
    pub(super) buffer: Vec<u8>,
    pub(super) buffer_start: usize,
    pub(super) scan_cursor: usize,
    pub(super) eof: bool,
}

#[cfg(molt_has_net_io)]
#[cfg(all(unix, molt_has_net_io))]
pub(super) type SocketFd = RawFd;
#[cfg(molt_has_net_io)]
#[cfg(all(windows, molt_has_net_io))]
pub(super) type SocketFd = RawSocket;

#[cfg(molt_has_net_io)]
pub(super) fn socket_fd_map() -> &'static Mutex<HashMap<SocketFd, PtrSlot>> {
    static SOCKET_FD_MAP: OnceLock<Mutex<HashMap<SocketFd, PtrSlot>>> = OnceLock::new();
    SOCKET_FD_MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(molt_has_net_io)]
pub(super) fn trace_socket_recv() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_SOCKET_RECV").as_deref() == Ok("1"))
}

#[cfg(molt_has_net_io)]
pub(super) fn trace_socket_send() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_SOCKET_SEND").as_deref() == Ok("1"))
}

#[cfg(molt_has_net_io)]
pub(super) fn socket_debug_fd(socket_ptr: *mut u8) -> Option<i64> {
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

#[cfg(molt_has_net_io)]
pub(super) fn socket_register_fd(socket_ptr: *mut u8) {
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

#[cfg(molt_has_net_io)]
pub(super) fn socket_unregister_fd(socket_ptr: *mut u8) {
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
        #[cfg(not(unix))]
        socket_unregister_peer_state(fd);
    }
}

#[cfg(molt_has_net_io)]
pub(super) fn socket_ptr_from_fd(fd: SocketFd) -> Option<*mut u8> {
    socket_fd_map().lock().unwrap().get(&fd).map(|slot| slot.0)
}

#[cfg(molt_has_net_io)]
pub(crate) fn socket_ptr_from_bits_or_fd(socket_bits: u64) -> *mut u8 {
    if let Some(fd) = to_i64(obj_from_bits(socket_bits)).or({
        if socket_bits <= i64::MAX as u64 {
            Some(socket_bits as i64)
        } else {
            None
        }
    }) {
        if fd < 0 {
            return std::ptr::null_mut();
        }
        #[cfg(unix)]
        {
            return socket_ptr_from_fd(fd as RawFd).unwrap_or(std::ptr::null_mut());
        }
        #[cfg(all(windows, molt_has_net_io))]
        {
            return socket_ptr_from_fd(fd as RawSocket).unwrap_or(std::ptr::null_mut());
        }
    }
    let ptr = ptr_from_bits(socket_bits);
    if !ptr.is_null() {
        return ptr;
    }
    std::ptr::null_mut()
}

#[cfg(molt_has_net_io)]
impl MoltSocketInner {
    pub(crate) fn source_mut(&mut self) -> Option<&mut dyn mio::event::Source> {
        match &mut self.kind {
            MoltSocketKind::Closed => None,
            MoltSocketKind::TcpStream(stream) => Some(stream),
            MoltSocketKind::TcpListener(listener) => Some(listener),
            MoltSocketKind::UdpSocket(sock) => Some(sock),
            #[cfg(all(unix, molt_has_net_io))]
            MoltSocketKind::UnixStream(stream) => Some(stream),
            #[cfg(all(unix, molt_has_net_io))]
            MoltSocketKind::UnixListener(listener) => Some(listener),
            #[cfg(all(unix, molt_has_net_io))]
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
            #[cfg(all(unix, molt_has_net_io))]
            MoltSocketKind::UnixStream(_) | MoltSocketKind::UnixListener(_) => true,
            _ => false,
        }
    }

    #[cfg(all(unix, molt_has_net_io))]
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

    #[cfg(all(windows, molt_has_net_io))]
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

#[cfg(molt_has_net_io)]
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

#[cfg(molt_has_net_io)]
pub(super) fn socket_timeout(socket_ptr: *mut u8) -> Option<Duration> {
    if socket_ptr.is_null() {
        return None;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let guard = socket.timeout.lock().unwrap();
    *guard
}

#[cfg(molt_has_net_io)]
pub(super) fn socket_set_timeout(socket_ptr: *mut u8, timeout: Option<Duration>) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    let mut guard = socket.timeout.lock().unwrap();
    *guard = timeout;
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone)]
pub(super) struct WasmSocketMeta {
    pub(super) family: i32,
    pub(super) sock_type: i32,
    pub(super) proto: i32,
    pub(super) timeout: Option<Duration>,
    pub(super) connect_pending: bool,
}

#[cfg(target_arch = "wasm32")]
pub(super) fn wasm_socket_meta_map() -> &'static Mutex<HashMap<i64, WasmSocketMeta>> {
    static MAP: OnceLock<Mutex<HashMap<i64, WasmSocketMeta>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(target_arch = "wasm32")]
pub(super) fn wasm_socket_meta_insert(handle: i64, meta: WasmSocketMeta) {
    wasm_socket_meta_map().lock().unwrap().insert(handle, meta);
}

#[cfg(target_arch = "wasm32")]
pub(super) fn wasm_socket_meta_remove(handle: i64) {
    wasm_socket_meta_map().lock().unwrap().remove(&handle);
}

#[cfg(target_arch = "wasm32")]
pub(super) fn with_wasm_socket_meta_mut<R, F>(handle: i64, f: F) -> Result<R, String>
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
pub(super) fn wasm_socket_family(handle: i64) -> Result<i32, String> {
    let guard = wasm_socket_meta_map().lock().unwrap();
    guard
        .get(&handle)
        .map(|meta| meta.family)
        .ok_or_else(|| "socket closed".to_string())
}

#[cfg(target_arch = "wasm32")]
pub(super) fn socket_timeout(handle: i64) -> Option<Duration> {
    let guard = wasm_socket_meta_map().lock().unwrap();
    guard.get(&handle).and_then(|meta| meta.timeout)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn socket_set_timeout(handle: i64, timeout: Option<Duration>) -> Result<(), String> {
    with_wasm_socket_meta_mut(handle, |meta| {
        meta.timeout = timeout;
    })
}

#[cfg(target_arch = "wasm32")]
pub(super) fn socket_connect_pending(handle: i64) -> bool {
    let guard = wasm_socket_meta_map().lock().unwrap();
    guard
        .get(&handle)
        .map(|meta| meta.connect_pending)
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn socket_set_connect_pending(handle: i64, pending: bool) -> Result<(), String> {
    with_wasm_socket_meta_mut(handle, |meta| {
        meta.connect_pending = pending;
    })
}

#[cfg(molt_has_net_io)]
#[allow(dead_code)]
pub(super) fn socket_mark_closed(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    socket.closed.store(true, AtomicOrdering::Relaxed);
}

#[cfg(molt_has_net_io)]
pub(crate) fn socket_ref_inc(socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    socket.refs.fetch_add(1, AtomicOrdering::AcqRel);
}

#[cfg(molt_has_net_io)]
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

#[cfg(molt_has_net_io)]
pub(crate) fn io_wait_release_socket(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let _header = unsafe { header_from_obj_ptr(future_ptr) };
    let payload_bytes = unsafe {
        crate::object::object_payload_size(future_ptr)
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
        crate::object::object_payload_size(future_ptr)
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

pub(super) fn iter_values_from_bits(_py: &PyToken<'_>, iterable_bits: u64) -> Result<Vec<u64>, u64> {
    let iter_bits = crate::molt_iter(iterable_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<u64> = Vec::new();
    loop {
        let pair_bits = crate::molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "iterator protocol violation",
                ));
            }
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "iterator protocol violation",
            ));
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        out.push(pair[0]);
    }
    Ok(out)
}

pub(super) fn collect_sendmsg_payload(_py: &PyToken<'_>, buffers_bits: u64) -> Result<Vec<Vec<u8>>, u64> {
    let values = iter_values_from_bits(_py, buffers_bits)?;
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(values.len());
    for value_bits in values {
        let send_data = match send_data_from_bits(value_bits) {
            Ok(val) => val,
            Err(msg) => return Err(raise_exception::<u64>(_py, "TypeError", &msg)),
        };
        match send_data {
            SendData::Borrowed(ptr, len) => {
                let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
                out.push(bytes.to_vec());
            }
            SendData::Owned(vec) => out.push(vec),
        }
    }
    Ok(out)
}

pub(super) type AncillaryItem = (i32, i32, Vec<u8>);

#[cfg(all(molt_has_net_io, not(unix)))]
#[derive(Clone)]
pub(super) struct PendingAncillaryChunk {
    pub(super) remaining: usize,
    pub(super) items: Vec<AncillaryItem>,
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_peer_map() -> &'static Mutex<HashMap<SocketFd, SocketFd>> {
    static MAP: OnceLock<Mutex<HashMap<SocketFd, SocketFd>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_ancillary_queue_map() -> &'static Mutex<HashMap<SocketFd, VecDeque<PendingAncillaryChunk>>>
{
    static MAP: OnceLock<Mutex<HashMap<SocketFd, VecDeque<PendingAncillaryChunk>>>> =
        OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_register_peer_pair(left: SocketFd, right: SocketFd) {
    let mut map = socket_peer_map().lock().unwrap();
    map.insert(left, right);
    map.insert(right, left);
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_unregister_peer_state(fd: SocketFd) {
    let peer = socket_peer_map().lock().unwrap().remove(&fd);
    if let Some(peer_fd) = peer {
        socket_peer_map().lock().unwrap().remove(&peer_fd);
    }
    socket_ancillary_queue_map().lock().unwrap().remove(&fd);
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_peer_available(fd: SocketFd) -> bool {
    socket_peer_map().lock().unwrap().contains_key(&fd)
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_enqueue_stream_ancillary(
    fd: SocketFd,
    data_len: usize,
    items: &[AncillaryItem],
) -> Result<(), std::io::Error> {
    if data_len == 0 || items.is_empty() {
        return Ok(());
    }
    let peer = socket_peer_map()
        .lock()
        .unwrap()
        .get(&fd)
        .copied()
        .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EOPNOTSUPP))?;
    let mut map = socket_ancillary_queue_map().lock().unwrap();
    map.entry(peer)
        .or_default()
        .push_back(PendingAncillaryChunk {
            remaining: data_len,
            items: items.to_vec(),
        });
    Ok(())
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_take_stream_ancillary(fd: SocketFd, data_len: usize, peek: bool) -> Vec<AncillaryItem> {
    if data_len == 0 {
        return Vec::new();
    }
    let mut map = socket_ancillary_queue_map().lock().unwrap();
    let Some(queue) = map.get_mut(&fd) else {
        return Vec::new();
    };
    let mut remaining = data_len;
    let mut out: Vec<AncillaryItem> = Vec::new();
    for chunk in queue.iter_mut() {
        if remaining == 0 {
            break;
        }
        if chunk.remaining == 0 {
            continue;
        }
        let take = remaining.min(chunk.remaining);
        if take == 0 {
            continue;
        }
        if !chunk.items.is_empty() {
            if peek {
                out.extend(chunk.items.iter().cloned());
            } else {
                out.extend(std::mem::take(&mut chunk.items));
            }
        }
        if !peek {
            chunk.remaining -= take;
        }
        remaining -= take;
    }
    if !peek {
        while queue
            .front()
            .map(|chunk| chunk.remaining == 0)
            .unwrap_or(false)
        {
            queue.pop_front();
        }
    }
    out
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_clip_ancillary_for_bufsize(
    items: Vec<AncillaryItem>,
    ancbufsize: i64,
) -> (Vec<AncillaryItem>, bool) {
    if items.is_empty() {
        return (Vec::new(), false);
    }
    if ancbufsize <= 0 {
        return (Vec::new(), true);
    }
    let cap = ancbufsize as usize;
    let mut used = 4usize;
    let mut out: Vec<AncillaryItem> = Vec::new();
    let mut truncated = false;
    for (level, kind, data) in items {
        let entry_size = 12usize.saturating_add(data.len());
        if used.saturating_add(entry_size) > cap {
            truncated = true;
            break;
        }
        used = used.saturating_add(entry_size);
        out.push((level, kind, data));
    }
    (out, truncated)
}

pub(super) fn parse_sendmsg_ancillary_items(
    _py: &PyToken<'_>,
    ancdata_bits: u64,
) -> Result<Vec<AncillaryItem>, u64> {
    if obj_from_bits(ancdata_bits).is_none() {
        return Ok(Vec::new());
    }
    let entries = iter_values_from_bits(_py, ancdata_bits)?;
    let mut out: Vec<AncillaryItem> = Vec::with_capacity(entries.len());
    for entry_bits in entries {
        let Some(entry_ptr) = maybe_ptr_from_bits(entry_bits) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "sendmsg ancillary data must be iterable of 3-item tuples",
            ));
        };
        let entry_type = unsafe { object_type_id(entry_ptr) };
        if entry_type != TYPE_ID_TUPLE && entry_type != TYPE_ID_LIST {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "sendmsg ancillary data must be iterable of 3-item tuples",
            ));
        }
        let parts = unsafe { seq_vec_ref(entry_ptr) };
        if parts.len() != 3 {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "sendmsg ancillary data must be iterable of 3-item tuples",
            ));
        }
        let level = match to_i64(obj_from_bits(parts[0])) {
            Some(val) => val as i32,
            None => {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "sendmsg ancillary level must be int",
                ));
            }
        };
        let kind = match to_i64(obj_from_bits(parts[1])) {
            Some(val) => val as i32,
            None => {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "sendmsg ancillary type must be int",
                ));
            }
        };
        let payload = match send_data_from_bits(parts[2]) {
            Ok(SendData::Borrowed(ptr, len)) => {
                unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
            }
            Ok(SendData::Owned(vec)) => vec,
            Err(msg) => return Err(raise_exception::<u64>(_py, "TypeError", &msg)),
        };
        out.push((level, kind, payload));
    }
    Ok(out)
}

#[cfg(unix)]
pub(super) fn encode_sendmsg_ancillary_buffer(items: &[AncillaryItem]) -> Result<Vec<u8>, String> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut total = 0usize;
    for (_, _, data) in items {
        let len_u32 = u32::try_from(data.len()).map_err(|_| "ancillary payload too large")?;
        let space = unsafe { libc::CMSG_SPACE(len_u32) as usize };
        total = total
            .checked_add(space)
            .ok_or_else(|| "ancillary payload too large".to_string())?;
    }
    let mut control = vec![0u8; total];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_control = control.as_mut_ptr() as *mut c_void;
    msg.msg_controllen = control
        .len()
        .try_into()
        .map_err(|_| "ancillary payload too large".to_string())?;
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg as *const _) };
    for (level, kind, data) in items {
        if cmsg.is_null() {
            return Err("ancillary header overflow".to_string());
        }
        let len_u32 = u32::try_from(data.len()).map_err(|_| "ancillary payload too large")?;
        let cmsg_len = unsafe { libc::CMSG_LEN(len_u32) as usize };
        unsafe {
            (*cmsg).cmsg_level = *level;
            (*cmsg).cmsg_type = *kind;
            (*cmsg).cmsg_len = cmsg_len as _;
            let dst = libc::CMSG_DATA(cmsg as *const _);
            std::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
            cmsg = libc::CMSG_NXTHDR(&msg as *const _, cmsg as *const _);
        }
    }
    Ok(control)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn encode_host_sendmsg_ancillary_buffer(items: &[AncillaryItem]) -> Result<Vec<u8>, String> {
    let count_u32 =
        u32::try_from(items.len()).map_err(|_| "ancillary item count too large".to_string())?;
    let mut total = 4usize;
    for (_, _, data) in items {
        total = total
            .checked_add(12)
            .and_then(|v| v.checked_add(data.len()))
            .ok_or_else(|| "ancillary payload too large".to_string())?;
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&count_u32.to_le_bytes());
    for (level, kind, data) in items {
        let len_u32 =
            u32::try_from(data.len()).map_err(|_| "ancillary payload too large".to_string())?;
        out.extend_from_slice(&level.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&len_u32.to_le_bytes());
        out.extend_from_slice(data);
    }
    Ok(out)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn decode_host_recvmsg_ancillary_buffer(buf: &[u8]) -> Result<Vec<AncillaryItem>, String> {
    if buf.len() < 4 {
        return Err("recvmsg ancillary payload too short".to_string());
    }
    let count = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    let mut offset = 4usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let header_end = match offset.checked_add(12) {
            Some(next) => next,
            None => return Err("recvmsg ancillary payload too large".to_string()),
        };
        if header_end > buf.len() {
            return Err("recvmsg ancillary payload truncated".to_string());
        }
        let level = i32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        offset += 4;
        let kind = i32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]);
        offset += 4;
        let data_len = u32::from_le_bytes([
            buf[offset],
            buf[offset + 1],
            buf[offset + 2],
            buf[offset + 3],
        ]) as usize;
        offset += 4;
        let data_end = match offset.checked_add(data_len) {
            Some(end) => end,
            None => return Err("recvmsg ancillary payload too large".to_string()),
        };
        if data_end > buf.len() {
            return Err("recvmsg ancillary payload truncated".to_string());
        }
        out.push((level, kind, buf[offset..data_end].to_vec()));
        offset = data_end;
    }
    if offset != buf.len() {
        return Err("recvmsg ancillary payload has trailing bytes".to_string());
    }
    Ok(out)
}

#[cfg(unix)]
pub(super) fn parse_recvmsg_ancillary_items(msg: &libc::msghdr) -> Vec<AncillaryItem> {
    let mut out: Vec<AncillaryItem> = Vec::new();
    let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(msg as *const _) };
    while !cmsg.is_null() {
        let cmsg_len = unsafe { (*cmsg).cmsg_len as usize };
        let header_len = unsafe { libc::CMSG_LEN(0) as usize };
        if cmsg_len >= header_len {
            let data_len = cmsg_len - header_len;
            let data_ptr = unsafe { libc::CMSG_DATA(cmsg as *const _) } as *const u8;
            let data = unsafe { std::slice::from_raw_parts(data_ptr, data_len) }.to_vec();
            let level = unsafe { (*cmsg).cmsg_level };
            let kind = unsafe { (*cmsg).cmsg_type };
            out.push((level, kind, data));
        }
        cmsg = unsafe { libc::CMSG_NXTHDR(msg as *const _, cmsg as *const _) };
    }
    out
}

pub(super) fn build_ancillary_list_bits(_py: &PyToken<'_>, items: &[(i32, i32, Vec<u8>)]) -> Result<u64, u64> {
    let mut item_bits: Vec<u64> = Vec::with_capacity(items.len());
    for (level, kind, data) in items {
        let bytes_ptr = alloc_bytes(_py, data);
        if bytes_ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let level_bits = MoltObject::from_int(*level as i64).bits();
        let kind_bits = MoltObject::from_int(*kind as i64).bits();
        let tuple_ptr = alloc_tuple(_py, &[level_bits, kind_bits, bytes_bits]);
        dec_ref_bits(_py, bytes_bits);
        if tuple_ptr.is_null() {
            for bits in item_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(MoltObject::none().bits());
        }
        item_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list(_py, item_bits.as_slice());
    for bits in item_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    Ok(MoltObject::from_ptr(list_ptr).bits())
}

pub(super) fn build_recvmsg_result_with_anc(
    _py: &PyToken<'_>,
    data: &[u8],
    msg_flags: i32,
    addr_bits: u64,
    anc_bits: u64,
) -> u64 {
    let data_ptr = alloc_bytes(_py, data);
    if data_ptr.is_null() {
        dec_ref_bits(_py, anc_bits);
        dec_ref_bits(_py, addr_bits);
        return MoltObject::none().bits();
    }
    let data_bits = MoltObject::from_ptr(data_ptr).bits();
    let flags_bits = MoltObject::from_int(msg_flags as i64).bits();
    let tuple_ptr = alloc_tuple(_py, &[data_bits, anc_bits, flags_bits, addr_bits]);
    dec_ref_bits(_py, data_bits);
    dec_ref_bits(_py, anc_bits);
    dec_ref_bits(_py, addr_bits);
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

pub(super) struct RecvmsgIntoTarget {
    pub(super) ptr: *mut u8,
    pub(super) len: usize,
    pub(super) is_memoryview: bool,
}

pub(super) fn collect_recvmsg_into_targets(
    _py: &PyToken<'_>,
    buffers_bits: u64,
) -> Result<Vec<RecvmsgIntoTarget>, u64> {
    let values = iter_values_from_bits(_py, buffers_bits)?;
    if values.is_empty() {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "recvmsg_into() requires at least one buffer",
        ));
    }
    let mut out: Vec<RecvmsgIntoTarget> = Vec::with_capacity(values.len());
    for value_bits in values {
        let Some(ptr) = maybe_ptr_from_bits(value_bits) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "recvmsg_into() argument must be an iterable of writable buffers",
            ));
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTEARRAY {
                out.push(RecvmsgIntoTarget {
                    ptr,
                    len: bytearray_len(ptr),
                    is_memoryview: false,
                });
                continue;
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if memoryview_readonly(ptr) {
                    return Err(raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "recvmsg_into() argument must be writable buffers",
                    ));
                }
                out.push(RecvmsgIntoTarget {
                    ptr,
                    len: memoryview_len(ptr),
                    is_memoryview: true,
                });
                continue;
            }
        }
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "recvmsg_into() argument must be an iterable of writable buffers",
        ));
    }
    Ok(out)
}

pub(super) fn write_recvmsg_into_targets(
    _py: &PyToken<'_>,
    targets: &[RecvmsgIntoTarget],
    data: &[u8],
) -> Result<(), u64> {
    let mut offset = 0usize;
    for target in targets {
        if offset >= data.len() {
            break;
        }
        let count = (data.len() - offset).min(target.len);
        if count == 0 {
            continue;
        }
        let chunk = &data[offset..offset + count];
        if target.is_memoryview {
            if let Some(slice) = unsafe { memoryview_bytes_slice_mut(target.ptr) } {
                let n = chunk.len().min(slice.len());
                slice[..n].copy_from_slice(&chunk[..n]);
            } else if let Err(msg) = unsafe { memoryview_write_bytes(target.ptr, chunk) } {
                return Err(raise_exception::<u64>(_py, "TypeError", &msg));
            }
        } else {
            let dst = unsafe { bytearray_vec(target.ptr) };
            let n = chunk.len().min(dst.len());
            dst[..n].copy_from_slice(&chunk[..n]);
        }
        offset += count;
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(super) fn socket_handle_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<i64, String> {
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
pub(super) fn errno_from_rc(rc: i32) -> i32 {
    if rc < 0 { -rc } else { 0 }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn would_block_errno(errno: i32) -> bool {
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

pub(super) enum SocketReaderPull {
    Eof,
    Data,
}

pub(super) const SOCKET_READER_COMPACT_PREFIX_MIN: usize = 4096;

pub(super) unsafe fn socket_reader_pull(
    _py: &PyToken<'_>,
    reader: &mut MoltSocketReader,
) -> Result<SocketReaderPull, u64> {
    unsafe {
        if reader.eof {
            return Ok(SocketReaderPull::Eof);
        }
        let recv_bits = molt_socket_recv(
            reader.socket_bits,
            MoltObject::from_int(4096).bits(),
            MoltObject::from_int(0).bits(),
        );
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let recv_obj = obj_from_bits(recv_bits);
        let Some(recv_ptr) = recv_obj.as_ptr() else {
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "socket recv intrinsic returned invalid value",
            ));
        };
        if object_type_id(recv_ptr) != TYPE_ID_BYTES {
            dec_ref_bits(_py, recv_bits);
            return Err(raise_exception::<u64>(
                _py,
                "RuntimeError",
                "socket recv intrinsic returned invalid value",
            ));
        }
        let n = bytes_len(recv_ptr);
        if n == 0 {
            reader.eof = true;
            dec_ref_bits(_py, recv_bits);
            return Ok(SocketReaderPull::Eof);
        }
        let bytes = std::slice::from_raw_parts(bytes_data(recv_ptr), n);
        reader.buffer.extend_from_slice(bytes);
        dec_ref_bits(_py, recv_bits);
        Ok(SocketReaderPull::Data)
    }
}

#[inline]
pub(super) fn socket_reader_unread_len(reader: &MoltSocketReader) -> usize {
    reader.buffer.len().saturating_sub(reader.buffer_start)
}

#[inline]
pub(super) fn socket_reader_unread_is_empty(reader: &MoltSocketReader) -> bool {
    socket_reader_unread_len(reader) == 0
}

#[inline]
pub(super) fn socket_reader_unread_slice(reader: &MoltSocketReader) -> &[u8] {
    &reader.buffer[reader.buffer_start..]
}

pub(super) fn socket_reader_maybe_compact(reader: &mut MoltSocketReader) {
    let consumed = reader.buffer_start;
    if consumed == 0 {
        return;
    }
    if consumed >= reader.buffer.len() {
        reader.buffer.clear();
        reader.buffer_start = 0;
        reader.scan_cursor = 0;
        return;
    }
    if consumed < SOCKET_READER_COMPACT_PREFIX_MIN
        || consumed.saturating_mul(2) < reader.buffer.len()
    {
        return;
    }
    let remaining = reader.buffer.len() - consumed;
    reader.buffer.copy_within(consumed.., 0);
    reader.buffer.truncate(remaining);
    reader.buffer_start = 0;
    reader.scan_cursor = reader.scan_cursor.saturating_sub(consumed);
}

pub(super) fn socket_reader_find_newline_up_to(
    reader: &mut MoltSocketReader,
    max_bytes: Option<usize>,
) -> Option<usize> {
    let unread_start = reader.buffer_start;
    let unread_end = reader.buffer.len();
    let search_end = match max_bytes {
        Some(limit) => unread_start.saturating_add(limit).min(unread_end),
        None => unread_end,
    };
    let search_start = reader.scan_cursor.max(unread_start).min(search_end);
    if search_start == search_end {
        reader.scan_cursor = reader.scan_cursor.max(search_end);
        return None;
    }
    match reader.buffer[search_start..search_end]
        .iter()
        .position(|&b| b == b'\n')
    {
        Some(rel_idx) => {
            let idx = search_start + rel_idx;
            reader.scan_cursor = idx.saturating_add(1);
            Some(idx - unread_start)
        }
        None => {
            reader.scan_cursor = reader.scan_cursor.max(search_end);
            None
        }
    }
}

pub(super) fn socket_reader_take(_py: &PyToken<'_>, reader: &mut MoltSocketReader, count: usize) -> u64 {
    let n = count.min(socket_reader_unread_len(reader));
    let unread = socket_reader_unread_slice(reader);
    let ptr = alloc_bytes(_py, &unread[..n]);
    if ptr.is_null() {
        reader.scan_cursor = reader.buffer_start;
        return MoltObject::none().bits();
    }
    reader.buffer_start += n;
    reader.scan_cursor = reader.scan_cursor.max(reader.buffer_start);
    socket_reader_maybe_compact(reader);
    MoltObject::from_ptr(ptr).bits()
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket handle from `molt_socket_new`/`molt_socket_clone`.
pub unsafe extern "C" fn molt_socket_reader_new(sock_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let clone_bits = molt_socket_clone(sock_bits);
            if obj_from_bits(clone_bits).is_none() {
                return MoltObject::none().bits();
            }
            let reader = Box::new(MoltSocketReader {
                socket_bits: clone_bits,
                buffer: Vec::new(),
                buffer_start: 0,
                scan_cursor: 0,
                eof: false,
            });
            bits_from_ptr(Box::into_raw(reader) as *mut u8)
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_drop(reader_bits: u64) {
    unsafe {
        crate::with_gil_entry!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return;
            }
            let reader = Box::from_raw(reader_ptr as *mut MoltSocketReader);
            molt_socket_drop(reader.socket_bits);
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_at_eof(reader_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return MoltObject::from_bool(true).bits();
            }
            let reader = &*(reader_ptr as *mut MoltSocketReader);
            MoltObject::from_bool(reader.eof && socket_reader_unread_is_empty(reader)).bits()
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_read(reader_bits: u64, n_bits: u64) -> u64 {
    unsafe {
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
                        return socket_reader_take(_py, reader, socket_reader_unread_len(reader));
                    }
                    match socket_reader_pull(_py, reader) {
                        Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                        Err(bits) => return bits,
                    }
                }
            }
            if !socket_reader_unread_is_empty(reader) {
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
                    Ok(SocketReaderPull::Eof) => {
                        if socket_reader_unread_is_empty(reader) {
                            let ptr = alloc_bytes(_py, &[]);
                            if ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(ptr).bits();
                        }
                        return socket_reader_take(_py, reader, n as usize);
                    }
                    Ok(SocketReaderPull::Data) => {
                        if !socket_reader_unread_is_empty(reader) {
                            return socket_reader_take(_py, reader, n as usize);
                        }
                    }
                    Err(bits) => return bits,
                }
            }
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_readline(reader_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let reader = &mut *(reader_ptr as *mut MoltSocketReader);
            loop {
                if let Some(idx) = socket_reader_find_newline_up_to(reader, None) {
                    return socket_reader_take(_py, reader, idx + 1);
                }
                if reader.eof {
                    return socket_reader_take(_py, reader, socket_reader_unread_len(reader));
                }
                match socket_reader_pull(_py, reader) {
                    Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                    Err(bits) => return bits,
                }
            }
        })
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid socket reader handle from `molt_socket_reader_new`.
pub unsafe extern "C" fn molt_socket_reader_readline_limit(
    reader_bits: u64,
    limit_bits: u64,
) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let reader_ptr = ptr_from_bits(reader_bits);
            if reader_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let Some(limit_raw) = to_i64(obj_from_bits(limit_bits)) else {
                return raise_exception::<u64>(_py, "TypeError", "size must be an integer");
            };
            let reader = &mut *(reader_ptr as *mut MoltSocketReader);
            if limit_raw == 0 {
                let ptr = alloc_bytes(_py, &[]);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
            if limit_raw < 0 {
                loop {
                    if let Some(idx) = socket_reader_find_newline_up_to(reader, None) {
                        return socket_reader_take(_py, reader, idx + 1);
                    }
                    if reader.eof {
                        return socket_reader_take(_py, reader, socket_reader_unread_len(reader));
                    }
                    match socket_reader_pull(_py, reader) {
                        Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                        Err(bits) => return bits,
                    }
                }
            }
            let limit = usize::try_from(limit_raw).unwrap_or(usize::MAX);
            loop {
                if let Some(idx) = socket_reader_find_newline_up_to(reader, Some(limit)) {
                    return socket_reader_take(_py, reader, idx + 1);
                }
                if socket_reader_unread_len(reader) >= limit {
                    return socket_reader_take(_py, reader, limit);
                }
                if reader.eof {
                    return socket_reader_take(
                        _py,
                        reader,
                        socket_reader_unread_len(reader).min(limit),
                    );
                }
                match socket_reader_pull(_py, reader) {
                    Ok(SocketReaderPull::Eof) | Ok(SocketReaderPull::Data) => {}
                    Err(bits) => return bits,
                }
            }
        })
    }
}

pub(super) fn host_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<Option<String>, String> {
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

pub(super) fn port_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<u16, String> {
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

pub(super) fn service_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<Option<String>, String> {
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
pub(super) fn unix_path_from_bits(_py: &PyToken<'_>, addr_bits: u64) -> Result<std::path::PathBuf, String> {
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

#[cfg(molt_has_net_io)]
pub(super) fn sockaddr_from_bits(_py: &PyToken<'_>, addr_bits: u64, family: i32) -> Result<SockAddr, String> {
    if family == libc::AF_UNIX {
        #[cfg(all(unix, molt_has_net_io))]
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

#[cfg(molt_has_net_io)]
pub(super) fn sockaddr_to_bits(_py: &PyToken<'_>, addr: &SockAddr) -> u64 {
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
pub(super) fn encode_sockaddr(_py: &PyToken<'_>, addr_bits: u64, family: i32) -> Result<Vec<u8>, String> {
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
pub(super) fn decode_sockaddr(_py: &PyToken<'_>, buf: &[u8]) -> Result<u64, String> {
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

#[cfg(molt_has_net_io)]
pub(super) fn socket_wait_ready(
    _py: &PyToken<'_>,
    socket_ptr: *mut u8,
    events: u32,
) -> Result<(), std::io::Error> {
    let timeout = socket_timeout(socket_ptr);
    #[cfg(unix)]
    {
        let fd = with_socket_mut(socket_ptr, |inner| {
            let fd = inner
                .raw_fd()
                .ok_or_else(|| std::io::Error::new(ErrorKind::NotConnected, "socket closed"))?;
            Ok(fd)
        })?;
        socket_wait_ready_poll(fd, events, timeout)
    }
    #[cfg(not(unix))]
    {
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
}

#[cfg(unix)]
pub(super) fn socket_wait_ready_poll(
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
    let rc = {
        let _release = crate::concurrency::GilReleaseGuard::new();
        unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) }
    };
    if rc < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if rc == 0 {
        return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
    }
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(super) fn socket_wait_ready(_py: &PyToken<'_>, handle: i64, events: u32) -> Result<(), std::io::Error> {
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
        if rc == -libc::ETIMEDOUT {
            return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
        }
        return Err(std::io::Error::from_raw_os_error(-rc));
    }
    let rc = unsafe { crate::molt_socket_wait_host(handle, events, -1) };
    if rc == 0 {
        return Ok(());
    }
    if rc == -libc::ETIMEDOUT {
        return Err(std::io::Error::new(ErrorKind::TimedOut, "timed out"));
    }
    Err(std::io::Error::from_raw_os_error(-rc))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn os_string_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<OsString, String> {
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

#[cfg(all(unix, molt_has_net_io))]
pub(super) type LibcSocket = c_int;
#[cfg(all(windows, molt_has_net_io))]
pub(super) type LibcSocket = libc::SOCKET;

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn libc_socket(fd: RawFd) -> LibcSocket {
    fd
}
#[cfg(all(windows, molt_has_net_io))]
pub(super) fn libc_socket(fd: RawSocket) -> LibcSocket {
    fd as LibcSocket
}

#[cfg(molt_has_net_io)]
pub(super) fn connect_raw_socket(fd: SocketFd, sockaddr: &SockAddr) -> std::io::Result<()> {
    let ret = unsafe {
        libc::connect(
            libc_socket(fd),
            sockaddr.as_ptr() as *const libc::sockaddr,
            sockaddr.len(),
        )
    };
    if ret == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(molt_has_net_io)]
pub(super) fn sock_addr_from_storage(storage: libc::sockaddr_storage, len: libc::socklen_t) -> SockAddr {
    let mut addr_storage = SockAddrStorage::zeroed();
    unsafe {
        *addr_storage.view_as::<libc::sockaddr_storage>() = storage;
        SockAddr::new(addr_storage, len)
    }
}

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn socket_is_acceptor(socket: &Socket) -> bool {
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

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn socket_is_acceptor(_socket: &Socket) -> bool {
    false
}

#[cfg(unix)]
pub(super) fn socket_relisten(fd: RawFd, backlog: i32) -> std::io::Result<()> {
    let rc = unsafe { libc::listen(fd, backlog) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn socket_relisten(socket: RawSocket, backlog: i32) -> std::io::Result<()> {
    let rc = unsafe { libc::listen(socket, backlog) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn with_sockref<T, F>(fd: RawFd, f: F) -> T
where
    F: FnOnce(SockRef<'_>) -> T,
{
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let sock_ref = SockRef::from(&borrowed);
    f(sock_ref)
}

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn with_sockref<T, F>(socket: RawSocket, f: F) -> T
where
    F: FnOnce(SockRef<'_>) -> T,
{
    let borrowed = unsafe { BorrowedSocket::borrow_raw(socket) };
    let sock_ref = SockRef::from(&borrowed);
    f(sock_ref)
}

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn take_error_raw(fd: RawFd) -> std::io::Result<Option<std::io::Error>> {
    with_sockref(fd, |sock_ref| sock_ref.take_error())
}

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn take_error_raw(socket: RawSocket) -> std::io::Result<Option<std::io::Error>> {
    with_sockref(socket, |sock_ref| sock_ref.take_error())
}

#[cfg(all(unix, molt_has_net_io))]
pub(super) fn take_error_mio<T: AsRawFd>(sock: &T) -> std::io::Result<Option<std::io::Error>> {
    take_error_raw(sock.as_raw_fd())
}

#[cfg(all(windows, molt_has_net_io))]
pub(super) fn take_error_mio<T: AsRawSocket>(sock: &T) -> std::io::Result<Option<std::io::Error>> {
    take_error_raw(sock.as_raw_socket())
}

