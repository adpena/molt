#[cfg(test)]
use crossbeam_channel::TrySendError;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Condvar, Mutex};

use super::super::sockets::{SendData, send_data_from_bits};
use crate::{
    MoltObject, PyToken, alloc_bytes, obj_from_bits, opaque_handle_bits, pending_bits_i64,
    ptr_from_bits, raise_exception, release_ptr, to_i64, usize_from_bits,
};

pub struct MoltStream {
    pub sender: Sender<Vec<u8>>,
    pub receiver: Receiver<Vec<u8>>,
    pub closed: AtomicBool,
    pub refs: AtomicUsize,
    max_queued_bytes: usize,
    queue_budget: Mutex<StreamQueueBudget>,
    queue_budget_cvar: Condvar,
    pub send_hook: Option<extern "C" fn(*mut u8, *const u8, usize) -> i64>,
    pub recv_hook: Option<extern "C" fn(*mut u8) -> i64>,
    pub close_hook: Option<extern "C" fn(*mut u8)>,
    pub hook_ctx: *mut u8,
}

struct MoltStreamReader {
    stream_bits: u64,
    buffer: Vec<u8>,
    buffer_start: usize,
    scan_cursor: usize,
    eof: bool,
}

#[derive(Debug, Default)]
struct StreamQueueBudget {
    queued_bytes: usize,
    peak_queued_bytes: usize,
    blocked_sends: usize,
}

const STREAM_DEFAULT_MAX_QUEUED_BYTES: usize = 16 * 1024 * 1024;
const STREAM_MIN_MAX_QUEUED_BYTES: usize = 64 * 1024;
const STREAM_MAX_MAX_QUEUED_BYTES: usize = 1024 * 1024 * 1024;
const STREAM_MAX_QUEUED_BYTES_ENV: &str = "MOLT_STREAM_MAX_QUEUED_BYTES";

fn parse_positive_usize_env(key: &str) -> Option<usize> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

fn bounded_stream_max_queued_bytes(value: usize) -> usize {
    value.clamp(STREAM_MIN_MAX_QUEUED_BYTES, STREAM_MAX_MAX_QUEUED_BYTES)
}

pub(crate) fn default_stream_max_queued_bytes() -> usize {
    parse_positive_usize_env(STREAM_MAX_QUEUED_BYTES_ENV)
        .map(bounded_stream_max_queued_bytes)
        .unwrap_or(STREAM_DEFAULT_MAX_QUEUED_BYTES)
}

fn new_stream_box(
    capacity: usize,
    max_queued_bytes: usize,
    send_hook: Option<extern "C" fn(*mut u8, *const u8, usize) -> i64>,
    recv_hook: Option<extern "C" fn(*mut u8) -> i64>,
    close_hook: Option<extern "C" fn(*mut u8)>,
    hook_ctx: *mut u8,
) -> Box<MoltStream> {
    let (sender, receiver) = bytes_channel(capacity);
    Box::new(MoltStream {
        sender,
        receiver,
        closed: AtomicBool::new(false),
        refs: AtomicUsize::new(1),
        max_queued_bytes: bounded_stream_max_queued_bytes(max_queued_bytes),
        queue_budget: Mutex::new(StreamQueueBudget::default()),
        queue_budget_cvar: Condvar::new(),
        send_hook,
        recv_hook,
        close_hook,
        hook_ctx,
    })
}

pub(crate) fn stream_new_with_byte_budget(capacity: usize, max_queued_bytes: usize) -> u64 {
    opaque_handle_bits(Box::into_raw(new_stream_box(
        capacity,
        max_queued_bytes,
        None,
        None,
        None,
        std::ptr::null_mut(),
    )) as *mut u8)
}

fn stream_can_reserve_bytes(current: usize, len: usize, max_queued_bytes: usize) -> bool {
    if len == 0 {
        return true;
    }
    match current.checked_add(len) {
        Some(next) => next <= max_queued_bytes || current == 0,
        None => false,
    }
}

fn stream_try_reserve_queued_bytes(stream: &MoltStream, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    if stream.closed.load(AtomicOrdering::Acquire) {
        return false;
    }
    let mut budget = stream.queue_budget.lock().unwrap();
    if !stream_can_reserve_bytes(budget.queued_bytes, len, stream.max_queued_bytes) {
        budget.blocked_sends = budget.blocked_sends.saturating_add(1);
        return false;
    }
    budget.queued_bytes = budget.queued_bytes.saturating_add(len);
    budget.peak_queued_bytes = budget.peak_queued_bytes.max(budget.queued_bytes);
    true
}

