// Socket state, registry, timeout, refcount, and runtime-custody authority.
// Owns socket handles, fd maps, WASM socket metadata, and RuntimeState integration.

#[cfg(all(molt_has_net_io, not(unix)))]
use super::ancillary::AncillaryItem;
use super::*;
#[cfg(molt_has_net_io)]
use socket2::Socket;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use std::collections::HashMap;
#[cfg(all(molt_has_net_io, not(unix)))]
use std::collections::VecDeque;
#[cfg(all(molt_has_net_io, unix))]
use std::os::unix::io::{AsRawFd, RawFd};
#[cfg(all(molt_has_net_io, windows))]
use std::os::windows::io::{AsRawSocket, RawSocket};
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use std::sync::Mutex;
#[cfg(molt_has_net_io)]
use std::sync::OnceLock;
#[cfg(molt_has_net_io)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use std::time::Duration;

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
pub(crate) struct MoltSocketInner {
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

#[cfg(molt_has_net_io)]
#[cfg(all(unix, molt_has_net_io))]
pub(super) type SocketFd = RawFd;
#[cfg(molt_has_net_io)]
#[cfg(all(windows, molt_has_net_io))]
pub(super) type SocketFd = RawSocket;

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) struct SocketRuntimeState {
    #[cfg(molt_has_net_io)]
    fd_map: Mutex<HashMap<SocketFd, PtrSlot>>,
    #[cfg(target_arch = "wasm32")]
    wasm_meta: Mutex<HashMap<i64, WasmSocketMeta>>,
    #[cfg(all(molt_has_net_io, not(unix)))]
    peer_map: Mutex<HashMap<SocketFd, SocketFd>>,
    #[cfg(all(molt_has_net_io, not(unix)))]
    ancillary_queue_map: Mutex<HashMap<SocketFd, VecDeque<PendingAncillaryChunk>>>,
}

