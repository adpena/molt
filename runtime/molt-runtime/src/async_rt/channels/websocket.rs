use crossbeam_channel::{Receiver, Sender};
#[cfg(molt_has_net_io)]
use std::net::TcpStream;
#[cfg(molt_has_net_io)]
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use super::super::poll::ws_wait_poll_fn_addr;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use super::super::sockets::require_net_capability;
use super::super::sockets::{SendData, send_data_from_bits};
#[cfg(any(target_arch = "wasm32", molt_has_net_io))]
use super::super::{current_token_id, token_id_from_bits};
use super::stream::bytes_channel;
#[cfg(molt_has_net_io)]
use super::stream::{default_stream_max_queued_bytes, stream_enqueue_bytes_blocking};
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use crate::audit::{AuditArgs, audit_capability_decision};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(target_arch = "wasm32")]
use crate::string_obj_to_owned;
#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
use crate::{
    IO_EVENT_ERROR, IO_EVENT_READ, IO_EVENT_WRITE, dec_ref_bits, has_capability,
    header_from_obj_ptr, inc_ref_bits, monotonic_now_secs, obj_from_bits, resolve_obj_ptr,
    runtime_state, to_f64, to_i64,
};
use crate::{
    MoltObject, PyToken, alloc_bytes, alloc_tuple, opaque_handle_bits, pending_bits_i64,
    ptr_from_bits, raise_exception, release_ptr, usize_from_bits,
};
#[cfg(molt_has_net_io)]
use crate::{
    alloc_string, exception_pending, intern_static_name, is_missing_bits, missing_bits,
    molt_getattr_builtin, raise_os_error, runtime_static_name_slot,
};
#[cfg(molt_has_net_io)]
use mio::net::TcpStream as MioTcpStream;
#[cfg(molt_has_net_io)]
use tungstenite::stream::MaybeTlsStream;
#[cfg(molt_has_net_io)]
use tungstenite::{Message, WebSocket, connect};
#[cfg(molt_has_net_io)]
mod tls_native;
#[cfg(molt_has_net_io)]
use tls_native::{
    tls_client_connect_native, tls_client_from_fd_native, tls_server_from_fd_native,
    tls_server_ssl_attr_string,
};

pub struct MoltWebSocket {
    pub sender: Sender<Vec<u8>>,
    pub receiver: Receiver<Vec<u8>>,
    pub closed: AtomicBool,
    pub refs: AtomicUsize,
    pub is_native: bool,
    pub send_hook: Option<extern "C" fn(*mut u8, *const u8, usize) -> i64>,
    pub recv_hook: Option<extern "C" fn(*mut u8) -> i64>,
    pub close_hook: Option<extern "C" fn(*mut u8)>,
    pub hook_ctx: *mut u8,
}

#[cfg(molt_has_net_io)]
struct NativeWebSocket {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    pending_pong: Option<Vec<u8>>,
    closed: bool,
    poll_stream_state: WsPollStreamState,
}

#[cfg(molt_has_net_io)]
#[derive(Copy, Clone, PartialEq, Eq)]
enum WsPollStreamState {
    Unregistered,
    InFlight,
    Registered,
}

#[cfg(molt_has_net_io)]
struct WsPollStream {
    stream: MioTcpStream,
    ctx: *const Mutex<NativeWebSocket>,
}

/// # Safety
/// `out_left` and `out_right` must be valid, writable pointers to `u64` slots.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_ws_pair(
    capacity_bits: u64,
    out_left: *mut u64,
    out_right: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if out_left.is_null() || out_right.is_null() {
            return 2;
        }
        let capacity = usize_from_bits(capacity_bits);
        let (a_tx, a_rx) = bytes_channel(capacity);
        let (b_tx, b_rx) = bytes_channel(capacity);
        let left = Box::new(MoltWebSocket {
            sender: a_tx,
            receiver: b_rx,
            closed: AtomicBool::new(false),
            refs: AtomicUsize::new(1),
            is_native: false,
            send_hook: None,
            recv_hook: None,
            close_hook: None,
            hook_ctx: std::ptr::null_mut(),
        });
        let right = Box::new(MoltWebSocket {
            sender: b_tx,
            receiver: a_rx,
            closed: AtomicBool::new(false),
            refs: AtomicUsize::new(1),
            is_native: false,
            send_hook: None,
            recv_hook: None,
            close_hook: None,
            hook_ctx: std::ptr::null_mut(),
        });
        // SAFETY: caller contract guarantees writable output pointers.
        unsafe {
            *out_left = opaque_handle_bits(Box::into_raw(left) as *mut u8);
            *out_right = opaque_handle_bits(Box::into_raw(right) as *mut u8);
        }
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ws_pair_obj(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut left = 0u64;
        let mut right = 0u64;
        let rc = unsafe { molt_ws_pair(capacity_bits, &mut left, &mut right) };
        if rc != 0 {
            return raise_exception::<_>(_py, "RuntimeError", "molt_ws_pair failed");
        }
        let tuple_ptr = alloc_tuple(_py, &[left, right]);
        if tuple_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ws_new_with_hooks(
    send_hook: usize,
    recv_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY (all three hook transmutes below): Same contract as
        // molt_stream_new_with_io_hooks: the host embedder supplies valid function
        // pointers as usize values. Non-zero means the pointer is a valid
        // extern "C" fn with the declared signature. The host must keep the backing
        // code alive for the lifetime of this MoltWebSocket.
        // Violation causes UB when the hook is later invoked.
        let send_hook = if send_hook == 0 {
            None
        } else {
            Some(unsafe {
                std::mem::transmute::<usize, extern "C" fn(*mut u8, *const u8, usize) -> i64>(
                    send_hook,
                )
            })
        };
        let recv_hook = if recv_hook == 0 {
            None
        } else {
            Some(unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u8) -> i64>(recv_hook) })
        };
        let close_hook = if close_hook == 0 {
            None
        } else {
            Some(unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u8)>(close_hook) })
        };
        let (s, r) = bytes_channel(0);
        let ws = Box::new(MoltWebSocket {
            sender: s,
            receiver: r,
            closed: AtomicBool::new(false),
            refs: AtomicUsize::new(1),
            is_native: false,
            send_hook,
            recv_hook,
            close_hook,
            hook_ctx,
        });
        Box::into_raw(ws) as *mut u8
    })
}