fn stream_reserve_queued_bytes_blocking(stream: &MoltStream, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let mut budget = stream.queue_budget.lock().unwrap();
    loop {
        if stream.closed.load(AtomicOrdering::Acquire) {
            return false;
        }
        if stream_can_reserve_bytes(budget.queued_bytes, len, stream.max_queued_bytes) {
            budget.queued_bytes = budget.queued_bytes.saturating_add(len);
            budget.peak_queued_bytes = budget.peak_queued_bytes.max(budget.queued_bytes);
            return true;
        }
        budget.blocked_sends = budget.blocked_sends.saturating_add(1);
        budget = stream.queue_budget_cvar.wait(budget).unwrap();
    }
}

pub(crate) fn stream_release_queued_bytes(stream: &MoltStream, len: usize) {
    if len == 0 {
        return;
    }
    let mut budget = stream.queue_budget.lock().unwrap();
    budget.queued_bytes = budget.queued_bytes.saturating_sub(len);
    drop(budget);
    stream.queue_budget_cvar.notify_all();
}

pub(crate) fn stream_close_local(stream: &MoltStream) {
    stream.closed.store(true, AtomicOrdering::Release);
    stream.queue_budget_cvar.notify_all();
}

#[cfg(test)]
fn stream_enqueue_bytes(stream: &MoltStream, bytes: Vec<u8>) -> Result<(), Vec<u8>> {
    let len = bytes.len();
    if !stream_try_reserve_queued_bytes(stream, len) {
        return Err(bytes);
    }
    match stream.sender.try_send(bytes) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(bytes)) | Err(TrySendError::Disconnected(bytes)) => {
            stream_release_queued_bytes(stream, len);
            Err(bytes)
        }
    }
}

pub(crate) fn stream_enqueue_bytes_blocking(stream: &MoltStream, bytes: Vec<u8>) -> bool {
    let len = bytes.len();
    if !stream_reserve_queued_bytes_blocking(stream, len) {
        return false;
    }
    match stream.sender.send(bytes) {
        Ok(()) => true,
        Err(_) => {
            stream_release_queued_bytes(stream, len);
            false
        }
    }
}

