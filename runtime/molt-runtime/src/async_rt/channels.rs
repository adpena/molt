use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};

use crate::{
    alloc_bytes, bits_from_ptr, obj_from_bits, pending_bits_i64, ptr_from_bits, raise_exception,
    release_ptr, runtime_state, to_i64, usize_from_bits, MoltObject,
};
use super::{cancel_tokens, current_token_id, token_id_from_bits};
use super::sockets::{send_data_from_bits, SendData};

// --- Channels ---

pub struct MoltChannel {
    pub sender: Sender<i64>,
    pub receiver: Receiver<i64>,
}

pub struct MoltStream {
    pub sender: Sender<Vec<u8>>,
    pub receiver: Receiver<Vec<u8>>,
    pub closed: AtomicBool,
    pub refs: AtomicUsize,
}

pub struct MoltWebSocket {
    pub sender: Sender<Vec<u8>>,
    pub receiver: Receiver<Vec<u8>>,
    pub closed: AtomicBool,
    pub send_hook: Option<extern "C" fn(*mut u8, *const u8, usize) -> i64>,
    pub recv_hook: Option<extern "C" fn(*mut u8) -> i64>,
    pub close_hook: Option<extern "C" fn(*mut u8)>,
    pub hook_ctx: *mut u8,
}

// TODO(runtime, owner:runtime, milestone:RT1, priority:P3): consolidate channel
// creation/send/recv helpers once ExceptionSentinel supports channel pointers to
// reduce duplication across wasm/native exports.

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_chan_new(capacity_bits: u64) -> *mut u8 {
    let capacity = match to_i64(obj_from_bits(capacity_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "channel capacity must be an integer"),
    };
    if capacity < 0 {
        return raise_exception::<_>("ValueError", "channel capacity must be non-negative");
    }
    let capacity = capacity as usize;
    let (s, r) = if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    };
    let chan = Box::new(MoltChannel {
        sender: s,
        receiver: r,
    });
    Box::into_raw(chan) as *mut u8
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_chan_new(capacity_bits: u64) -> u64 {
    let capacity = match to_i64(obj_from_bits(capacity_bits)) {
        Some(val) => val,
        None => return raise_exception::<_>("TypeError", "channel capacity must be an integer"),
    };
    if capacity < 0 {
        return raise_exception::<_>("ValueError", "channel capacity must be non-negative");
    }
    let capacity = capacity as usize;
    let (s, r) = if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    };
    let chan = Box::new(MoltChannel {
        sender: s,
        receiver: r,
    });
    bits_from_ptr(Box::into_raw(chan) as *mut u8)
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_ptr` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_drop(chan_ptr: *mut u8) {
    if chan_ptr.is_null() {
        return;
    }
    drop(Box::from_raw(chan_ptr as *mut MoltChannel));
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_bits` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_drop(chan_bits: u64) {
    let chan_ptr = ptr_from_bits(chan_bits);
    if chan_ptr.is_null() {
        return;
    }
    release_ptr(chan_ptr);
    drop(Box::from_raw(chan_ptr as *mut MoltChannel));
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_ptr` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send(chan_ptr: *mut u8, val: i64) -> i64 {
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.sender.try_send(val) {
        Ok(_) => 0,                   // Ready(None)
        Err(_) => pending_bits_i64(), // PENDING
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_bits` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send(chan_bits: u64, val: i64) -> i64 {
    let chan_ptr = ptr_from_bits(chan_bits);
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.sender.try_send(val) {
        Ok(_) => 0,                   // Ready(None)
        Err(_) => pending_bits_i64(), // PENDING
    }
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_ptr` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv(chan_ptr: *mut u8) -> i64 {
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(_) => pending_bits_i64(), // PENDING
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_bits` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv(chan_bits: u64) -> i64 {
    let chan_ptr = ptr_from_bits(chan_bits);
    let chan = &*(chan_ptr as *mut MoltChannel);
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(_) => pending_bits_i64(), // PENDING
    }
}

fn bytes_channel(capacity: usize) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>) {
    if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    }
}