#[cfg(molt_has_net_io)]
type WsConnectHook = extern "C" fn(*const u8, usize) -> *mut u8;

#[cfg(molt_has_net_io)]
static WS_CONNECT_HOOK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
fn ws_ref_inc(ws_ptr: *mut MoltWebSocket) {
    if ws_ptr.is_null() {
        return;
    }
    let ws = unsafe { &*ws_ptr };
    ws.refs.fetch_add(1, AtomicOrdering::Relaxed);
}

fn ws_ref_dec(_py: &PyToken<'_>, ws_ptr: *mut MoltWebSocket) {
    if ws_ptr.is_null() {
        return;
    }
    let ws = unsafe { &*ws_ptr };
    if ws.refs.fetch_sub(1, AtomicOrdering::AcqRel) != 1 {
        return;
    }
    if !ws.closed.load(AtomicOrdering::Relaxed) {
        if let Some(hook) = ws.close_hook {
            hook(ws.hook_ctx);
        }
        ws.closed.store(true, AtomicOrdering::Relaxed);
    }
    #[cfg(molt_has_net_io)]
    {
        runtime_state(_py)
            .io_poller()
            .deregister_socket(_py, ws_ptr as *mut u8);
    }
    release_ptr(ws_ptr as *mut u8);
    unsafe {
        drop(Box::from_raw(ws_ptr));
    }
}

#[cfg(any(target_arch = "wasm32", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostSendOutcome {
    Sent,
    Pending,
    Error(i32),
}

#[cfg(any(target_arch = "wasm32", test))]
fn classify_host_send_result(rc: i32) -> HostSendOutcome {
    if rc == 0 {
        HostSendOutcome::Sent
    } else if rc == -libc::EWOULDBLOCK || rc == -libc::EAGAIN {
        HostSendOutcome::Pending
    } else {
        HostSendOutcome::Error(rc)
    }
}

#[cfg(test)]
mod host_send_result_tests {
    use super::{HostSendOutcome, classify_host_send_result};

    #[test]
    fn websocket_host_send_errors_never_masquerade_as_closed() {
        assert_eq!(classify_host_send_result(0), HostSendOutcome::Sent);
        assert_eq!(
            classify_host_send_result(-libc::EWOULDBLOCK),
            HostSendOutcome::Pending
        );
        assert_eq!(
            classify_host_send_result(-libc::ECONNRESET),
            HostSendOutcome::Error(-libc::ECONNRESET)
        );
    }
}

#[cfg(target_arch = "wasm32")]
extern "C" fn ws_send_host_hook(ctx: *mut u8, data_ptr: *const u8, len: usize) -> i64 {
    if ctx.is_null() {
        return pending_bits_i64();
    }
    let handle = unsafe { *(ctx as *mut i64) };
    let rc = unsafe { crate::molt_ws_send_host(handle, data_ptr, len as u64) };
    match classify_host_send_result(rc) {
        HostSendOutcome::Sent => 0,
        HostSendOutcome::Pending => pending_bits_i64(),
        HostSendOutcome::Error(errno) => {
            let guard = crate::GilGuard::new();
            raise_exception::<i64>(
                &guard.token(),
                "OSError",
                &format!("websocket host send failed with errno {}", -errno),
            )
        }
    }
}

#[cfg(target_arch = "wasm32")]
extern "C" fn ws_recv_host_hook(ctx: *mut u8) -> i64 {
    if ctx.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let handle = unsafe { *(ctx as *mut i64) };
    let mut cap = 65536usize;
    let mut buf = vec![0u8; cap];
    loop {
        let mut out_len: u32 = 0;
        let rc = unsafe {
            crate::molt_ws_recv_host(
                handle,
                buf.as_mut_ptr(),
                buf.len() as u32,
                (&mut out_len) as *mut u32,
            )
        };
        if rc == 0 {
            let len = out_len as usize;
            if len == 0 {
                return MoltObject::none().bits() as i64;
            }
            if len > buf.len() {
                cap = len;
                buf.resize(cap, 0);
                continue;
            }
            let ptr = alloc_bytes(&crate::GilGuard::new().token(), &buf[..len]);
            if ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            return MoltObject::from_ptr(ptr).bits() as i64;
        }
        if rc == -libc::EWOULDBLOCK || rc == -libc::EAGAIN {
            return pending_bits_i64();
        }
        if rc == -libc::ENOMEM && out_len as usize > buf.len() {
            cap = out_len as usize;
            buf.resize(cap, 0);
            continue;
        }
        return MoltObject::none().bits() as i64;
    }
}

#[cfg(target_arch = "wasm32")]
extern "C" fn ws_close_host_hook(ctx: *mut u8) {
    if ctx.is_null() {
        return;
    }
    let handle = unsafe { *(ctx as *mut i64) };
    let _ = unsafe { crate::molt_ws_close_host(handle) };
    unsafe {
        drop(Box::from_raw(ctx as *mut i64));
    }
}

#[cfg(target_arch = "wasm32")]
fn ws_host_handle(ws: &MoltWebSocket) -> Option<i64> {
    if ws.hook_ctx.is_null() {
        return None;
    }
    let handle = unsafe { *(ws.hook_ctx as *const i64) };
    if handle <= 0 { None } else { Some(handle) }
}