#[cfg(all(molt_has_net_io, not(unix)))]
#[derive(Clone)]
struct PendingAncillaryChunk {
    remaining: usize,
    items: Vec<AncillaryItem>,
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
impl SocketRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(molt_has_net_io)]
            fd_map: Mutex::new(HashMap::new()),
            #[cfg(target_arch = "wasm32")]
            wasm_meta: Mutex::new(HashMap::new()),
            #[cfg(all(molt_has_net_io, not(unix)))]
            peer_map: Mutex::new(HashMap::new()),
            #[cfg(all(molt_has_net_io, not(unix)))]
            ancillary_queue_map: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn clear(&self) {
        #[cfg(molt_has_net_io)]
        self.fd_map.lock().unwrap().clear();
        #[cfg(target_arch = "wasm32")]
        self.wasm_meta.lock().unwrap().clear();
        #[cfg(all(molt_has_net_io, not(unix)))]
        {
            self.peer_map.lock().unwrap().clear();
            self.ancillary_queue_map.lock().unwrap().clear();
        }
    }

    #[cfg(molt_has_net_io)]
    fn register_fd(&self, fd: SocketFd, socket_ptr: *mut u8) {
        self.fd_map.lock().unwrap().insert(fd, PtrSlot(socket_ptr));
    }

    #[cfg(molt_has_net_io)]
    fn unregister_fd(&self, fd: SocketFd) {
        self.fd_map.lock().unwrap().remove(&fd);
    }

    #[cfg(molt_has_net_io)]
    fn ptr_from_fd(&self, fd: SocketFd) -> Option<*mut u8> {
        self.fd_map.lock().unwrap().get(&fd).map(|slot| slot.0)
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_meta_insert(&self, handle: i64, meta: WasmSocketMeta) {
        self.wasm_meta.lock().unwrap().insert(handle, meta);
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_meta_remove(&self, handle: i64) {
        self.wasm_meta.lock().unwrap().remove(&handle);
    }

    #[cfg(target_arch = "wasm32")]
    fn with_wasm_meta_mut<R, F>(&self, handle: i64, f: F) -> Result<R, String>
    where
        F: FnOnce(&mut WasmSocketMeta) -> R,
    {
        let mut guard = self.wasm_meta.lock().unwrap();
        let Some(meta) = guard.get_mut(&handle) else {
            return Err("socket closed".to_string());
        };
        Ok(f(meta))
    }

    #[cfg(target_arch = "wasm32")]
    fn wasm_meta_clone(&self, handle: i64) -> Option<WasmSocketMeta> {
        self.wasm_meta.lock().unwrap().get(&handle).cloned()
    }

    #[cfg(all(test, molt_has_net_io))]
    fn fd_map_len(&self) -> usize {
        self.fd_map.lock().unwrap().len()
    }

    #[cfg(all(molt_has_net_io, not(unix)))]
    fn register_peer_pair(&self, left: SocketFd, right: SocketFd) {
        let mut map = self.peer_map.lock().unwrap();
        map.insert(left, right);
        map.insert(right, left);
    }

    #[cfg(all(molt_has_net_io, not(unix)))]
    fn unregister_peer_state(&self, fd: SocketFd) {
        {
            let mut map = self.peer_map.lock().unwrap();
            let peer = map.remove(&fd);
            if let Some(peer_fd) = peer {
                map.remove(&peer_fd);
            }
        }
        self.ancillary_queue_map.lock().unwrap().remove(&fd);
    }

    #[cfg(all(molt_has_net_io, not(unix)))]
    fn peer_available(&self, fd: SocketFd) -> bool {
        self.peer_map.lock().unwrap().contains_key(&fd)
    }

    #[cfg(all(molt_has_net_io, not(unix)))]
    fn peer_for_fd(&self, fd: SocketFd) -> Option<SocketFd> {
        self.peer_map.lock().unwrap().get(&fd).copied()
    }

    #[cfg(all(molt_has_net_io, not(unix)))]
    fn push_ancillary(&self, fd: SocketFd, chunk: PendingAncillaryChunk) {
        self.ancillary_queue_map
            .lock()
            .unwrap()
            .entry(fd)
            .or_default()
            .push_back(chunk);
    }

    #[cfg(all(molt_has_net_io, not(unix)))]
    fn take_stream_ancillary(
        &self,
        fd: SocketFd,
        data_len: usize,
        peek: bool,
    ) -> Vec<AncillaryItem> {
        let mut map = self.ancillary_queue_map.lock().unwrap();
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
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(super) fn socket_runtime_state_for_gil() -> Option<&'static SocketRuntimeState> {
    crate::state::runtime_state::runtime_state_for_gil().map(|state| &state.socket_state)
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
pub(crate) fn socket_runtime_state_clear(state: &crate::state::runtime_state::RuntimeState) {
    state.socket_state.clear();
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
        socket_runtime_state_for_gil()
            .expect("socket fd registration requires an active RuntimeState")
            .register_fd(fd, socket_ptr);
    }
}

#[cfg(molt_has_net_io)]
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
        if let Some(state) = socket_runtime_state_for_gil() {
            state.unregister_fd(fd);
        }
        #[cfg(not(unix))]
        socket_unregister_peer_state(fd);
    }
}