pub(crate) fn bytes_channel(capacity: usize) -> (Sender<Vec<u8>>, Receiver<Vec<u8>>) {
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

const STREAM_READER_COMPACT_PREFIX_MIN: usize = 4096;

unsafe fn stream_reader_pull(
    _py: &PyToken<'_>,
    reader: &mut MoltStreamReader,
) -> Result<ReaderPull, u64> {
    if reader.eof {
        return Ok(ReaderPull::Eof);
    }
    let pending = pending_bits_i64() as u64;
    // SAFETY: `reader.stream_bits` is created by `molt_stream_reader_new` from a live stream.
    let recv_bits = unsafe { molt_stream_recv(reader.stream_bits) as u64 };
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

#[inline]
fn stream_reader_unread_len(reader: &MoltStreamReader) -> usize {
    reader.buffer.len().saturating_sub(reader.buffer_start)
}

#[inline]
fn stream_reader_unread_is_empty(reader: &MoltStreamReader) -> bool {
    stream_reader_unread_len(reader) == 0
}

#[inline]
fn stream_reader_unread_slice(reader: &MoltStreamReader) -> &[u8] {
    &reader.buffer[reader.buffer_start..]
}

fn stream_reader_maybe_compact(reader: &mut MoltStreamReader) {
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
    if consumed < STREAM_READER_COMPACT_PREFIX_MIN
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

fn stream_reader_find_newline(reader: &mut MoltStreamReader) -> Option<usize> {
    let unread_start = reader.buffer_start;
    let unread_end = reader.buffer.len();
    let search_start = reader.scan_cursor.max(unread_start).min(unread_end);
    if search_start == unread_end {
        return None;
    }
    match reader.buffer[search_start..unread_end]
        .iter()
        .position(|&b| b == b'\n')
    {
        Some(rel_idx) => {
            let idx = search_start + rel_idx;
            reader.scan_cursor = idx.saturating_add(1);
            Some(idx - unread_start)
        }
        None => {
            reader.scan_cursor = unread_end;
            None
        }
    }
}

fn stream_reader_take(_py: &PyToken<'_>, reader: &mut MoltStreamReader, count: usize) -> u64 {
    let n = count.min(stream_reader_unread_len(reader));
    let unread = stream_reader_unread_slice(reader);
    let ptr = alloc_bytes(_py, &unread[..n]);
    if ptr.is_null() {
        reader.scan_cursor = reader.buffer_start;
        return MoltObject::none().bits();
    }
    reader.buffer_start += n;
    reader.scan_cursor = reader.scan_cursor.max(reader.buffer_start);
    stream_reader_maybe_compact(reader);
    MoltObject::from_ptr(ptr).bits()
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid stream handle from `molt_stream_new`/`molt_stream_clone`.
pub unsafe extern "C" fn molt_stream_reader_new(stream_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return MoltObject::none().bits();
        }
        // SAFETY: `stream_bits` is validated by caller contract and checked for null above.
        let cloned_bits = unsafe { molt_stream_clone(stream_bits) };
        if obj_from_bits(cloned_bits).is_none() {
            return MoltObject::none().bits();
        }
        let reader = Box::new(MoltStreamReader {
            stream_bits: cloned_bits,
            buffer: Vec::new(),
            buffer_start: 0,
            scan_cursor: 0,
            eof: false,
        });
        opaque_handle_bits(Box::into_raw(reader) as *mut u8)
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid stream reader handle from `molt_stream_reader_new`.
pub unsafe extern "C" fn molt_stream_reader_drop(reader_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return;
        }
        // SAFETY: ownership of the boxed reader is transferred for drop.
        let reader = unsafe { Box::from_raw(reader_ptr as *mut MoltStreamReader) };
        // SAFETY: stream handle was retained when this reader was created.
        unsafe { molt_stream_drop(reader.stream_bits) };
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid stream reader handle from `molt_stream_reader_new`.
pub unsafe extern "C" fn molt_stream_reader_at_eof(reader_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::from_bool(true).bits();
        }
        // SAFETY: caller contract guarantees a valid reader handle.
        let reader = unsafe { &*(reader_ptr as *mut MoltStreamReader) };
        MoltObject::from_bool(reader.eof && stream_reader_unread_is_empty(reader)).bits()
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid stream reader handle from `molt_stream_reader_new`.
pub unsafe extern "C" fn molt_stream_reader_read(reader_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::none().bits();
        }
        // SAFETY: caller contract guarantees a valid reader handle.
        let reader = unsafe { &mut *(reader_ptr as *mut MoltStreamReader) };
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
                    return stream_reader_take(_py, reader, stream_reader_unread_len(reader));
                }
                // SAFETY: `reader` owns a retained stream handle.
                match unsafe { stream_reader_pull(_py, reader) } {
                    Ok(ReaderPull::Pending) => return pending_bits_i64() as u64,
                    Ok(ReaderPull::Eof) | Ok(ReaderPull::Data) => {}
                    Err(bits) => return bits,
                }
            }
        }
        if !stream_reader_unread_is_empty(reader) {
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
            // SAFETY: `reader` owns a retained stream handle.
            match unsafe { stream_reader_pull(_py, reader) } {
                Ok(ReaderPull::Pending) => return pending_bits_i64() as u64,
                Ok(ReaderPull::Eof) => {
                    if stream_reader_unread_is_empty(reader) {
                        let ptr = alloc_bytes(_py, &[]);
                        if ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(ptr).bits();
                    }
                    return stream_reader_take(_py, reader, n as usize);
                }
                Ok(ReaderPull::Data) => {
                    if !stream_reader_unread_is_empty(reader) {
                        return stream_reader_take(_py, reader, n as usize);
                    }
                }
                Err(bits) => return bits,
            }
        }
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must pass a valid stream reader handle from `molt_stream_reader_new`.
pub unsafe extern "C" fn molt_stream_reader_readline(reader_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let reader_ptr = ptr_from_bits(reader_bits);
        if reader_ptr.is_null() {
            return MoltObject::none().bits();
        }
        // SAFETY: caller contract guarantees a valid reader handle.
        let reader = unsafe { &mut *(reader_ptr as *mut MoltStreamReader) };
        loop {
            if let Some(idx) = stream_reader_find_newline(reader) {
                return stream_reader_take(_py, reader, idx + 1);
            }
            if reader.eof {
                return stream_reader_take(_py, reader, stream_reader_unread_len(reader));
            }
            // SAFETY: `reader` owns a retained stream handle.
            match unsafe { stream_reader_pull(_py, reader) } {
                Ok(ReaderPull::Pending) => return pending_bits_i64() as u64,
                Ok(ReaderPull::Eof) | Ok(ReaderPull::Data) => {}
                Err(bits) => return bits,
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stream_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let capacity = usize_from_bits(capacity_bits);
        stream_new_with_byte_budget(capacity, default_stream_max_queued_bytes())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stream_new_with_io_hooks(
    send_hook: usize,
    recv_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    crate::with_gil_entry_nopanic!(_py, {
        // SAFETY (all three hook transmutes below): The usize values are function
        // pointers provided by the host embedder (Cloudflare Worker JS glue or native
        // embedder) via the C FFI boundary. The caller guarantees:
        //   1. Non-zero values are valid pointers to functions with the declared
        //      extern "C" signature.
        //   2. The pointed-to functions remain valid for the lifetime of this
        //      MoltStream (the stream holds no ownership; the host must outlive it).
        //   3. hook_ctx passed alongside is the correct opaque context pointer that
        //      each hook expects as its first argument.
        // Violation (bad pointer or signature mismatch) causes UB at hook call site.
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
        let stream = new_stream_box(
            0,
            default_stream_max_queued_bytes(),
            send_hook,
            recv_hook,
            close_hook,
            hook_ctx,
        );
        Box::into_raw(stream) as *mut u8
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stream_new_with_hooks(
    send_hook: usize,
    close_hook: usize,
    hook_ctx: *mut u8,
) -> *mut u8 {
    molt_stream_new_with_io_hooks(send_hook, 0, close_hook, hook_ctx)
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_clone(stream_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return MoltObject::none().bits();
        }
        // SAFETY: caller contract guarantees `stream_bits` points to a live stream.
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        stream.refs.fetch_add(1, AtomicOrdering::AcqRel);
        stream_bits
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_ptr` must be readable for `len_bits` bytes.
pub unsafe extern "C" fn molt_stream_send(
    stream_bits: u64,
    data_ptr: *const u8,
    len_bits: u64,
) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        let len = usize_from_bits(len_bits);
        if stream_ptr.is_null() || (data_ptr.is_null() && len != 0) {
            return pending_bits_i64();
        }
        // SAFETY: caller contract guarantees `stream_bits` points to a live stream.
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        if let Some(hook) = stream.send_hook {
            return hook(stream.hook_ctx, data_ptr, len);
        }
        if !stream_try_reserve_queued_bytes(stream, len) {
            return pending_bits_i64();
        }
        // SAFETY: caller contract guarantees `data_ptr` is readable for `len` bytes.
        let source = unsafe { std::slice::from_raw_parts(data_ptr, len) };
        let mut bytes = Vec::new();
        if bytes.try_reserve_exact(len).is_err() {
            stream_release_queued_bytes(stream, len);
            return raise_exception::<i64>(
                _py,
                "MemoryError",
                "stream send buffer allocation failed",
            );
        }
        bytes.extend_from_slice(source);
        match stream.sender.try_send(bytes) {
            Ok(_) => 0,
            Err(_) => {
                stream_release_queued_bytes(stream, len);
                pending_bits_i64()
            }
        }
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `stream_bits` is valid; `data_bits` must be bytes-like.
pub unsafe extern "C" fn molt_stream_send_obj(stream_bits: u64, data_bits: u64) -> u64 {
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
        unsafe { molt_stream_send(stream_bits, data_ptr, data_len as u64) as u64 }
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_recv(stream_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return MoltObject::none().bits() as i64;
        }
        // SAFETY: caller contract guarantees `stream_bits` points to a live stream.
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        if let Some(hook) = stream.recv_hook {
            return hook(stream.hook_ctx);
        }
        match stream.receiver.try_recv() {
            Ok(bytes) => {
                stream_release_queued_bytes(stream, bytes.len());
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
                        // SAFETY: these are extern "C" host poll functions; safe to call from unsafe fn context.
                        let _ = unsafe { crate::molt_db_host_poll() };
                        let _ = unsafe { crate::molt_process_host_poll() };
                        if let Ok(bytes) = stream.receiver.try_recv() {
                            stream_release_queued_bytes(stream, bytes.len());
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

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_close(stream_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        // SAFETY: caller contract guarantees `stream_bits` points to a live stream.
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        if let Some(hook) = stream.close_hook {
            hook(stream.hook_ctx);
        }
        stream_close_local(stream);
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `stream_bits` is a valid stream pointer.
pub unsafe extern "C" fn molt_stream_drop(stream_bits: u64) {
    crate::with_gil_entry_nopanic!(_py, {
        let stream_ptr = ptr_from_bits(stream_bits);
        if stream_ptr.is_null() {
            return;
        }
        // SAFETY: caller contract guarantees `stream_bits` points to a live stream.
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        if stream.refs.fetch_sub(1, AtomicOrdering::AcqRel) > 1 {
            return;
        }
        if !stream.closed.load(AtomicOrdering::Relaxed)
            && let Some(hook) = stream.close_hook
        {
            hook(stream.hook_ctx);
        }
        stream_close_local(stream);
        release_ptr(stream_ptr);
        // SAFETY: this is the final ref-counted owner.
        unsafe { drop(Box::from_raw(stream_ptr as *mut MoltStream)) };
    })
}
#[cfg(test)]
mod stream_tests {
    use super::{
        MoltStream, STREAM_MIN_MAX_QUEUED_BYTES, molt_stream_drop, molt_stream_recv,
        molt_stream_send, stream_enqueue_bytes_blocking, stream_new_with_byte_budget,
        stream_release_queued_bytes,
    };
    use crate::{MoltObject, dec_ref_bits, obj_from_bits, pending_bits_i64, ptr_from_bits};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn stream_byte_budget_returns_pending_until_recv_releases_bytes() {
        let _guard = crate::test_mutex_guard();
        let stream_bits = stream_new_with_byte_budget(0, STREAM_MIN_MAX_QUEUED_BYTES);
        let full = vec![1u8; STREAM_MIN_MAX_QUEUED_BYTES];
        let one = [2u8; 1];

        let first = unsafe { molt_stream_send(stream_bits, full.as_ptr(), full.len() as u64) };
        assert_eq!(first, 0);
        let blocked = unsafe { molt_stream_send(stream_bits, one.as_ptr(), one.len() as u64) };
        assert_eq!(blocked, pending_bits_i64());

        crate::with_gil_entry_nopanic!(_py, {
            let recv_bits = unsafe { molt_stream_recv(stream_bits) as u64 };
            assert!(!obj_from_bits(recv_bits).is_none());
            dec_ref_bits(_py, recv_bits);
        });

        let unblocked = unsafe { molt_stream_send(stream_bits, one.as_ptr(), one.len() as u64) };
        assert_eq!(unblocked, 0);
        unsafe { molt_stream_drop(stream_bits) };
    }

    #[test]
    fn stream_blocking_enqueue_waits_for_byte_budget_release() {
        let stream_bits = stream_new_with_byte_budget(0, STREAM_MIN_MAX_QUEUED_BYTES);
        let stream_ptr = ptr_from_bits(stream_bits);
        assert!(!stream_ptr.is_null());
        let stream = unsafe { &*(stream_ptr as *mut MoltStream) };
        assert!(
            super::stream_enqueue_bytes(stream, vec![1u8; STREAM_MIN_MAX_QUEUED_BYTES]).is_ok()
        );

        let (started_tx, started_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker_bits = stream_bits;
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            let ptr = ptr_from_bits(worker_bits);
            let stream = unsafe { &*(ptr as *mut MoltStream) };
            let ok = stream_enqueue_bytes_blocking(stream, vec![2u8; 1]);
            done_tx.send(ok).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(50)).is_err());

        let first = stream.receiver.try_recv().unwrap();
        stream_release_queued_bytes(stream, first.len());
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        worker.join().unwrap();
        let second = stream.receiver.try_recv().unwrap();
        assert_eq!(second, vec![2u8; 1]);
        stream_release_queued_bytes(stream, second.len());
        unsafe { molt_stream_drop(stream_bits) };
    }

    #[test]
    fn stream_oversized_single_message_can_make_forward_progress() {
        let stream_bits = stream_new_with_byte_budget(0, STREAM_MIN_MAX_QUEUED_BYTES);
        let oversized = vec![3u8; STREAM_MIN_MAX_QUEUED_BYTES + 1];
        let sent =
            unsafe { molt_stream_send(stream_bits, oversized.as_ptr(), oversized.len() as u64) };
        assert_eq!(sent, 0);
        crate::with_gil_entry_nopanic!(_py, {
            let recv_bits = unsafe { molt_stream_recv(stream_bits) as u64 };
            assert_ne!(recv_bits, pending_bits_i64() as u64);
            assert_ne!(recv_bits, MoltObject::none().bits());
            dec_ref_bits(_py, recv_bits);
        });
        unsafe { molt_stream_drop(stream_bits) };
    }
}