#[cfg(molt_has_net_io)]
fn ws_set_nonblocking(ws: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> std::io::Result<()> {
    match ws.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_nonblocking(true)?;
        }
        MaybeTlsStream::Rustls(stream) => {
            stream.get_ref().set_nonblocking(true)?;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(molt_has_net_io)]
fn ws_is_native(ws: &MoltWebSocket) -> bool {
    ws.is_native && !ws.hook_ctx.is_null()
}

#[cfg(molt_has_net_io)]
fn ws_prepare_poll_stream(ws: &MoltWebSocket) -> Option<WsPollStream> {
    if !ws_is_native(ws) {
        return None;
    }
    let ctx = ws.hook_ctx as *const Mutex<NativeWebSocket>;
    if ctx.is_null() {
        return None;
    }
    let mut guard = unsafe { &*ctx }.lock().unwrap();
    if guard.closed {
        return None;
    }
    if guard.poll_stream_state != WsPollStreamState::Unregistered {
        return None;
    }
    guard.poll_stream_state = WsPollStreamState::InFlight;
    let stream_ref = match guard.socket.get_ref() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => stream.get_ref(),
        _ => {
            guard.poll_stream_state = WsPollStreamState::Unregistered;
            return None;
        }
    };
    let cloned = match stream_ref.try_clone() {
        Ok(val) => val,
        Err(_) => {
            guard.poll_stream_state = WsPollStreamState::Unregistered;
            return None;
        }
    };
    let _ = cloned.set_nonblocking(true);
    Some(WsPollStream {
        stream: MioTcpStream::from_std(cloned),
        ctx,
    })
}

#[cfg(molt_has_net_io)]
fn ws_commit_poll_stream(ctx: *const Mutex<NativeWebSocket>, registered: bool) {
    if ctx.is_null() {
        return;
    }
    let mut guard = unsafe { &*ctx }.lock().unwrap();
    if guard.poll_stream_state == WsPollStreamState::InFlight {
        guard.poll_stream_state = if registered {
            WsPollStreamState::Registered
        } else {
            WsPollStreamState::Unregistered
        };
    }
}

#[cfg(molt_has_net_io)]
fn ws_is_would_block(err: &tungstenite::Error) -> bool {
    matches!(
        err,
        tungstenite::Error::Io(io_err)
            if io_err.kind() == std::io::ErrorKind::WouldBlock
    )
}

#[cfg(molt_has_net_io)]
fn ws_flush_pending_pong(ws: &mut NativeWebSocket) -> Result<(), Box<tungstenite::Error>> {
    if let Some(payload) = ws.pending_pong.take() {
        match ws.socket.send(Message::Pong(payload.clone().into())) {
            Ok(_) => Ok(()),
            Err(err) => {
                if ws_is_would_block(&err) {
                    ws.pending_pong = Some(payload);
                }
                Err(Box::new(err))
            }
        }
    } else {
        Ok(())
    }
}

#[cfg(molt_has_net_io)]
extern "C" fn ws_send_native_hook(ctx: *mut u8, data_ptr: *const u8, len: usize) -> i64 {
    if ctx.is_null() {
        return pending_bits_i64();
    }
    if data_ptr.is_null() && len != 0 {
        return MoltObject::none().bits() as i64;
    }
    let payload = unsafe { std::slice::from_raw_parts(data_ptr, len) };
    let ctx = unsafe { &*(ctx as *mut Mutex<NativeWebSocket>) };
    let mut guard = ctx.lock().unwrap();
    if guard.closed {
        return MoltObject::none().bits() as i64;
    }
    if let Err(err) = ws_flush_pending_pong(&mut guard) {
        if ws_is_would_block(err.as_ref()) {
            return pending_bits_i64();
        }
        guard.closed = true;
        return MoltObject::none().bits() as i64;
    }
    match guard.socket.send(Message::Binary(payload.to_vec().into())) {
        Ok(_) => 0,
        Err(err) if ws_is_would_block(&err) => pending_bits_i64(),
        Err(_) => {
            guard.closed = true;
            MoltObject::none().bits() as i64
        }
    }
}

#[cfg(molt_has_net_io)]
extern "C" fn ws_recv_native_hook(ctx: *mut u8) -> i64 {
    if ctx.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let ctx = unsafe { &*(ctx as *mut Mutex<NativeWebSocket>) };
    let mut guard = ctx.lock().unwrap();
    if guard.closed {
        return MoltObject::none().bits() as i64;
    }
    if let Err(err) = ws_flush_pending_pong(&mut guard) {
        if ws_is_would_block(err.as_ref()) {
            return pending_bits_i64();
        }
        guard.closed = true;
        return MoltObject::none().bits() as i64;
    }
    loop {
        match guard.socket.read() {
            Ok(Message::Binary(bytes)) => {
                let ptr = alloc_bytes(&crate::GilGuard::new().token(), &bytes);
                if ptr.is_null() {
                    return MoltObject::none().bits() as i64;
                }
                return MoltObject::from_ptr(ptr).bits() as i64;
            }
            Ok(Message::Text(text)) => {
                let ptr = alloc_bytes(&crate::GilGuard::new().token(), text.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits() as i64;
                }
                return MoltObject::from_ptr(ptr).bits() as i64;
            }
            Ok(Message::Ping(payload)) => match guard.socket.send(Message::Pong(payload.clone())) {
                Ok(_) => continue,
                Err(err) if ws_is_would_block(&err) => {
                    guard.pending_pong = Some(payload.to_vec());
                    return pending_bits_i64();
                }
                Err(_) => {
                    guard.closed = true;
                    return MoltObject::none().bits() as i64;
                }
            },
            Ok(Message::Pong(_)) => continue,
            Ok(Message::Frame(_)) => continue,
            Ok(Message::Close(_)) => {
                guard.closed = true;
                let _ = guard.socket.close(None);
                return MoltObject::none().bits() as i64;
            }
            Err(err) if ws_is_would_block(&err) => return pending_bits_i64(),
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                guard.closed = true;
                return MoltObject::none().bits() as i64;
            }
            Err(_) => {
                guard.closed = true;
                return MoltObject::none().bits() as i64;
            }
        }
    }
}