#[cfg(molt_has_net_io)]
fn socket_ptr_from_fd(fd: SocketFd) -> Option<*mut u8> {
    socket_runtime_state_for_gil().and_then(|state| state.ptr_from_fd(fd))
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
impl MoltSocket {
    pub(super) fn new(
        kind: MoltSocketKind,
        family: i32,
        sock_type: i32,
        proto: i32,
        connect_pending: bool,
        timeout: Option<Duration>,
    ) -> Self {
        Self {
            inner: Mutex::new(MoltSocketInner {
                kind,
                family,
                sock_type,
                proto,
                connect_pending,
            }),
            timeout: Mutex::new(timeout),
            closed: AtomicBool::new(false),
            refs: AtomicUsize::new(1),
        }
    }
}

#[cfg(molt_has_net_io)]
pub(super) fn socket_alloc(
    kind: MoltSocketKind,
    family: i32,
    sock_type: i32,
    proto: i32,
    timeout: Option<Duration>,
    connect_pending: bool,
) -> *mut u8 {
    let socket = Box::new(MoltSocket::new(
        kind,
        family,
        sock_type,
        proto,
        connect_pending,
        timeout,
    ));
    let socket_ptr = Box::into_raw(socket) as *mut u8;
    socket_register_fd(socket_ptr);
    socket_ptr
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
pub(crate) fn socket_timeout(socket_ptr: *mut u8) -> Option<Duration> {
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
pub(crate) struct WasmSocketMeta {
    pub(super) family: i32,
    pub(super) sock_type: i32,
    pub(super) proto: i32,
    timeout: Option<Duration>,
    pub(super) connect_pending: bool,
}

#[cfg(target_arch = "wasm32")]
impl WasmSocketMeta {
    pub(crate) fn new(family: i32, sock_type: i32, proto: i32, timeout: Option<Duration>) -> Self {
        Self {
            family,
            sock_type,
            proto,
            timeout,
            connect_pending: false,
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn wasm_socket_meta_insert(handle: i64, meta: WasmSocketMeta) {
    socket_runtime_state_for_gil()
        .expect("wasm socket metadata registration requires an active RuntimeState")
        .wasm_meta_insert(handle, meta);
}

#[cfg(target_arch = "wasm32")]
pub(super) fn wasm_socket_meta_clone(handle: i64) -> Option<WasmSocketMeta> {
    socket_runtime_state_for_gil().and_then(|state| state.wasm_meta_clone(handle))
}

#[cfg(target_arch = "wasm32")]
pub(super) fn wasm_socket_meta_remove(handle: i64) {
    if let Some(state) = socket_runtime_state_for_gil() {
        state.wasm_meta_remove(handle);
    }
}

#[cfg(target_arch = "wasm32")]
fn with_wasm_socket_meta_mut<R, F>(handle: i64, f: F) -> Result<R, String>
where
    F: FnOnce(&mut WasmSocketMeta) -> R,
{
    socket_runtime_state_for_gil()
        .ok_or_else(|| "socket runtime unavailable".to_string())?
        .with_wasm_meta_mut(handle, f)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn wasm_socket_family(handle: i64) -> Result<i32, String> {
    socket_runtime_state_for_gil()
        .and_then(|state| state.wasm_meta_clone(handle))
        .map(|meta| meta.family)
        .ok_or_else(|| "socket closed".to_string())
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn socket_timeout(handle: i64) -> Option<Duration> {
    socket_runtime_state_for_gil()
        .and_then(|state| state.wasm_meta_clone(handle))
        .and_then(|meta| meta.timeout)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn socket_set_timeout(handle: i64, timeout: Option<Duration>) -> Result<(), String> {
    with_wasm_socket_meta_mut(handle, |meta| {
        meta.timeout = timeout;
    })
}

#[cfg(target_arch = "wasm32")]
pub(super) fn socket_connect_pending(handle: i64) -> bool {
    socket_runtime_state_for_gil()
        .and_then(|state| state.wasm_meta_clone(handle))
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
pub(super) fn socket_close_ptr(_py: &PyToken<'_>, socket_ptr: *mut u8) {
    if socket_ptr.is_null() {
        return;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    if socket.closed.load(AtomicOrdering::Relaxed) {
        return;
    }
    socket_unregister_fd(socket_ptr);
    runtime_state(_py)
        .io_poller()
        .deregister_socket(_py, socket_ptr);
    socket.closed.store(true, AtomicOrdering::Relaxed);
    let mut guard = socket.inner.lock().unwrap();
    guard.kind = MoltSocketKind::Closed;
}

#[cfg(molt_has_net_io)]
pub(super) fn socket_detach_raw(_py: &PyToken<'_>, socket_ptr: *mut u8) -> i64 {
    if socket_ptr.is_null() {
        return -1;
    }
    let socket = unsafe { &*(socket_ptr as *mut MoltSocket) };
    socket_unregister_fd(socket_ptr);
    runtime_state(_py)
        .io_poller()
        .deregister_socket(_py, socket_ptr);
    socket.closed.store(true, AtomicOrdering::Relaxed);
    let mut guard = socket.inner.lock().unwrap();
    let kind = std::mem::replace(&mut guard.kind, MoltSocketKind::Closed);
    #[cfg(all(unix, molt_has_net_io))]
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
    #[cfg(all(windows, molt_has_net_io))]
    {
        match kind {
            MoltSocketKind::Pending(sock) => sock.into_raw_socket() as i64,
            MoltSocketKind::TcpStream(sock) => sock.into_raw_socket() as i64,
            MoltSocketKind::TcpListener(sock) => sock.into_raw_socket() as i64,
            MoltSocketKind::UdpSocket(sock) => sock.into_raw_socket() as i64,
            MoltSocketKind::Closed => -1,
        }
    }
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
    socket_close_ptr(_py, socket_ptr);
    release_ptr(socket_ptr);
    unsafe {
        drop(Box::from_raw(socket_ptr as *mut MoltSocket));
    }
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(crate) fn socket_register_peer_pair(left: SocketFd, right: SocketFd) {
    socket_runtime_state_for_gil()
        .expect("socket peer registration requires an active RuntimeState")
        .register_peer_pair(left, right);
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_unregister_peer_state(fd: SocketFd) {
    if let Some(state) = socket_runtime_state_for_gil() {
        state.unregister_peer_state(fd);
    }
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_peer_available(fd: SocketFd) -> bool {
    socket_runtime_state_for_gil()
        .map(|state| state.peer_available(fd))
        .unwrap_or(false)
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
    let state = socket_runtime_state_for_gil()
        .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EOPNOTSUPP))?;
    let peer = state
        .peer_for_fd(fd)
        .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EOPNOTSUPP))?;
    state.push_ancillary(
        peer,
        PendingAncillaryChunk {
            remaining: data_len,
            items: items.to_vec(),
        },
    );
    Ok(())
}

#[cfg(all(molt_has_net_io, not(unix)))]
pub(super) fn socket_take_stream_ancillary(
    fd: SocketFd,
    data_len: usize,
    peek: bool,
) -> Vec<AncillaryItem> {
    if data_len == 0 {
        return Vec::new();
    }
    socket_runtime_state_for_gil()
        .map(|state| state.take_stream_ancillary(fd, data_len, peek))
        .unwrap_or_default()
}

#[cfg(all(test, molt_has_net_io))]
mod socket_runtime_state_tests {
    fn test_fd(value: i32) -> SocketFd {
        #[cfg(unix)]
        {
            value
        }
        #[cfg(windows)]
        {
            value as usize
        }
    }

    #[test]
    fn fd_map_is_runtime_scoped_and_clearable() {
        let state = SocketRuntimeState::new();
        let socket_ptr = 0x1000usize as *mut u8;
        let fd = test_fd(41);

        state.register_fd(fd, socket_ptr);
        assert_eq!(state.ptr_from_fd(fd), Some(socket_ptr));
        assert_eq!(state.fd_map_len(), 1);

        state.clear();
        assert_eq!(state.ptr_from_fd(fd), None);
        assert_eq!(state.fd_map_len(), 0);
    }

    #[test]
    fn fd_unregister_removes_only_requested_socket() {
        let state = SocketRuntimeState::new();
        let first_ptr = 0x1000usize as *mut u8;
        let second_ptr = 0x2000usize as *mut u8;
        let first_fd = test_fd(41);
        let second_fd = test_fd(42);

        state.register_fd(first_fd, first_ptr);
        state.register_fd(second_fd, second_ptr);
        state.unregister_fd(first_fd);

        assert_eq!(state.ptr_from_fd(first_fd), None);
        assert_eq!(state.ptr_from_fd(second_fd), Some(second_ptr));
        assert_eq!(state.fd_map_len(), 1);
    }
}