#[no_mangle]
pub extern "C" fn molt_stream_new(capacity_bits: u64) -> u64 {
    let capacity = usize_from_bits(capacity_bits);
    let (s, r) = bytes_channel(capacity);
    let stream = Box::new(MoltStream {
        sender: s,
        receiver: r,
        closed: AtomicBool::new(false),
        refs: AtomicUsize::new(1),
    });
    bits_from_ptr(Box::into_raw(stream) as *mut u8)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_clone(stream_bits: u64) -> u64 {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    stream.refs.fetch_add(1, AtomicOrdering::AcqRel);
    stream_bits
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_ptr` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_stream_send(
    stream_bits: u64,
    data_ptr: *const u8,
    len_bits: u64,
) -> i64 {
    let stream_ptr = ptr_from_bits(stream_bits);
    let len = usize_from_bits(len_bits);
    if stream_ptr.is_null() || (data_ptr.is_null() && len != 0) {
        return pending_bits_i64();
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    let bytes = std::slice::from_raw_parts(data_ptr, len).to_vec();
    match stream.sender.try_send(bytes) {
        Ok(_) => 0,
        Err(_) => pending_bits_i64(),
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_bits` must be bytes-like.
pub unsafe extern "C" fn molt_stream_send_obj(stream_bits: u64, data_bits: u64) -> u64 {
    let send_data = match send_data_from_bits(data_bits) {
        Ok(data) => data,
        Err(msg) => return raise_exception::<_>("TypeError", &msg),
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
    molt_stream_send(stream_bits, data_ptr, data_len as u64) as u64
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_recv(stream_bits: u64) -> i64 {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    match stream.receiver.try_recv() {
        Ok(bytes) => {
            let ptr = alloc_bytes(&bytes);
            if ptr.is_null() {
                MoltObject::none().bits() as i64
            } else {
                MoltObject::from_ptr(ptr).bits() as i64
            }
        }
        Err(_) => {
            if stream.closed.load(AtomicOrdering::Relaxed) {
                MoltObject::none().bits() as i64
            } else {
                pending_bits_i64()
            }
        }
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_close(stream_bits: u64) {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    stream.closed.store(true, AtomicOrdering::Relaxed);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `out_left` and `out_right` are valid writable pointers.
pub unsafe extern "C" fn molt_ws_pair(
    capacity_bits: u64,
    out_left: *mut u64,
    out_right: *mut u64,
) -> i32 {
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
        send_hook: None,
        recv_hook: None,
        close_hook: None,
        hook_ctx: std::ptr::null_mut(),
    });
    let right = Box::new(MoltWebSocket {
        sender: b_tx,
        receiver: a_rx,
        closed: AtomicBool::new(false),
        send_hook: None,
        recv_hook: None,
        close_hook: None,
        hook_ctx: std::ptr::null_mut(),
    });
    *out_left = bits_from_ptr(Box::into_raw(left) as *mut u8);
    *out_right = bits_from_ptr(Box::into_raw(right) as *mut u8);
    0
}

#[no_mangle]
pub extern "C" fn molt_ws_new_with_hooks(
    send_hook: usize,
    recv_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    let send_hook = if send_hook == 0 {
        None
    } else {
        Some(unsafe {
            std::mem::transmute::<usize, extern "C" fn(*mut u8, *const u8, usize) -> i64>(send_hook)
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
        send_hook,
        recv_hook,
        close_hook,
        hook_ctx,
    });
    Box::into_raw(ws) as *mut u8
}

type WsConnectHook = extern "C" fn(*const u8, usize) -> *mut u8;
type DbHostHook = extern "C" fn(*const u8, usize, *mut u64, u64) -> i32;

static WS_CONNECT_HOOK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DB_QUERY_HOOK: AtomicUsize = AtomicUsize::new(0);
static DB_EXEC_HOOK: AtomicUsize = AtomicUsize::new(0);

#[no_mangle]
pub extern "C" fn molt_ws_set_connect_hook(ptr: usize) {
    WS_CONNECT_HOOK.store(ptr, AtomicOrdering::Release);
}

#[no_mangle]
pub extern "C" fn molt_db_set_query_hook(ptr: usize) {
    DB_QUERY_HOOK.store(ptr, AtomicOrdering::Release);
}

#[no_mangle]
pub extern "C" fn molt_db_set_exec_hook(ptr: usize) {
    DB_EXEC_HOOK.store(ptr, AtomicOrdering::Release);
}

fn load_capabilities() -> HashSet<String> {
    let mut set = HashSet::new();
    let caps = std::env::var("MOLT_CAPABILITIES").unwrap_or_default();
    for cap in caps.split(',') {
        let cap = cap.trim();
        if !cap.is_empty() {
            set.insert(cap.to_string());
        }
    }
    set
}

pub(crate) fn has_capability(name: &str) -> bool {
    let caps = runtime_state().capabilities.get_or_init(load_capabilities);
    caps.contains(name)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `url_ptr` is valid for `url_len` bytes and `out` is writable.
pub unsafe extern "C" fn molt_ws_connect(
    url_ptr: *const u8,
    url_len_bits: u64,
    out: *mut u64,
) -> i32 {
    if out.is_null() {
        return 2;
    }
    let url_len = usize_from_bits(url_len_bits);
    if url_ptr.is_null() && url_len != 0 {
        return 1;
    }
    if !has_capability("websocket.connect") {
        return 6;
    }
    let hook_ptr = WS_CONNECT_HOOK.load(AtomicOrdering::Acquire);
    if hook_ptr == 0 {
        // TODO(molt): Provide a host-level connect hook for production sockets.
        return 7;
    }
    let hook: WsConnectHook = std::mem::transmute(hook_ptr);
    let ws_ptr = hook(url_ptr, url_len);
    if ws_ptr.is_null() {
        return 7;
    }
    *out = bits_from_ptr(ws_ptr);
    0
}

#[no_mangle]
/// # Safety
/// Caller must ensure `req_ptr` is valid for `len_bits` bytes and `out` is writable.
pub unsafe extern "C" fn molt_db_query(
    req_ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
    token_bits: u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    if req_ptr.is_null() && len != 0 {
        return 1;
    }
    if !has_capability("db.read") {
        return 6;
    }
    cancel_tokens();
    let token_id = match token_id_from_bits(token_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => return 1,
    };
    #[cfg(target_arch = "wasm32")]
    {
        return molt_db_query_host(req_ptr as u64, len_bits, out as u64, token_id);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let hook_ptr = DB_QUERY_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            return 7;
        }
        let hook: DbHostHook = std::mem::transmute(hook_ptr);
        hook(req_ptr, len, out, token_id)
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `req_ptr` is valid for `len_bits` bytes and `out` is writable.
pub unsafe extern "C" fn molt_db_exec(
    req_ptr: *const u8,
    len_bits: u64,
    out: *mut u64,
    token_bits: u64,
) -> i32 {
    let len = usize_from_bits(len_bits);
    if out.is_null() {
        return 2;
    }
    if req_ptr.is_null() && len != 0 {
        return 1;
    }
    if !has_capability("db.write") {
        return 6;
    }
    cancel_tokens();
    let token_id = match token_id_from_bits(token_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => return 1,
    };
    #[cfg(target_arch = "wasm32")]
    {
        return molt_db_exec_host(req_ptr as u64, len_bits, out as u64, token_id);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let hook_ptr = DB_EXEC_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            return 7;
        }
        let hook: DbHostHook = std::mem::transmute(hook_ptr);
        hook(req_ptr, len, out, token_id)
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is valid; `data_ptr` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_ws_send(ws_bits: u64, data_ptr: *const u8, len_bits: u64) -> i64 {
    let ws_ptr = ptr_from_bits(ws_bits);
    let len = usize_from_bits(len_bits);
    if ws_ptr.is_null() || (data_ptr.is_null() && len != 0) {
        return pending_bits_i64();
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.send_hook {
        return hook(ws.hook_ctx, data_ptr, len);
    }
    let bytes = std::slice::from_raw_parts(data_ptr, len).to_vec();
    match ws.sender.try_send(bytes) {
        Ok(_) => 0,
        Err(_) => pending_bits_i64(),
    }
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_recv(ws_bits: u64) -> i64 {
    let ws_ptr = ptr_from_bits(ws_bits);
    if ws_ptr.is_null() {
        return MoltObject::none().bits() as i64;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.recv_hook {
        return hook(ws.hook_ctx);
    }
    match ws.receiver.try_recv() {
        Ok(bytes) => {
            let ptr = alloc_bytes(&bytes);
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
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_close(ws_bits: u64) {
    let ws_ptr = ptr_from_bits(ws_bits);
    if ws_ptr.is_null() {
        return;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if let Some(hook) = ws.close_hook {
        hook(ws.hook_ctx);
    }
    ws.closed.store(true, AtomicOrdering::Relaxed);
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_drop(stream_bits: u64) {
    let stream_ptr = ptr_from_bits(stream_bits);
    if stream_ptr.is_null() {
        return;
    }
    let stream = &*(stream_ptr as *mut MoltStream);
    if stream.refs.fetch_sub(1, AtomicOrdering::AcqRel) > 1 {
        return;
    }
    release_ptr(stream_ptr);
    drop(Box::from_raw(stream_ptr as *mut MoltStream));
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_drop(ws_bits: u64) {
    let ws_ptr = ptr_from_bits(ws_bits);
    if ws_ptr.is_null() {
        return;
    }
    let ws = &*(ws_ptr as *mut MoltWebSocket);
    if !ws.closed.load(AtomicOrdering::Relaxed) {
        if let Some(hook) = ws.close_hook {
            hook(ws.hook_ctx);
        }
    }
    release_ptr(ws_ptr);
    drop(Box::from_raw(ws_ptr as *mut MoltWebSocket));
}