#[cfg(molt_has_net_io)]
extern "C" fn ws_close_native_hook(ctx: *mut u8) {
    if ctx.is_null() {
        return;
    }
    let ctx = unsafe { Box::from_raw(ctx as *mut Mutex<NativeWebSocket>) };
    let mut guard = ctx.lock().unwrap();
    if !guard.closed {
        guard.closed = true;
        let _ = guard.socket.close(None);
    }
}

#[cfg(molt_has_net_io)]
fn ws_connect_native(url_ptr: *const u8, url_len: usize) -> *mut u8 {
    if url_ptr.is_null() && url_len != 0 {
        return std::ptr::null_mut();
    }
    let url_bytes = unsafe { std::slice::from_raw_parts(url_ptr, url_len) };
    let url_str = match std::str::from_utf8(url_bytes) {
        Ok(val) => val,
        Err(_) => return std::ptr::null_mut(),
    };
    if !ws_url_has_supported_scheme(url_str) {
        return std::ptr::null_mut();
    }
    let (mut socket, _) = {
        let _release = GilReleaseGuard::new();
        match connect(url_str) {
            Ok(val) => val,
            Err(_) => return std::ptr::null_mut(),
        }
    };
    if ws_set_nonblocking(&mut socket).is_err() {
        return std::ptr::null_mut();
    }
    let ctx_ptr = Box::into_raw(Box::new(Mutex::new(NativeWebSocket {
        socket,
        pending_pong: None,
        closed: false,
        poll_stream_state: WsPollStreamState::Unregistered,
    }))) as *mut u8;
    let ws_ptr = molt_ws_new_with_hooks(
        ws_send_native_hook as *const () as usize,
        ws_recv_native_hook as *const () as usize,
        ws_close_native_hook as *const () as usize,
        ctx_ptr,
    );
    if ws_ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ctx_ptr as *mut Mutex<NativeWebSocket>));
        }
    } else {
        unsafe {
            let ws = &mut *(ws_ptr as *mut MoltWebSocket);
            ws.is_native = true;
        }
    }
    ws_ptr
}

#[cfg(molt_has_net_io)]
fn ws_url_has_supported_scheme(url: &str) -> bool {
    url.starts_with("ws://") || url.starts_with("wss://")
}

#[cfg(molt_has_net_io)]
pub(crate) fn ws_wait_release(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let _header = unsafe { header_from_obj_ptr(future_ptr) };
    let payload_bytes = unsafe { crate::object::object_payload_size(future_ptr) };
    if payload_bytes < std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    let ws_bits = unsafe { *payload_ptr };
    let ws_ptr = ptr_from_bits(ws_bits);
    if !ws_ptr.is_null() {
        ws_ref_dec(_py, ws_ptr as *mut MoltWebSocket);
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
pub(crate) fn ws_wait_release(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = unsafe { header_from_obj_ptr(future_ptr) };
    let payload_bytes = unsafe { crate::object::object_payload_size(future_ptr) };
    if payload_bytes < std::mem::size_of::<u64>() {
        return;
    }
    let payload_ptr = future_ptr as *mut u64;
    let ws_bits = unsafe { *payload_ptr };
    let ws_ptr = ptr_from_bits(ws_bits);
    if !ws_ptr.is_null() {
        ws_ref_dec(_py, ws_ptr as *mut MoltWebSocket);
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

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_ws_set_connect_hook(ptr: usize) {
    crate::with_gil_entry_nopanic!(_py, {
        WS_CONNECT_HOOK.store(ptr, AtomicOrdering::Release);
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass runtime-encoded values; returned handle is owned by the runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_tls_client_connect_new(
    host_bits: u64,
    port_bits: u64,
    server_hostname_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let Some(host) = string_obj_to_owned(obj_from_bits(host_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "host must be str");
        };
        if host.is_empty() {
            return raise_exception::<u64>(_py, "ValueError", "host cannot be empty");
        }
        let Some(port_raw) = to_i64(obj_from_bits(port_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "port must be int");
        };
        if !(0..=65535).contains(&port_raw) {
            return raise_exception::<u64>(_py, "OverflowError", "port out of range");
        }
        let server_name = if obj_from_bits(server_hostname_bits).is_none() {
            host.clone()
        } else {
            let Some(name) = string_obj_to_owned(obj_from_bits(server_hostname_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "server_hostname must be str or None",
                );
            };
            if name.is_empty() {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "server_hostname cannot be empty",
                );
            }
            name
        };
        match tls_client_connect_native(&host, port_raw as u16, &server_name) {
            Ok(stream_ptr) => opaque_handle_bits(stream_ptr),
            Err(err) => raise_os_error::<u64>(_py, err, "connect"),
        }
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_tls_client_connect_new(
    _host_bits: u64,
    _port_bits: u64,
    _server_hostname_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "asyncio TLS client transport is unavailable on wasm",
        )
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass a valid socket fd encoded as runtime int bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_tls_client_from_fd_new(
    fd_bits: u64,
    server_hostname_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.connect", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let Some(fd_raw) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "fd must be int");
        };
        if fd_raw < 0 {
            return raise_exception::<u64>(_py, "ValueError", "fd must be >= 0");
        }
        let server_name = if obj_from_bits(server_hostname_bits).is_none() {
            "localhost".to_string()
        } else {
            let Some(name) = string_obj_to_owned(obj_from_bits(server_hostname_bits)) else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "server_hostname must be str or None",
                );
            };
            if name.is_empty() {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "server_hostname cannot be empty",
                );
            }
            name
        };
        let stream_ptr = tls_client_from_fd_native(fd_raw, &server_name);
        if stream_ptr.is_null() {
            return raise_exception::<u64>(_py, "OSError", "asyncio TLS start_tls upgrade failed");
        }
        opaque_handle_bits(stream_ptr)
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_tls_client_from_fd_new(
    _fd_bits: u64,
    _server_hostname_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "asyncio TLS start_tls upgrade is unavailable on wasm",
        )
    })
}

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_tls_server_payload(ssl_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net.bind", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let bool_true_bits = MoltObject::from_bool(true).bits();
        let bool_false_bits = MoltObject::from_bool(false).bits();
        if obj_from_bits(ssl_bits).is_none()
            || ssl_bits == bool_true_bits
            || ssl_bits == bool_false_bits
        {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "server ssl requires a context with certfile/keyfile",
            );
        }
        let certfile = match tls_server_ssl_attr_string(
            _py,
            ssl_bits,
            runtime_static_name_slot(_py, b"certfile"),
            b"certfile",
        ) {
            Ok(Some(value)) => value,
            Ok(None) => {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "server ssl context is missing certfile",
                );
            }
            Err(bits) => return bits,
        };
        let keyfile = match tls_server_ssl_attr_string(
            _py,
            ssl_bits,
            runtime_static_name_slot(_py, b"keyfile"),
            b"keyfile",
        ) {
            Ok(Some(value)) => value,
            Ok(None) => {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "server ssl context is missing keyfile",
                );
            }
            Err(bits) => return bits,
        };

        let cert_ptr = alloc_string(_py, certfile.as_bytes());
        if cert_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let cert_bits = MoltObject::from_ptr(cert_ptr).bits();
        let key_ptr = alloc_string(_py, keyfile.as_bytes());
        if key_ptr.is_null() {
            dec_ref_bits(_py, cert_bits);
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[cert_bits, key_bits]);
        dec_ref_bits(_py, cert_bits);
        dec_ref_bits(_py, key_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_tls_server_payload(_ssl_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "asyncio TLS server payload is unavailable on wasm",
        )
    })
}

