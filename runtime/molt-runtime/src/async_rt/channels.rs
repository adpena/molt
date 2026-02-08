use crossbeam_channel::{bounded, unbounded, Receiver, Sender, TryRecvError, TrySendError};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex, OnceLock};

use super::poll::ws_wait_poll_fn_addr;
use super::sockets::{require_net_capability, send_data_from_bits, SendData};
use super::{cancel_tokens, current_token_id, token_id_from_bits};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::{
    alloc_bytes, alloc_string, alloc_tuple, bits_from_ptr, dec_ref_bits, exception_pending,
    header_from_obj_ptr, inc_ref_bits, intern_static_name, is_missing_bits, missing_bits,
    molt_getattr_builtin, monotonic_now_secs, obj_from_bits, pending_bits_i64, ptr_from_bits,
    raise_exception, raise_os_error, release_ptr, resolve_obj_ptr, runtime_state,
    string_obj_to_owned, to_f64, to_i64, usize_from_bits, GilReleaseGuard, MoltObject, PyToken,
    IO_EVENT_ERROR, IO_EVENT_READ, IO_EVENT_WRITE,
};
#[cfg(target_arch = "wasm32")]
use crate::{molt_db_exec_host, molt_db_query_host};
#[cfg(not(target_arch = "wasm32"))]
use mio::net::TcpStream as MioTcpStream;
#[cfg(not(target_arch = "wasm32"))]
use rustls::pki_types::{CertificateDer, ServerName};
#[cfg(not(target_arch = "wasm32"))]
use rustls::{
    ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection, StreamOwned,
};
#[cfg(not(target_arch = "wasm32"))]
use std::fs::File;
#[cfg(not(target_arch = "wasm32"))]
use std::io::{BufReader, Read, Write};
#[cfg(not(target_arch = "wasm32"))]
use std::net::TcpStream;
#[cfg(all(not(target_arch = "wasm32"), unix))]
use std::os::unix::io::{FromRawFd, RawFd};
#[cfg(all(not(target_arch = "wasm32"), unix))]
use std::os::unix::net::UnixStream;
#[cfg(all(not(target_arch = "wasm32"), windows))]
use std::os::windows::io::{FromRawSocket, RawSocket};
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::stream::MaybeTlsStream;
#[cfg(not(target_arch = "wasm32"))]
use tungstenite::{connect, Message, WebSocket};
#[cfg(not(target_arch = "wasm32"))]
use url::Url;
#[cfg(not(target_arch = "wasm32"))]
use webpki_roots::TLS_SERVER_ROOTS;

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
    pub send_hook: Option<extern "C" fn(*mut u8, *const u8, usize) -> i64>,
    pub recv_hook: Option<extern "C" fn(*mut u8) -> i64>,
    pub close_hook: Option<extern "C" fn(*mut u8)>,
    pub hook_ctx: *mut u8,
}

struct MoltStreamReader {
    stream_bits: u64,
    buffer: Vec<u8>,
    eof: bool,
}

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

#[cfg(not(target_arch = "wasm32"))]
struct NativeWebSocket {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    pending_pong: Option<Vec<u8>>,
    closed: bool,
    poll_stream_state: WsPollStreamState,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Copy, Clone, PartialEq, Eq)]
enum WsPollStreamState {
    Unregistered,
    InFlight,
    Registered,
}

#[cfg(not(target_arch = "wasm32"))]
struct WsPollStream {
    stream: MioTcpStream,
    ctx: *const Mutex<NativeWebSocket>,
}

#[cfg(not(target_arch = "wasm32"))]
struct NativeTlsStream {
    stream: NativeTlsEndpoint,
    pending_write: Vec<u8>,
    pending_write_offset: usize,
    closed: bool,
}

#[cfg(not(target_arch = "wasm32"))]
enum NativeTlsEndpoint {
    ClientTcp(StreamOwned<ClientConnection, TcpStream>),
    #[cfg(unix)]
    ClientUnix(StreamOwned<ClientConnection, UnixStream>),
    ServerTcp(StreamOwned<ServerConnection, TcpStream>),
    #[cfg(unix)]
    ServerUnix(StreamOwned<ServerConnection, UnixStream>),
}

type ChanHandle = u64;

#[inline]
fn chan_handle_from_ptr(ptr: *mut u8) -> ChanHandle {
    bits_from_ptr(ptr)
}

#[inline]
unsafe fn chan_ptr_from_handle(handle: ChanHandle) -> *mut u8 {
    ptr_from_bits(handle)
}

#[inline]
unsafe fn chan_release_ptr(ptr: *mut u8) {
    release_ptr(ptr);
}

fn chan_try_send_impl(_py: &PyToken<'_>, chan: &MoltChannel, val: i64) -> i64 {
    let ok_bits = MoltObject::from_int(0).bits() as i64;
    let bits = val as u64;
    inc_ref_bits(_py, bits);
    match chan.sender.try_send(val) {
        Ok(_) => ok_bits,
        Err(TrySendError::Full(_)) => {
            dec_ref_bits(_py, bits);
            pending_bits_i64()
        }
        Err(TrySendError::Disconnected(_)) => {
            dec_ref_bits(_py, bits);
            raise_exception::<i64>(_py, "RuntimeError", "channel disconnected")
        }
    }
}