#[cfg(molt_has_net_io)]
/// # Safety
/// Caller must pass a valid socket fd and certificate/key path string bits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_asyncio_tls_server_from_fd_new(
    fd_bits: u64,
    certfile_bits: u64,
    keyfile_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.listen", "net.bind", "net"]).is_err() {
            return MoltObject::none().bits();
        }
        let Some(fd_raw) = to_i64(obj_from_bits(fd_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "fd must be int");
        };
        if fd_raw < 0 {
            return raise_exception::<u64>(_py, "ValueError", "fd must be >= 0");
        }
        let Some(certfile) = string_obj_to_owned(obj_from_bits(certfile_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "certfile must be str");
        };
        if certfile.is_empty() {
            return raise_exception::<u64>(_py, "ValueError", "certfile cannot be empty");
        }
        let Some(keyfile) = string_obj_to_owned(obj_from_bits(keyfile_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "keyfile must be str");
        };
        if keyfile.is_empty() {
            return raise_exception::<u64>(_py, "ValueError", "keyfile cannot be empty");
        }
        let stream_ptr = tls_server_from_fd_native(fd_raw, &certfile, &keyfile);
        if stream_ptr.is_null() {
            return raise_exception::<u64>(_py, "OSError", "asyncio TLS server upgrade failed");
        }
        opaque_handle_bits(stream_ptr)
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_asyncio_tls_server_from_fd_new(
    _fd_bits: u64,
    _certfile_bits: u64,
    _keyfile_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "asyncio TLS server transport is unavailable on wasm",
        )
    })
}

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `url_ptr` is valid for `url_len` bytes and `out` is writable.
pub unsafe extern "C" fn molt_ws_connect(
    url_ptr: *const u8,
    url_len_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry_nopanic!(_py, {
        if out.is_null() {
            return 2;
        }
        let url_len = usize_from_bits(url_len_bits);
        if url_ptr.is_null() && url_len != 0 {
            return 1;
        }
        let ws_allowed = has_capability(_py, "websocket.connect");
        audit_capability_decision(
            "net.websocket_connect",
            "websocket.connect",
            AuditArgs::None,
            ws_allowed,
        );
        if !ws_allowed {
            return 6;
        }
        let hook_ptr = WS_CONNECT_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            let ws_ptr = ws_connect_native(url_ptr, url_len);
            if ws_ptr.is_null() {
                return 7;
            }
            // SAFETY: caller guarantees `out` is writable when non-null.
            unsafe { *out = opaque_handle_bits(ws_ptr) };
            return 0;
        }
        // SAFETY: hook_ptr was stored into WS_CONNECT_HOOK by the host via
        // a registration function that accepts only `WsConnectHook`-typed values.
        // The AtomicUsize store/load preserves the bit pattern. The host must keep
        // the function alive for the process lifetime (static hook). Transmuting a
        // stale or mistyped pointer causes UB on the subsequent call.
        let hook: WsConnectHook = unsafe { std::mem::transmute(hook_ptr) };
        let ws_ptr = hook(url_ptr, url_len);
        if ws_ptr.is_null() {
            return 7;
        }
        // SAFETY: caller guarantees `out` is writable when non-null.
        unsafe { *out = opaque_handle_bits(ws_ptr) };
        0
    })
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_ws_connect_obj(url_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let url = match string_obj_to_owned(obj_from_bits(url_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "url must be str");
            }
        };
        let mut handle: u64 = 0;
        let rc =
            unsafe { molt_ws_connect(url.as_ptr(), url.len() as u64, &mut handle as *mut u64) };
        if rc != 0 {
            return ws_connect_error(_py, rc);
        }
        if handle == 0 {
            return ws_connect_error(_py, 7);
        }
        handle
    })
}

#[cfg(all(test, not(target_arch = "wasm32"), molt_has_net_io))]
mod tests {
    use super::ws_url_has_supported_scheme;

    #[test]
    fn websocket_scheme_gate_accepts_ws_and_wss_only() {
        assert!(ws_url_has_supported_scheme("ws://127.0.0.1:8080"));
        assert!(ws_url_has_supported_scheme("wss://example.com/socket"));
        assert!(!ws_url_has_supported_scheme("http://example.com/socket"));
        assert!(!ws_url_has_supported_scheme("https://example.com/socket"));
        assert!(!ws_url_has_supported_scheme("ftp://example.com/socket"));
    }
}

#[cfg(any(molt_has_net_io, target_arch = "wasm32"))]
fn ws_connect_error(_py: &PyToken<'_>, code: i32) -> u64 {
    match code {
        1 => raise_exception::<_>(_py, "ValueError", "websocket url payload is invalid"),
        2 => raise_exception::<_>(_py, "RuntimeError", "websocket output pointer is invalid"),
        6 => raise_exception::<_>(
            _py,
            "PermissionError",
            "missing websocket.connect capability",
        ),
        7 => raise_exception::<_>(
            _py,
            "RuntimeError",
            "websocket connect failed or host transport is unavailable",
        ),
        _ => raise_exception::<_>(_py, "RuntimeError", "websocket connect failed"),
    }
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `url_ptr` is valid for `url_len` bytes and `out` is writable.
pub unsafe extern "C" fn molt_ws_connect(
    url_ptr: *const u8,
    url_len_bits: u64,
    out: *mut u64,
) -> i32 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if out.is_null() {
                return 2;
            }
            let url_len = usize_from_bits(url_len_bits);
            if url_ptr.is_null() && url_len != 0 {
                return 1;
            }
            let ws_allowed = has_capability(_py, "websocket.connect");
            audit_capability_decision(
                "net.websocket_connect",
                "websocket.connect",
                AuditArgs::None,
                ws_allowed,
            );
            if !ws_allowed {
                return 6;
            }
            let mut handle: i64 = 0;
            let rc = unsafe {
                crate::molt_ws_connect_host(url_ptr as u32, url_len_bits, &mut handle as *mut i64)
            };
            if rc != 0 {
                return rc;
            }
            if handle == 0 {
                return 7;
            }
            let ctx_ptr = Box::into_raw(Box::new(handle)) as *mut u8;
            let ws_ptr = molt_ws_new_with_hooks(
                ws_send_host_hook as *const () as usize,
                ws_recv_host_hook as *const () as usize,
                ws_close_host_hook as *const () as usize,
                ctx_ptr,
            );
            if ws_ptr.is_null() {
                let _ = unsafe { crate::molt_ws_close_host(handle) };
                unsafe {
                    drop(Box::from_raw(ctx_ptr as *mut i64));
                }
                return 7;
            }
            *out = opaque_handle_bits(ws_ptr);
            0
        })
    }
}

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
pub extern "C" fn molt_ws_wait_new(ws_bits: u64, events_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.poll"]).is_err() {
            return MoltObject::none().bits();
        }
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return raise_exception::<_>(_py, "TypeError", "invalid websocket");
        }
        let ws = unsafe { &*(ws_ptr as *mut MoltWebSocket) };
        if ws.closed.load(AtomicOrdering::Relaxed) || !ws_is_native(ws) {
            return MoltObject::none().bits();
        }
        let events = match to_i64(obj_from_bits(events_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "events must be int"),
        };
        if events == 0 {
            return raise_exception::<_>(_py, "ValueError", "events must be non-zero");
        }
        let obj_bits = crate::molt_future_new(
            ws_wait_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = ws_bits;
            *payload_ptr.add(1) = events_bits;
            *payload_ptr.add(2) = timeout_bits;
            inc_ref_bits(_py, events_bits);
            inc_ref_bits(_py, timeout_bits);
        }
        ws_ref_inc(ws_ptr as *mut MoltWebSocket);
        obj_bits
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn molt_ws_wait_new(ws_bits: u64, events_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if require_net_capability::<u64>(_py, &["net", "net.poll"]).is_err() {
            return MoltObject::none().bits();
        }
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return raise_exception::<_>(_py, "TypeError", "invalid websocket");
        }
        let ws = unsafe { &*(ws_ptr as *mut MoltWebSocket) };
        if ws.closed.load(AtomicOrdering::Relaxed) {
            return MoltObject::none().bits();
        }
        if ws_host_handle(ws).is_none() {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "websocket wait unavailable on wasm host transport",
            );
        }
        let events = match to_i64(obj_from_bits(events_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "events must be int"),
        };
        if events == 0 {
            return raise_exception::<_>(_py, "ValueError", "events must be non-zero");
        }
        let obj_bits = crate::molt_future_new(
            ws_wait_poll_fn_addr(),
            (3 * std::mem::size_of::<u64>()) as u64,
        );
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = ws_bits;
            *payload_ptr.add(1) = events_bits;
            *payload_ptr.add(2) = timeout_bits;
            inc_ref_bits(_py, events_bits);
            inc_ref_bits(_py, timeout_bits);
        }
        ws_ref_inc(ws_ptr as *mut MoltWebSocket);
        obj_bits
    })
}