fn chan_try_recv_impl(_py: &PyToken<'_>, chan: &MoltChannel) -> i64 {
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(TryRecvError::Empty) => pending_bits_i64(),
        Err(TryRecvError::Disconnected) => {
            raise_exception::<i64>(_py, "RuntimeError", "channel disconnected")
        }
    }
}

fn chan_send_blocking_impl(_py: &PyToken<'_>, chan: &MoltChannel, val: i64) -> i64 {
    let ok_bits = MoltObject::from_int(0).bits() as i64;
    let bits = val as u64;
    inc_ref_bits(_py, bits);
    match chan.sender.try_send(val) {
        Ok(_) => ok_bits,
        Err(TrySendError::Full(_)) => {
            let _release = GilReleaseGuard::new();
            match chan.sender.send(val) {
                Ok(_) => ok_bits,
                Err(_) => {
                    dec_ref_bits(_py, bits);
                    raise_exception::<i64>(_py, "RuntimeError", "channel send failed")
                }
            }
        }
        Err(TrySendError::Disconnected(_)) => {
            dec_ref_bits(_py, bits);
            raise_exception::<i64>(_py, "RuntimeError", "channel disconnected")
        }
    }
}

fn chan_recv_blocking_impl(_py: &PyToken<'_>, chan: &MoltChannel) -> i64 {
    match chan.receiver.try_recv() {
        Ok(val) => val,
        Err(TryRecvError::Empty) => {
            let _release = GilReleaseGuard::new();
            match chan.receiver.recv() {
                Ok(val) => val,
                Err(_) => raise_exception::<i64>(_py, "RuntimeError", "channel recv failed"),
            }
        }
        Err(TryRecvError::Disconnected) => {
            raise_exception::<i64>(_py, "RuntimeError", "channel disconnected")
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_chan_new(capacity_bits: u64) -> ChanHandle {
    crate::with_gil_entry!(_py, {
        let capacity = match to_i64(obj_from_bits(capacity_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "channel capacity must be an integer",
                )
            }
        };
        if capacity < 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "channel capacity must be non-negative",
            );
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
        chan_handle_from_ptr(Box::into_raw(chan) as *mut u8)
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_drop(chan_handle: ChanHandle) -> u64 {
    crate::with_gil_entry!(_py, {
        let chan_ptr = chan_ptr_from_handle(chan_handle);
        if chan_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let chan = Box::from_raw(chan_ptr as *mut MoltChannel);
        while let Ok(val) = chan.receiver.try_recv() {
            dec_ref_bits(_py, val as u64);
        }
        chan_release_ptr(chan_ptr);
        drop(chan);
        MoltObject::none().bits()
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send(chan_handle: ChanHandle, val: i64) -> i64 {
    crate::with_gil_entry!(_py, {
        let chan_ptr = chan_ptr_from_handle(chan_handle);
        let chan = &*(chan_ptr as *mut MoltChannel);
        chan_try_send_impl(_py, chan, val)
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_try_send(chan_handle: ChanHandle, val: i64) -> i64 {
    crate::with_gil_entry!(_py, {
        let chan_ptr = chan_ptr_from_handle(chan_handle);
        let chan = &*(chan_ptr as *mut MoltChannel);
        chan_try_send_impl(_py, chan, val)
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_send_blocking(chan_handle: ChanHandle, val: i64) -> i64 {
    crate::with_gil_entry!(_py, {
        let chan_ptr = chan_ptr_from_handle(chan_handle);
        let chan = &*(chan_ptr as *mut MoltChannel);
        chan_send_blocking_impl(_py, chan, val)
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv(chan_handle: ChanHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        let chan_ptr = chan_ptr_from_handle(chan_handle);
        let chan = &*(chan_ptr as *mut MoltChannel);
        chan_try_recv_impl(_py, chan)
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_try_recv(chan_handle: ChanHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        let chan_ptr = chan_ptr_from_handle(chan_handle);
        let chan = &*(chan_ptr as *mut MoltChannel);
        chan_try_recv_impl(_py, chan)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv_blocking(chan_handle: ChanHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        let chan_ptr = chan_ptr_from_handle(chan_handle);
        let chan = &*(chan_ptr as *mut MoltChannel);
        chan_try_recv_impl(_py, chan)
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must ensure `chan_handle` is a valid channel pointer.
pub unsafe extern "C" fn molt_chan_recv_blocking(chan_handle: ChanHandle) -> i64 {
    crate::with_gil_entry!(_py, {
        let chan_ptr = chan_ptr_from_handle(chan_handle);
        let chan = &*(chan_ptr as *mut MoltChannel);
        chan_recv_blocking_impl(_py, chan)
    })
}

fn bytes_channel(capacity: usize) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>) {
    if capacity == 0 {
        unbounded()
    } else {
        bounded(capacity)
    }
}

enum ReaderPull {
    Pending,
    Eof,
    Data,
}

unsafe fn stream_reader_pull(
    _py: &PyToken<'_>,
    reader: &mut MoltStreamReader,
) -> Result<ReaderPull, u64> {
    if reader.eof {
        return Ok(ReaderPull::Eof);
    }
    let pending = pending_bits_i64() as u64;
    let recv_bits = molt_stream_recv(reader.stream_bits) as u64;
    if recv_bits == pending {
        return Ok(ReaderPull::Pending);
    }
    let recv_obj = obj_from_bits(recv_bits);
    if recv_obj.is_none() {
        reader.eof = true;
        return Ok(ReaderPull::Eof);
    }
    let data = match send_data_from_bits(recv_bits) {
        Ok(SendData::Borrowed(ptr, len)) => {
            unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
        }
        Ok(SendData::Owned(vec)) => vec,
        Err(msg) => return Err(raise_exception::<u64>(_py, "TypeError", &msg)),
    };
    if data.is_empty() {
        return Ok(ReaderPull::Data);
    }
    reader.buffer.extend_from_slice(&data);
    Ok(ReaderPull::Data)
}

fn stream_reader_take(_py: &PyToken<'_>, reader: &mut MoltStreamReader, count: usize) -> u64 {
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
/// Caller must pass a valid stream handle from `molt_stream_new`/`molt_stream_clone`.
pub unsafe extern "C" fn molt_stream_reader_new(stream_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let cloned_bits = molt_stream_clone(stream_bits);
        if obj_from_bits(cloned_bits).is_none() {
            return MoltObject::none().bits();
        }
        let reader = Box::new(MoltStreamReader {
            stream_bits: cloned_bits,
            buffer: Vec::new(),
            eof: false,
        });
        bits_from_ptr(Box::into_raw(reader) as *mut u8)
    })
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid stream reader handle from `molt_stream_reader_new`.
pub unsafe extern "C" fn molt_stream_reader_drop(reader_bits: u64) {
    crate::with_gil_entry!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return;
        }
        let reader = Box::from_raw(reader_ptr as *mut MoltStreamReader);
        molt_stream_drop(reader.stream_bits);
    })
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid stream reader handle from `molt_stream_reader_new`.
pub unsafe extern "C" fn molt_stream_reader_at_eof(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::from_bool(true).bits();
        }
        let reader = &*(reader_ptr as *mut MoltStreamReader);
        MoltObject::from_bool(reader.eof && reader.buffer.is_empty()).bits()
    })
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid stream reader handle from `molt_stream_reader_new`.
pub unsafe extern "C" fn molt_stream_reader_read(reader_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let reader = &mut *(reader_ptr as *mut MoltStreamReader);
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
                    return stream_reader_take(_py, reader, reader.buffer.len());
                }
                match stream_reader_pull(_py, reader) {
                    Ok(ReaderPull::Pending) => return pending_bits_i64() as u64,
                    Ok(ReaderPull::Eof) | Ok(ReaderPull::Data) => {}
                    Err(bits) => return bits,
                }
            }
        }
        if !reader.buffer.is_empty() {
            return stream_reader_take(_py, reader, n as usize);
        }
        if reader.eof {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        loop {
            match stream_reader_pull(_py, reader) {
                Ok(ReaderPull::Pending) => return pending_bits_i64() as u64,
                Ok(ReaderPull::Eof) => {
                    if reader.buffer.is_empty() {
                        let ptr = alloc_bytes(_py, &[]);
                        if ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(ptr).bits();
                    }
                    return stream_reader_take(_py, reader, n as usize);
                }
                Ok(ReaderPull::Data) => {
                    if !reader.buffer.is_empty() {
                        return stream_reader_take(_py, reader, n as usize);
                    }
                }
                Err(bits) => return bits,
            }
        }
    })
}

#[no_mangle]
/// # Safety
/// Caller must pass a valid stream reader handle from `molt_stream_reader_new`.
pub unsafe extern "C" fn molt_stream_reader_readline(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let reader = &mut *(reader_ptr as *mut MoltStreamReader);
        loop {
            if let Some(idx) = reader.buffer.iter().position(|&b| b == b'\n') {
                return stream_reader_take(_py, reader, idx + 1);
            }
            if reader.eof {
                return stream_reader_take(_py, reader, reader.buffer.len());
            }
            match stream_reader_pull(_py, reader) {
                Ok(ReaderPull::Pending) => return pending_bits_i64() as u64,
                Ok(ReaderPull::Eof) | Ok(ReaderPull::Data) => {}
                Err(bits) => return bits,
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_stream_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let capacity = usize_from_bits(capacity_bits);
        let (s, r) = bytes_channel(capacity);
        let stream = Box::new(MoltStream {
            sender: s,
            receiver: r,
            closed: AtomicBool::new(false),
            refs: AtomicUsize::new(1),
            send_hook: None,
            recv_hook: None,
            close_hook: None,
            hook_ctx: std::ptr::null_mut(),
        });
        bits_from_ptr(Box::into_raw(stream) as *mut u8)
    })
}

#[no_mangle]
pub extern "C" fn molt_stream_new_with_io_hooks(
    send_hook: usize,
    recv_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    crate::with_gil_entry!(_py, {
        let send_hook = if send_hook == 0 {
            None
        } else {
            Some(unsafe {
                std::mem::transmute::<usize, extern "C" fn(*mut u8, *const u8, usize) -> i64>(
                    send_hook,
                )
            })
        };
        let close_hook = if close_hook == 0 {
            None
        } else {
            Some(unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u8)>(close_hook) })
        };
        let recv_hook = if recv_hook == 0 {
            None
        } else {
            Some(unsafe { std::mem::transmute::<usize, extern "C" fn(*mut u8) -> i64>(recv_hook) })
        };
        let (s, r) = bytes_channel(0);
        let stream = Box::new(MoltStream {
            sender: s,
            receiver: r,
            closed: AtomicBool::new(false),
            refs: AtomicUsize::new(1),
            send_hook,
            recv_hook,
            close_hook,
            hook_ctx,
        });
        Box::into_raw(stream) as *mut u8
    })
}

#[no_mangle]
pub extern "C" fn molt_stream_new_with_hooks(
    send_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    molt_stream_new_with_io_hooks(send_hook, 0, close_hook, hook_ctx)
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_clone(stream_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let stream = &*(stream_ptr as *mut MoltStream);
        stream.refs.fetch_add(1, AtomicOrdering::AcqRel);
        stream_bits
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_ptr` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_stream_send(
    stream_bits: u64,
    data_ptr: *const u8,
    len_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        let len = usize_from_bits(len_bits);
        if stream_ptr.is_null() || (data_ptr.is_null() && len != 0) {
            return pending_bits_i64();
        }
        let stream = &*(stream_ptr as *mut MoltStream);
        if let Some(hook) = stream.send_hook {
            return hook(stream.hook_ctx, data_ptr, len);
        }
        let bytes = std::slice::from_raw_parts(data_ptr, len).to_vec();
        match stream.sender.try_send(bytes) {
            Ok(_) => 0,
            Err(_) => pending_bits_i64(),
        }
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_bits` must be bytes-like.
pub unsafe extern "C" fn molt_stream_send_obj(stream_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        molt_stream_send(stream_bits, data_ptr, data_len as u64) as u64
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_recv(stream_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let stream = &*(stream_ptr as *mut MoltStream);
        if let Some(hook) = stream.recv_hook {
            return hook(stream.hook_ctx);
        }
        match stream.receiver.try_recv() {
            Ok(bytes) => {
                let ptr = alloc_bytes(_py, &bytes);
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
                    #[cfg(target_arch = "wasm32")]
                    {
                        let _ = crate::molt_db_host_poll();
                        let _ = crate::molt_process_host_poll();
                        if let Ok(bytes) = stream.receiver.try_recv() {
                            let ptr = alloc_bytes(_py, &bytes);
                            return if ptr.is_null() {
                                MoltObject::none().bits() as i64
                            } else {
                                MoltObject::from_ptr(ptr).bits() as i64
                            };
                        }
                        if stream.closed.load(AtomicOrdering::Relaxed) {
                            return MoltObject::none().bits() as i64;
                        }
                    }
                    pending_bits_i64()
                }
            }
        }
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_close(stream_bits: u64) {
    crate::with_gil_entry!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        let stream = &*(stream_ptr as *mut MoltStream);
        if let Some(hook) = stream.close_hook {
            hook(stream.hook_ctx);
        }
        stream.closed.store(true, AtomicOrdering::Relaxed);
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `out_left` and `out_right` are valid writable pointers.
pub unsafe extern "C" fn molt_ws_pair(
    capacity_bits: u64,
    out_left: *mut u64,
    out_right: *mut u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
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
        *out_left = bits_from_ptr(Box::into_raw(left) as *mut u8);
        *out_right = bits_from_ptr(Box::into_raw(right) as *mut u8);
        0
    })
}

#[no_mangle]
pub extern "C" fn molt_ws_pair_obj(capacity_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[no_mangle]
pub extern "C" fn molt_ws_new_with_hooks(
    send_hook: usize,
    recv_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    crate::with_gil_entry!(_py, {
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

type WsConnectHook = extern "C" fn(*const u8, usize) -> *mut u8;
type DbHostHook = extern "C" fn(*const u8, usize, *mut u64, u64) -> i32;

static WS_CONNECT_HOOK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static DB_QUERY_HOOK: AtomicUsize = AtomicUsize::new(0);
static DB_EXEC_HOOK: AtomicUsize = AtomicUsize::new(0);

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
    #[cfg(not(target_arch = "wasm32"))]
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

#[cfg(target_arch = "wasm32")]
extern "C" fn ws_send_host_hook(ctx: *mut u8, data_ptr: *const u8, len: usize) -> i64 {
    if ctx.is_null() {
        return pending_bits_i64();
    }
    let handle = unsafe { *(ctx as *mut i64) };
    let rc = unsafe { crate::molt_ws_send_host(handle, data_ptr, len as u64) };
    if rc == 0 {
        0
    } else if rc == -(libc::EWOULDBLOCK as i32) || rc == -(libc::EAGAIN as i32) {
        pending_bits_i64()
    } else {
        // Treat send errors as a closed socket for now.
        MoltObject::none().bits() as i64
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
        if rc == -(libc::EWOULDBLOCK as i32) || rc == -(libc::EAGAIN as i32) {
            return pending_bits_i64();
        }
        if rc == -(libc::ENOMEM as i32) && out_len as usize > buf.len() {
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
    if handle <= 0 {
        None
    } else {
        Some(handle)
    }
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn ws_is_native(ws: &MoltWebSocket) -> bool {
    ws.is_native && !ws.hook_ctx.is_null()
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn ws_is_would_block(err: &tungstenite::Error) -> bool {
    matches!(
        err,
        tungstenite::Error::Io(io_err)
            if io_err.kind() == std::io::ErrorKind::WouldBlock
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn ws_flush_pending_pong(ws: &mut NativeWebSocket) -> Result<(), Box<tungstenite::Error>> {
    if let Some(payload) = ws.pending_pong.take() {
        match ws.socket.send(Message::Pong(payload.clone())) {
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

#[cfg(not(target_arch = "wasm32"))]
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
    match guard.socket.send(Message::Binary(payload.to_vec())) {
        Ok(_) => 0,
        Err(err) if ws_is_would_block(&err) => pending_bits_i64(),
        Err(_) => {
            guard.closed = true;
            MoltObject::none().bits() as i64
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
                    guard.pending_pong = Some(payload);
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn tls_is_would_block(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::WouldBlock
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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
        tls_stream_send_native_hook as usize,
        tls_stream_recv_native_hook as usize,
        tls_stream_close_native_hook as usize,
        ctx_ptr,
    );
    if stream_ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ctx_ptr as *mut Mutex<NativeTlsStream>));
        }
    }
    stream_ptr
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
extern "C" fn tls_stream_close_native_hook(ctx: *mut u8) {
    if ctx.is_null() {
        return;
    }
    let mutex = unsafe { Box::from_raw(ctx as *mut Mutex<NativeTlsStream>) };
    let mut state = mutex.lock().unwrap();
    state.closed = true;
    let _ = tls_endpoint_shutdown(&mut state.stream);
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(all(not(target_arch = "wasm32"), unix))]
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

#[cfg(not(target_arch = "wasm32"))]
fn tls_client_connect_native(host: &str, port: u16, server_name: &str) -> *mut u8 {
    let tcp = {
        let _release = GilReleaseGuard::new();
        match TcpStream::connect((host, port)) {
            Ok(value) => value,
            Err(_) => return std::ptr::null_mut(),
        }
    };
    tls_client_wrap_stream_native(tcp, server_name)
}

#[cfg(not(target_arch = "wasm32"))]
fn tls_server_config_cache() -> &'static Mutex<HashMap<(String, String), Arc<ServerConfig>>> {
    static CACHE: OnceLock<Mutex<HashMap<(String, String), Arc<ServerConfig>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(all(not(target_arch = "wasm32"), unix))]
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

#[cfg(all(not(target_arch = "wasm32"), unix))]
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

#[cfg(all(not(target_arch = "wasm32"), unix))]
fn tls_server_from_fd_native(raw_fd: i64, certfile: &str, keyfile: &str) -> *mut u8 {
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

#[cfg(all(not(target_arch = "wasm32"), windows))]
fn tls_server_from_fd_native(raw_fd: i64, certfile: &str, keyfile: &str) -> *mut u8 {
    if raw_fd < 0 {
        return std::ptr::null_mut();
    }
    let Ok(config) = tls_server_load_config(certfile, keyfile) else {
        return std::ptr::null_mut();
    };
    let tcp = unsafe { TcpStream::from_raw_socket(raw_fd as RawSocket) };
    tls_server_wrap_stream_native(tcp, config)
}

#[cfg(not(target_arch = "wasm32"))]
fn tls_server_ssl_attr_string(
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

#[cfg(all(not(target_arch = "wasm32"), unix))]
fn tls_client_from_fd_native(raw_fd: i64, server_name: &str) -> *mut u8 {
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

#[cfg(all(not(target_arch = "wasm32"), windows))]
fn tls_client_from_fd_native(raw_fd: i64, server_name: &str) -> *mut u8 {
    if raw_fd < 0 {
        return std::ptr::null_mut();
    }
    let tcp = unsafe { TcpStream::from_raw_socket(raw_fd as RawSocket) };
    tls_client_wrap_stream_native(tcp, server_name)
}

#[cfg(not(target_arch = "wasm32"))]
fn ws_connect_native(url_ptr: *const u8, url_len: usize) -> *mut u8 {
    if url_ptr.is_null() && url_len != 0 {
        return std::ptr::null_mut();
    }
    let url_bytes = unsafe { std::slice::from_raw_parts(url_ptr, url_len) };
    let url_str = match std::str::from_utf8(url_bytes) {
        Ok(val) => val,
        Err(_) => return std::ptr::null_mut(),
    };
    let url = match Url::parse(url_str) {
        Ok(val) => val,
        Err(_) => return std::ptr::null_mut(),
    };
    if url.scheme() != "ws" && url.scheme() != "wss" {
        return std::ptr::null_mut();
    }
    let (mut socket, _) = {
        let _release = GilReleaseGuard::new();
        match connect(url) {
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
        ws_send_native_hook as usize,
        ws_recv_native_hook as usize,
        ws_close_native_hook as usize,
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

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn ws_wait_release(_py: &PyToken<'_>, future_ptr: *mut u8) {
    if future_ptr.is_null() {
        return;
    }
    let header = unsafe { header_from_obj_ptr(future_ptr) };
    let payload_bytes = unsafe {
        (*header)
            .size
            .saturating_sub(std::mem::size_of::<crate::MoltHeader>())
    };
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
    let payload_bytes = unsafe {
        (*header)
            .size
            .saturating_sub(std::mem::size_of::<crate::MoltHeader>())
    };
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

#[no_mangle]
pub extern "C" fn molt_ws_set_connect_hook(ptr: usize) {
    crate::with_gil_entry!(_py, {
        WS_CONNECT_HOOK.store(ptr, AtomicOrdering::Release);
    })
}

#[no_mangle]
pub extern "C" fn molt_db_set_query_hook(ptr: usize) {
    crate::with_gil_entry!(_py, {
        DB_QUERY_HOOK.store(ptr, AtomicOrdering::Release);
    })
}

#[no_mangle]
pub extern "C" fn molt_db_set_exec_hook(ptr: usize) {
    crate::with_gil_entry!(_py, {
        DB_EXEC_HOOK.store(ptr, AtomicOrdering::Release);
    })
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

fn load_trusted() -> bool {
    match std::env::var("MOLT_TRUSTED") {
        Ok(val) => matches!(
            val.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

pub(crate) fn is_trusted(_py: &PyToken<'_>) -> bool {
    *runtime_state(_py).trusted.get_or_init(load_trusted)
}

pub(crate) fn has_capability(_py: &PyToken<'_>, name: &str) -> bool {
    if is_trusted(_py) {
        return true;
    }
    let caps = runtime_state(_py)
        .capabilities
        .get_or_init(load_capabilities);
    caps.contains(name)
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_tls_client_connect_new(
    host_bits: u64,
    port_bits: u64,
    server_hostname_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
        let stream_ptr = tls_client_connect_native(&host, port_raw as u16, &server_name);
        if stream_ptr.is_null() {
            return raise_exception::<u64>(_py, "OSError", "asyncio TLS client connection failed");
        }
        bits_from_ptr(stream_ptr)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_asyncio_tls_client_connect_new(
    _host_bits: u64,
    _port_bits: u64,
    _server_hostname_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "asyncio TLS client transport is unavailable on wasm",
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_tls_client_from_fd_new(
    fd_bits: u64,
    server_hostname_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
        bits_from_ptr(stream_ptr)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_asyncio_tls_client_from_fd_new(
    _fd_bits: u64,
    _server_hostname_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "asyncio TLS start_tls upgrade is unavailable on wasm",
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_asyncio_tls_server_payload(ssl_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        static CERTFILE_NAME: AtomicU64 = AtomicU64::new(0);
        static KEYFILE_NAME: AtomicU64 = AtomicU64::new(0);
        let certfile = match tls_server_ssl_attr_string(_py, ssl_bits, &CERTFILE_NAME, b"certfile")
        {
            Ok(Some(value)) => value,
            Ok(None) => {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "server ssl context is missing certfile",
                )
            }
            Err(bits) => return bits,
        };
        let keyfile = match tls_server_ssl_attr_string(_py, ssl_bits, &KEYFILE_NAME, b"keyfile") {
            Ok(Some(value)) => value,
            Ok(None) => {
                return raise_exception::<u64>(
                    _py,
                    "RuntimeError",
                    "server ssl context is missing keyfile",
                )
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
#[no_mangle]
pub extern "C" fn molt_asyncio_tls_server_payload(_ssl_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "asyncio TLS server payload is unavailable on wasm",
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub unsafe extern "C" fn molt_asyncio_tls_server_from_fd_new(
    fd_bits: u64,
    certfile_bits: u64,
    keyfile_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
        bits_from_ptr(stream_ptr)
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn molt_asyncio_tls_server_from_fd_new(
    _fd_bits: u64,
    _certfile_bits: u64,
    _keyfile_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(
            _py,
            "RuntimeError",
            "asyncio TLS server transport is unavailable on wasm",
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must ensure `url_ptr` is valid for `url_len` bytes and `out` is writable.
pub unsafe extern "C" fn molt_ws_connect(
    url_ptr: *const u8,
    url_len_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        if out.is_null() {
            return 2;
        }
        let url_len = usize_from_bits(url_len_bits);
        if url_ptr.is_null() && url_len != 0 {
            return 1;
        }
        if !has_capability(_py, "websocket.connect") {
            return 6;
        }
        let hook_ptr = WS_CONNECT_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            let ws_ptr = ws_connect_native(url_ptr, url_len);
            if ws_ptr.is_null() {
                return 7;
            }
            *out = bits_from_ptr(ws_ptr);
            return 0;
        }
        let hook: WsConnectHook = std::mem::transmute(hook_ptr);
        let ws_ptr = hook(url_ptr, url_len);
        if ws_ptr.is_null() {
            return 7;
        }
        *out = bits_from_ptr(ws_ptr);
        0
    })
}

#[no_mangle]
pub extern "C" fn molt_ws_connect_obj(url_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
#[no_mangle]
/// # Safety
/// Caller must ensure `url_ptr` is valid for `url_len` bytes and `out` is writable.
pub unsafe extern "C" fn molt_ws_connect(
    url_ptr: *const u8,
    url_len_bits: u64,
    out: *mut u64,
) -> i32 {
    crate::with_gil_entry!(_py, {
        if out.is_null() {
            return 2;
        }
        let url_len = usize_from_bits(url_len_bits);
        if url_ptr.is_null() && url_len != 0 {
            return 1;
        }
        if !has_capability(_py, "websocket.connect") {
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
            ws_send_host_hook as usize,
            ws_recv_host_hook as usize,
            ws_close_host_hook as usize,
            ctx_ptr,
        );
        if ws_ptr.is_null() {
            let _ = unsafe { crate::molt_ws_close_host(handle) };
            unsafe {
                drop(Box::from_raw(ctx_ptr as *mut i64));
            }
            return 7;
        }
        *out = bits_from_ptr(ws_ptr);
        0
    })
}

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
pub extern "C" fn molt_ws_wait_new(ws_bits: u64, events_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
#[no_mangle]
pub extern "C" fn molt_ws_wait_new(ws_bits: u64, events_bits: u64, timeout_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[cfg(not(target_arch = "wasm32"))]
#[no_mangle]
/// # Safety
/// Caller must pass a valid ws-wait awaitable object bits value.
pub unsafe extern "C" fn molt_ws_wait(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<crate::MoltHeader>());
        let payload_len = payload_bytes / std::mem::size_of::<u64>();
        if payload_len < 2 {
            return raise_exception::<i64>(_py, "TypeError", "ws wait payload too small");
        }
        let payload_ptr = obj_ptr as *mut u64;
        let ws_bits = *payload_ptr;
        let events_bits = *payload_ptr.add(1);
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return raise_exception::<i64>(_py, "TypeError", "invalid websocket");
        }
        let ws = &*(ws_ptr as *mut MoltWebSocket);
        if ws.closed.load(AtomicOrdering::Relaxed) {
            let mask = IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return MoltObject::from_int(mask as i64).bits() as i64;
        }
        let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
        if events == 0 {
            return raise_exception::<i64>(_py, "ValueError", "events must be non-zero");
        }
        if (*header).state == 0 {
            let mut timeout: Option<f64> = None;
            if payload_len >= 3 {
                let timeout_bits = *payload_ptr.add(2);
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
                    dec_ref_bits(_py, *payload_ptr.add(2));
                    *payload_ptr.add(2) = deadline_bits;
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
            (*header).state = 1;
            return pending_bits_i64();
        }
        if let Some(mask) = runtime_state(_py).io_poller().take_ready(obj_ptr) {
            let res_bits = MoltObject::from_int(mask as i64).bits();
            return res_bits as i64;
        }
        if payload_len >= 3 {
            let deadline_obj = obj_from_bits(*payload_ptr.add(2));
            if let Some(deadline) = to_f64(deadline_obj) {
                if deadline.is_finite() && monotonic_now_secs(_py) >= deadline {
                    runtime_state(_py).io_poller().cancel_waiter(obj_ptr);
                    return raise_exception::<i64>(_py, "TimeoutError", "timed out");
                }
            }
        }
        pending_bits_i64()
    })
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub unsafe extern "C" fn molt_ws_wait(obj_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj_ptr = ptr_from_bits(obj_bits);
        if obj_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let header = header_from_obj_ptr(obj_ptr);
        let payload_bytes = (*header)
            .size
            .saturating_sub(std::mem::size_of::<crate::MoltHeader>());
        let payload_len = payload_bytes / std::mem::size_of::<u64>();
        if payload_len < 2 {
            return raise_exception::<i64>(_py, "TypeError", "ws wait payload too small");
        }
        let payload_ptr = obj_ptr as *mut u64;
        let ws_bits = *payload_ptr;
        let events_bits = *payload_ptr.add(1);
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return raise_exception::<i64>(_py, "TypeError", "invalid websocket");
        }
        let ws = &*(ws_ptr as *mut MoltWebSocket);
        if ws.closed.load(AtomicOrdering::Relaxed) {
            let mask = IO_EVENT_ERROR | IO_EVENT_READ | IO_EVENT_WRITE;
            return MoltObject::from_int(mask as i64).bits() as i64;
        }
        let events = to_i64(obj_from_bits(events_bits)).unwrap_or(0) as u32;
        if events == 0 {
            return raise_exception::<i64>(_py, "ValueError", "events must be non-zero");
        }
        if (*header).state == 0 {
            let mut timeout: Option<f64> = None;
            if payload_len >= 3 {
                let timeout_bits = *payload_ptr.add(2);
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
                    dec_ref_bits(_py, *payload_ptr.add(2));
                    *payload_ptr.add(2) = deadline_bits;
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
            (*header).state = 1;
            return pending_bits_i64();
        }
        if let Some(mask) = runtime_state(_py).io_poller().take_ready(obj_ptr) {
            let res_bits = MoltObject::from_int(mask as i64).bits();
            return res_bits as i64;
        }
        if payload_len >= 3 {
            let deadline_obj = obj_from_bits(*payload_ptr.add(2));
            if let Some(deadline) = to_f64(deadline_obj) {
                if deadline.is_finite() && monotonic_now_secs(_py) >= deadline {
                    runtime_state(_py).io_poller().cancel_waiter(obj_ptr);
                    return raise_exception::<i64>(_py, "TimeoutError", "timed out");
                }
            }
        }
        pending_bits_i64()
    })
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
    crate::with_gil_entry!(_py, {
        db_query_impl(_py, req_ptr, len_bits, out, token_bits)
    })
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
    crate::with_gil_entry!(_py, {
        db_exec_impl(_py, req_ptr, len_bits, out, token_bits)
    })
}

fn db_query_impl(
    _py: &PyToken<'_>,
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
    if !has_capability(_py, "db.read") {
        return 6;
    }
    cancel_tokens(_py);
    let token_id = match token_id_from_bits(token_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => return 1,
    };
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { molt_db_query_host(req_ptr as u64, len_bits, out as u64, token_id) };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let hook_ptr = DB_QUERY_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            return 7;
        }
        let hook: DbHostHook = unsafe { std::mem::transmute(hook_ptr) };
        hook(req_ptr, len, out, token_id)
    }
}

fn db_exec_impl(
    _py: &PyToken<'_>,
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
    if !has_capability(_py, "db.write") {
        return 6;
    }
    cancel_tokens(_py);
    let token_id = match token_id_from_bits(token_bits) {
        Some(0) => current_token_id(),
        Some(id) => id,
        None => return 1,
    };
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { molt_db_exec_host(req_ptr as u64, len_bits, out as u64, token_id) };
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let hook_ptr = DB_EXEC_HOOK.load(AtomicOrdering::Acquire);
        if hook_ptr == 0 {
            return 7;
        }
        let hook: DbHostHook = unsafe { std::mem::transmute(hook_ptr) };
        hook(req_ptr, len, out, token_id)
    }
}

fn db_error(_py: &PyToken<'_>, op: &str, code: i32, cap: &str) -> u64 {
    match code {
        1 => raise_exception::<_>(_py, "ValueError", &format!("{op} invalid input")),
        2 => raise_exception::<_>(_py, "RuntimeError", &format!("{op} output pointer invalid")),
        6 => raise_exception::<_>(_py, "PermissionError", &format!("missing {cap} capability")),
        7 => raise_exception::<_>(_py, "RuntimeError", &format!("{op} host unavailable")),
        _ => raise_exception::<_>(_py, "RuntimeError", &format!("{op} failed")),
    }
}

#[no_mangle]
pub extern "C" fn molt_db_query_obj(req_bits: u64, token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let send_data = match send_data_from_bits(req_bits) {
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
        let mut out = 0u64;
        let rc = db_query_impl(_py, data_ptr, data_len as u64, &mut out, token_bits);
        if rc != 0 {
            return db_error(_py, "db_query", rc, "db.read");
        }
        out
    })
}

#[no_mangle]
pub extern "C" fn molt_db_exec_obj(req_bits: u64, token_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let send_data = match send_data_from_bits(req_bits) {
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
        let mut out = 0u64;
        let rc = db_exec_impl(_py, data_ptr, data_len as u64, &mut out, token_bits);
        if rc != 0 {
            return db_error(_py, "db_exec", rc, "db.write");
        }
        out
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is valid; `data_ptr` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_ws_send(ws_bits: u64, data_ptr: *const u8, len_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let ws_ptr = ptr_from_bits(ws_bits);
        let len = usize_from_bits(len_bits);
        if ws_ptr.is_null() || (data_ptr.is_null() && len != 0) {
            return pending_bits_i64();
        }
        let ws = &*(ws_ptr as *mut MoltWebSocket);
        if ws.send_hook.is_some() && ws.closed.load(AtomicOrdering::Relaxed) {
            return MoltObject::none().bits() as i64;
        }
        if let Some(hook) = ws.send_hook {
            return hook(ws.hook_ctx, data_ptr, len);
        }
        let bytes = std::slice::from_raw_parts(data_ptr, len).to_vec();
        match ws.sender.try_send(bytes) {
            Ok(_) => 0,
            Err(_) => pending_bits_i64(),
        }
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is valid; `data_bits` must be bytes-like.
pub unsafe extern "C" fn molt_ws_send_obj(ws_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
        molt_ws_send(ws_bits, data_ptr, data_len as u64) as u64
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_recv(ws_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        let ws = &*(ws_ptr as *mut MoltWebSocket);
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

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_close(ws_bits: u64) {
    crate::with_gil_entry!(_py, {
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return;
        }
        let ws = &*(ws_ptr as *mut MoltWebSocket);
        if ws.closed.swap(true, AtomicOrdering::AcqRel) {
            return;
        }
        if let Some(hook) = ws.close_hook {
            hook(ws.hook_ctx);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            runtime_state(_py)
                .io_poller()
                .deregister_socket(_py, ws_ptr);
        }
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_drop(stream_bits: u64) {
    crate::with_gil_entry!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        let stream = &*(stream_ptr as *mut MoltStream);
        if stream.refs.fetch_sub(1, AtomicOrdering::AcqRel) > 1 {
            return;
        }
        if !stream.closed.load(AtomicOrdering::Relaxed) {
            if let Some(hook) = stream.close_hook {
                hook(stream.hook_ctx);
            }
        }
        release_ptr(stream_ptr);
        drop(Box::from_raw(stream_ptr as *mut MoltStream));
    })
}

#[no_mangle]
/// # Safety
/// Caller must ensure `ws_bits` is a valid websocket pointer.
pub unsafe extern "C" fn molt_ws_drop(ws_bits: u64) {
    crate::with_gil_entry!(_py, {
        let ws_ptr = ptr_from_bits(ws_bits);
        if ws_ptr.is_null() {
            return;
        }
        ws_ref_dec(_py, ws_ptr as *mut MoltWebSocket);
    })
}