#[cfg(molt_has_net_io)]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid ws-wait awaitable object bits value.
pub unsafe extern "C" fn molt_ws_wait(obj_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        // SAFETY: `obj_bits` must reference a live awaitable object.
        let _header = unsafe { header_from_obj_ptr(obj_ptr) };
        // SAFETY: header pointer came from a live object header.
        let payload_bytes = unsafe { crate::object::object_payload_size(obj_ptr) };
        let payload_len = payload_bytes / std::mem::size_of::<u64>();
        if payload_len < 2 {
            return raise_exception::<i64>(_py, "TypeError", "ws wait payload too small");
        }
        let payload_ptr = obj_ptr as *mut u64;
        // SAFETY: payload layout is validated by `payload_len` above.
        let ws_bits = unsafe { *payload_ptr };
        // SAFETY: payload has at least two `u64` slots.
        let events_bits = unsafe { *payload_ptr.add(1) };
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return raise_exception::<i64>(_py, "TypeError", "invalid websocket");
        }
        // SAFETY: ws bits are expected to point to a live websocket.
        let ws = unsafe { &*(ws_ptr as *mut MoltWebSocket) };
        if ws.closed.load(AtomicOrdering::Relaxed) {
            let mask = IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return MoltObject::from_int(mask as i64).bits() as i64;
        }
        let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
        if events == 0 {
            return raise_exception::<i64>(_py, "ValueError", "events must be non-zero");
        }
        // SAFETY: header points at the awaitable header allocated for this object.
        if crate::object::object_state(obj_ptr) == 0 {
            let mut timeout: Option<f64> = None;
            if payload_len >= 3 {
                // SAFETY: payload length check guarantees index 2 exists.
                let timeout_bits = unsafe { *payload_ptr.add(2) };
                let timeout_obj = obj_from_bits(timeout_bits);
                if !timeout_obj.is_none() {
                    if let Some(val) = to_f64(timeout_obj) {
                        if !val.is_finite() || val < 0.0 {
                            return raise_exception::<i64>(
                                _py,
                                "ValueError",
                                "timeout must be non-negative",
                            );
                        }
                        timeout = Some(val);
                    } else {
                        return raise_exception::<i64>(
                            _py,
                            "TypeError",
                            "timeout must be float or None",
                        );
                    }
                }
            }
            if let Some(val) = timeout {
                if val == 0.0 {
                    return raise_exception::<i64>(_py, "TimeoutError", "timed out");
                }
                let deadline = monotonic_now_secs(_py) + val;
                let deadline_bits = MoltObject::from_float(deadline).bits();
                if payload_len >= 3 {
                    // SAFETY: payload length check guarantees index 2 exists.
                    dec_ref_bits(_py, unsafe { *payload_ptr.add(2) });
                    // SAFETY: payload length check guarantees index 2 exists.
                    unsafe { *payload_ptr.add(2) = deadline_bits };
                    inc_ref_bits(_py, deadline_bits);
                }
            }
            if !ws_is_native(ws) {
                return raise_exception::<i64>(_py, "RuntimeError", "websocket wait unavailable");
            }
            let poll_stream = ws_prepare_poll_stream(ws);
            let (stream, poll_ctx) = match poll_stream {
                Some(poll_stream) => (Some(poll_stream.stream), Some(poll_stream.ctx)),
                None => (None, None),
            };
            let register_result = runtime_state(_py)
                .io_poller()
                .register_ws_wait(obj_ptr, ws_ptr, events, stream);
            if let Some(ctx) = poll_ctx {
                ws_commit_poll_stream(ctx, register_result.is_ok());
            }
            if let Err(err) = register_result {
                return raise_os_error::<i64>(_py, err, "ws_wait");
            }
            // SAFETY: header points at mutable state for this awaitable object.
            crate::object::object_set_state(obj_ptr, 1);
            return pending_bits_i64();
        }
        if let Some(mask) = runtime_state(_py).io_poller().take_ready(obj_ptr) {
            let res_bits = MoltObject::from_int(mask as i64).bits();
            return res_bits as i64;
        }
        if payload_len >= 3 {
            // SAFETY: payload length check guarantees index 2 exists.
            let deadline_obj = obj_from_bits(unsafe { *payload_ptr.add(2) });
            if let Some(deadline) = to_f64(deadline_obj)
                && deadline.is_finite()
                && monotonic_now_secs(_py) >= deadline
            {
                runtime_state(_py).io_poller().cancel_waiter(obj_ptr);
                return raise_exception::<i64>(_py, "TimeoutError", "timed out");
            }
        }
        pending_bits_i64()
    })
}

#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `obj_bits` is a valid WebSocket object pointer.
pub unsafe extern "C" fn molt_ws_wait(obj_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        // SAFETY: `obj_bits` must reference a live awaitable object.
        let header = unsafe { header_from_obj_ptr(obj_ptr) };
        // SAFETY: header pointer came from a live object header.
        let payload_bytes = unsafe { crate::object::object_payload_size(obj_ptr) };
        let payload_len = payload_bytes / std::mem::size_of::<u64>();
        if payload_len < 2 {
            return raise_exception::<i64>(_py, "TypeError", "ws wait payload too small");
        }
        let payload_ptr = obj_ptr as *mut u64;
        // SAFETY: payload layout is validated by `payload_len` above.
        let ws_bits = unsafe { *payload_ptr };
        // SAFETY: payload has at least two `u64` slots.
        let events_bits = unsafe { *payload_ptr.add(1) };
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return raise_exception::<i64>(_py, "TypeError", "invalid websocket");
        }
        // SAFETY: ws bits are expected to point to a live websocket.
        let ws = unsafe { &*(ws_ptr as *mut MoltWebSocket) };
        if ws.closed.load(AtomicOrdering::Relaxed) {
            let mask = IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return MoltObject::from_int(mask as i64).bits() as i64;
        }
        let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
        if events == 0 {
            return raise_exception::<i64>(_py, "ValueError", "events must be non-zero");
        }
        // SAFETY: header points at mutable state for this awaitable object.
        if crate::object::object_state(obj_ptr) == 0 {
            let mut timeout: Option<f64> = None;
            if payload_len >= 3 {
                // SAFETY: payload length check guarantees index 2 exists.
                let timeout_bits = unsafe { *payload_ptr.add(2) };
                let timeout_obj = obj_from_bits(timeout_bits);
                if !timeout_obj.is_none() {
                    if let Some(val) = to_f64(timeout_obj) {
                        if !val.is_finite() || val < 0.0 {
                            return raise_exception::<i64>(
                                _py,
                                "ValueError",
                                "timeout must be non-negative",
                            );
                        }
                        timeout = Some(val);
                    } else {
                        return raise_exception::<i64>(
                            _py,
                            "TypeError",
                            "timeout must be float or None",
                        );
                    }
                }
            }
            if let Some(val) = timeout {
                if val == 0.0 {
                    return raise_exception::<i64>(_py, "TimeoutError", "timed out");
                }
                let deadline = monotonic_now_secs(_py) + val;
                let deadline_bits = MoltObject::from_float(deadline).bits();
                if payload_len >= 3 {
                    // SAFETY: payload length check guarantees index 2 exists.
                    dec_ref_bits(_py, unsafe { *payload_ptr.add(2) });
                    // SAFETY: payload length check guarantees index 2 exists.
                    unsafe { *payload_ptr.add(2) = deadline_bits };
                    inc_ref_bits(_py, deadline_bits);
                }
            }
            let Some(handle) = ws_host_handle(ws) else {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    "websocket wait unavailable on wasm host transport",
                );
            };
            if let Err(err) = runtime_state(_py)
                .io_poller()
                .register_ws_wait(obj_ptr, handle, events)
            {
                return raise_exception::<i64>(
                    _py,
                    "RuntimeError",
                    &format!(
                        "websocket wait registration failed on wasm host transport: {}",
                        err
                    ),
                );
            }
            // SAFETY: header points at mutable state for this awaitable object.
            crate::object::object_set_state(obj_ptr, 1);
            return pending_bits_i64();
        }
        if let Some(mask) = runtime_state(_py).io_poller().take_ready(obj_ptr) {
            let res_bits = MoltObject::from_int(mask as i64).bits();
            return res_bits as i64;
        }
        if payload_len >= 3 {
            // SAFETY: payload length check guarantees index 2 exists.
            let deadline_obj = obj_from_bits(unsafe { *payload_ptr.add(2) });
            if let Some(deadline) = to_f64(deadline_obj)
                && deadline.is_finite()
                && monotonic_now_secs(_py) >= deadline
            {
                runtime_state(_py).io_poller().cancel_waiter(obj_ptr);
                return raise_exception::<i64>(_py, "TimeoutError", "timed out");
            }
        }
        pending_bits_i64()
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `ws_bits` is valid; `data_ptr` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_ws_send(ws_bits: u64, data_ptr: *const u8, len_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ws_ptr = ptr_from_bits(ws_bits);
        let len = usize_from_bits(len_bits);
        if ws_ptr.is_null() || (data_ptr.is_null() && len != 0) {
            return pending_bits_i64();
        }
        // SAFETY: caller contract guarantees `ws_bits` points to a live websocket.
        let ws = unsafe { &*(ws_ptr as *mut MoltWebSocket) };
        if ws.send_hook.is_some() && ws.closed.load(AtomicOrdering::Relaxed) {
            return MoltObject::none().bits() as i64;
        }
        if let Some(hook) = ws.send_hook {
            return hook(ws.hook_ctx, data_ptr, len);
        }
        // SAFETY: caller contract guarantees `data_ptr` is readable for `len` bytes.
        let bytes = unsafe { std::slice::from_raw_parts(data_ptr, len) }.to_vec();
        match ws.sender.try_send(bytes) {
            Ok(_) => 0,
            Err(_) => pending_bits_i64(),
        }
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `ws_bits` is valid; `data_bits` must be bytes-like.
pub unsafe extern "C" fn molt_ws_send_obj(ws_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        // SAFETY: pointer/length pair comes from validated bytes-like object.
        unsafe { molt_ws_send(ws_bits, data_ptr, data_len as u64) as u64 }
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_recv(ws_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        // SAFETY: caller contract guarantees `ws_bits` points to a live websocket.
        let ws = unsafe { &*(ws_ptr as *mut MoltWebSocket) };
        if ws.recv_hook.is_some() && ws.closed.load(AtomicOrdering::Relaxed) {
            return MoltObject::none().bits() as i64;
        }
        if let Some(hook) = ws.recv_hook {
            return hook(ws.hook_ctx);
        }
        match ws.receiver.try_recv() {
            Ok(bytes) => {
                let ptr = alloc_bytes(_py, &bytes);
                if ptr.is_null() {
                    MoltObject::none().bits() as i64
                } else {
                    MoltObject::from_ptr(ptr).bits() as i64
                }
            }
            Err(_) => {
                if ws.closed.load(AtomicOrdering::Relaxed) {
                    MoltObject::none().bits() as i64
                } else {
                    pending_bits_i64()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_close(ws_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return;
        }
        // SAFETY: caller contract guarantees `ws_bits` points to a live websocket.
        let ws = unsafe { &*(ws_ptr as *mut MoltWebSocket) };
        if ws.closed.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        if let Some(hook) = ws.close_hook {
            hook(ws.hook_ctx);
        }
        #[cfg(molt_has_net_io)]
        {
            runtime_state(_py)
                .io_poller()
                .deregister_socket(_py, ws_ptr);
        }
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_drop(ws_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return;
        }
        ws_ref_dec(_py, ws_ptr as *mut MoltWebSocket);
    })
}
