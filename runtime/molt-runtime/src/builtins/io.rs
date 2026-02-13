use crate::PyToken;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::object::{
    MoltFileBackend, MoltMemoryBackend, MoltTextBackend, NEWLINE_KIND_CR, NEWLINE_KIND_CRLF,
    NEWLINE_KIND_LF,
};
use crate::*;
use getrandom::fill as getrandom_fill;
use num_bigint::{BigInt, Sign};
use num_traits::{ToPrimitive, Zero};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Seek, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const DEFAULT_BUFFER_SIZE: i64 = 8192;
static HANDLE_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
static SYS_STDIN_HANDLE_BITS: AtomicU64 = AtomicU64::new(0);
static SYS_STDOUT_HANDLE_BITS: AtomicU64 = AtomicU64::new(0);
static SYS_STDERR_HANDLE_BITS: AtomicU64 = AtomicU64::new(0);

macro_rules! file_handle_require_attached {
    ($py:expr, $handle:expr) => {
        if $handle.detached {
            return raise_exception::<_>($py, "ValueError", file_handle_detached_message($handle));
        }
    };
}

fn resolve_file_handle_ptr(_py: &PyToken<'_>, obj_bits: u64) -> Result<*mut MoltFileHandle, u64> {
    let obj = obj_from_bits(obj_bits);
    if let Some(ptr) = obj.as_ptr() {
        if unsafe { object_type_id(ptr) } == TYPE_ID_FILE_HANDLE {
            let handle_ptr = unsafe { file_handle_ptr(ptr) };
            if handle_ptr.is_null() {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "file handle missing",
                ));
            }
            return Ok(handle_ptr);
        }
    }
    let name_bits = intern_static_name(_py, &HANDLE_ATTR_NAME, b"_handle");
    let missing = missing_bits(_py);
    let attr_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !is_missing_bits(_py, attr_bits) {
        let mut resolved = None;
        if let Some(attr_ptr) = obj_from_bits(attr_bits).as_ptr() {
            if unsafe { object_type_id(attr_ptr) } == TYPE_ID_FILE_HANDLE {
                let handle_ptr = unsafe { file_handle_ptr(attr_ptr) };
                if !handle_ptr.is_null() {
                    resolved = Some(handle_ptr);
                }
            }
        }
        dec_ref_bits(_py, attr_bits);
        if let Some(handle_ptr) = resolved {
            return Ok(handle_ptr);
        }
    }
    Err(raise_exception::<_>(
        _py,
        "TypeError",
        "expected file handle",
    ))
}

#[allow(clippy::too_many_arguments)]
fn alloc_file_handle_with_state(
    _py: &PyToken<'_>,
    state: Arc<MoltFileState>,
    readable: bool,
    writable: bool,
    text: bool,
    closefd: bool,
    owns_fd: bool,
    line_buffering: bool,
    write_through: bool,
    buffer_size: i64,
    class_bits: u64,
    name_bits: u64,
    mode: String,
    encoding: Option<String>,
    encoding_original: Option<String>,
    errors: Option<String>,
    newline: Option<String>,
    buffer_bits: u64,
    mem_bits: u64,
) -> *mut u8 {
    let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut MoltFileHandle>();
    let ptr = alloc_object(_py, total, TYPE_ID_FILE_HANDLE);
    if ptr.is_null() {
        return ptr;
    }
    let handle = Box::new(MoltFileHandle {
        state,
        readable,
        writable,
        text,
        closefd,
        owns_fd,
        closed: false,
        detached: false,
        line_buffering,
        write_through,
        buffer_size,
        class_bits,
        name_bits,
        mode,
        encoding,
        encoding_original,
        text_bom_seen: false,
        text_bom_written: false,
        errors,
        newline,
        buffer_bits,
        pending_byte: None,
        text_pending_bytes: Vec::new(),
        text_pending_text: Vec::new(),
        mem_bits,
        read_buf: Vec::new(),
        read_pos: 0,
        write_buf: Vec::new(),
        newlines_mask: 0,
        newlines_len: 0,
        newlines_seen: [0; 3],
    });
    if name_bits != 0 {
        inc_ref_bits(_py, name_bits);
    }
    if buffer_bits != 0 {
        inc_ref_bits(_py, buffer_bits);
    }
    if mem_bits != 0 {
        inc_ref_bits(_py, mem_bits);
    }
    let handle_ptr = Box::into_raw(handle);
    unsafe {
        *(ptr as *mut *mut MoltFileHandle) = handle_ptr;
    }
    ptr
}

fn file_handle_close_ptr(ptr: *mut u8) -> bool {
    if ptr.is_null() {
        return false;
    }
    unsafe {
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return false;
        }
        let handle = &mut *handle_ptr;
        if handle.closed {
            return false;
        }
        handle.closed = true;
        if !handle.owns_fd {
            return false;
        }
        let backend_state = Arc::clone(&handle.state);
        let mut guard = backend_state.backend.lock().unwrap();
        let had_backend = guard.take().is_some();
        #[cfg(windows)]
        if had_backend {
            let mut fd_guard = backend_state.crt_fd.lock().unwrap();
            if let Some(fd) = fd_guard.take() {
                unsafe {
                    libc::_close(fd as libc::c_int);
                }
            }
        }
        had_backend
    }
}

unsafe fn memory_backend_vec_from_bits(
    _py: &PyToken<'_>,
    mem_bits: u64,
) -> Result<&'static mut Vec<u8>, u64> {
    unsafe {
        if mem_bits == 0 || obj_from_bits(mem_bits).is_none() {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend missing",
            ));
        }
        let Some(ptr) = obj_from_bits(mem_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend missing",
            ));
        };
        if object_type_id(ptr) != TYPE_ID_BYTEARRAY {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend is not bytearray",
            ));
        }
        Ok(bytearray_vec(ptr))
    }
}

unsafe fn memory_backend_vec(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
) -> Result<&'static mut Vec<u8>, u64> {
    unsafe { memory_backend_vec_from_bits(_py, handle.mem_bits) }
}

unsafe fn memory_backend_vec_ref_from_bits(
    _py: &PyToken<'_>,
    mem_bits: u64,
) -> Result<&'static Vec<u8>, u64> {
    unsafe {
        if mem_bits == 0 || obj_from_bits(mem_bits).is_none() {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend missing",
            ));
        }
        let Some(ptr) = obj_from_bits(mem_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend missing",
            ));
        };
        if object_type_id(ptr) != TYPE_ID_BYTEARRAY {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "memory backend is not bytearray",
            ));
        }
        Ok(bytearray_vec_ref(ptr))
    }
}

unsafe fn memory_backend_vec_ref(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
) -> Result<&'static Vec<u8>, u64> {
    unsafe { memory_backend_vec_ref_from_bits(_py, handle.mem_bits) }
}

unsafe fn collect_bytes_like(_py: &PyToken<'_>, bits: u64) -> Result<Vec<u8>, u64> {
    unsafe {
        let obj = obj_from_bits(bits);
        if obj.is_none() {
            return Ok(Vec::new());
        }
        let Some(ptr) = obj.as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "a bytes-like object is required",
            ));
        };
        match object_type_id(ptr) {
            TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                let len = bytes_len(ptr);
                let raw = std::slice::from_raw_parts(bytes_data(ptr), len);
                Ok(raw.to_vec())
            }
            TYPE_ID_MEMORYVIEW => {
                if let Some(out) = memoryview_collect_bytes(ptr) {
                    Ok(out)
                } else {
                    Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        "a bytes-like object is required",
                    ))
                }
            }
            _ => Err(raise_exception::<_>(
                _py,
                "TypeError",
                "a bytes-like object is required",
            )),
        }
    }
}

unsafe fn backend_read_bytes(
    _py: &PyToken<'_>,
    mem_bits: u64,
    backend: &mut MoltFileBackend,
    buf: &mut [u8],
) -> Result<usize, u64> {
    unsafe {
        match backend {
            MoltFileBackend::File(file) => match file.read(buf) {
                Ok(n) => Ok(n),
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "read failed")),
            },
            MoltFileBackend::Memory(mem) => {
                let data = memory_backend_vec_ref_from_bits(_py, mem_bits)?;
                if mem.pos >= data.len() {
                    return Ok(0);
                }
                let available = data.len().saturating_sub(mem.pos);
                let n = available.min(buf.len());
                buf[..n].copy_from_slice(&data[mem.pos..mem.pos + n]);
                mem.pos = mem.pos.saturating_add(n);
                Ok(n)
            }
            MoltFileBackend::Text(_) => Err(raise_exception::<_>(
                _py,
                "UnsupportedOperation",
                "binary read on text backend",
            )),
        }
    }
}

unsafe fn backend_write_bytes(
    _py: &PyToken<'_>,
    mem_bits: u64,
    backend: &mut MoltFileBackend,
    bytes: &[u8],
) -> Result<usize, u64> {
    unsafe {
        match backend {
            MoltFileBackend::File(file) => match file.write(bytes) {
                Ok(n) => Ok(n),
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "write failed")),
            },
            MoltFileBackend::Memory(mem) => {
                let data = memory_backend_vec_from_bits(_py, mem_bits)?;
                if mem.pos > data.len() {
                    data.resize(mem.pos, 0);
                }
                let end = mem.pos.saturating_add(bytes.len());
                if end > data.len() {
                    data.resize(end, 0);
                }
                data[mem.pos..end].copy_from_slice(bytes);
                mem.pos = end;
                Ok(bytes.len())
            }
            MoltFileBackend::Text(_) => Err(raise_exception::<_>(
                _py,
                "UnsupportedOperation",
                "binary write on text backend",
            )),
        }
    }
}

unsafe fn backend_flush(_py: &PyToken<'_>, backend: &mut MoltFileBackend) -> Result<(), u64> {
    match backend {
        MoltFileBackend::File(file) => match file.flush() {
            Ok(()) => Ok(()),
            Err(_) => Err(raise_exception::<_>(_py, "OSError", "flush failed")),
        },
        MoltFileBackend::Memory(_) | MoltFileBackend::Text(_) => Ok(()),
    }
}

unsafe fn backend_seek(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    from: std::io::SeekFrom,
) -> Result<u64, u64> {
    unsafe {
        match backend {
            MoltFileBackend::File(file) => match file.seek(from) {
                Ok(pos) => Ok(pos),
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "seek failed")),
            },
            MoltFileBackend::Memory(mem) => {
                let pos = match from {
                    std::io::SeekFrom::Start(pos) => pos as i64,
                    std::io::SeekFrom::Current(delta) => mem.pos as i64 + delta,
                    std::io::SeekFrom::End(delta) => {
                        let len = memory_backend_vec_ref(_py, handle)?.len() as i64;
                        len + delta
                    }
                };
                if pos < 0 {
                    return Err(raise_exception::<_>(
                        _py,
                        "ValueError",
                        "negative seek position",
                    ));
                }
                mem.pos = pos as usize;
                Ok(mem.pos as u64)
            }
            MoltFileBackend::Text(_) => Err(raise_exception::<_>(
                _py,
                "UnsupportedOperation",
                "seek on text backend",
            )),
        }
    }
}

unsafe fn backend_tell(_py: &PyToken<'_>, backend: &mut MoltFileBackend) -> Result<u64, u64> {
    match backend {
        MoltFileBackend::File(file) => match file.stream_position() {
            Ok(pos) => Ok(pos),
            Err(_) => Err(raise_exception::<_>(_py, "OSError", "tell failed")),
        },
        MoltFileBackend::Memory(mem) => Ok(mem.pos as u64),
        MoltFileBackend::Text(_) => Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "tell on text backend",
        )),
    }
}

unsafe fn backend_truncate(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: u64,
) -> Result<(), u64> {
    unsafe {
        match backend {
            MoltFileBackend::File(file) => match file.set_len(size) {
                Ok(()) => Ok(()),
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "truncate failed")),
            },
            MoltFileBackend::Memory(mem) => {
                let data = memory_backend_vec(_py, handle)?;
                let size_usize = size as usize;
                if size_usize < data.len() {
                    data.truncate(size_usize);
                } else if size_usize > data.len() {
                    data.resize(size_usize, 0);
                }
                if mem.pos > data.len() {
                    mem.pos = data.len();
                }
                Ok(())
            }
            MoltFileBackend::Text(_) => Err(raise_exception::<_>(
                _py,
                "UnsupportedOperation",
                "truncate on text backend",
            )),
        }
    }
}

fn clear_read_buffer(handle: &mut MoltFileHandle) {
    handle.read_buf.clear();
    handle.read_pos = 0;
}

fn prepend_read_buffer(handle: &mut MoltFileHandle, prefix: &[u8]) {
    if prefix.is_empty() {
        return;
    }
    let unread = if handle.read_pos < handle.read_buf.len() {
        handle.read_buf[handle.read_pos..].to_vec()
    } else {
        Vec::new()
    };
    let mut buf = Vec::with_capacity(prefix.len() + unread.len());
    buf.extend_from_slice(prefix);
    buf.extend_from_slice(&unread);
    handle.read_buf = buf;
    handle.read_pos = 0;
}

fn clear_write_buffer(handle: &mut MoltFileHandle) {
    handle.write_buf.clear();
}

fn unread_bytes(handle: &MoltFileHandle) -> usize {
    handle.read_buf.len().saturating_sub(handle.read_pos)
}

unsafe fn rewind_read_buffer(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
) -> Result<(), u64> {
    let unread = unread_bytes(handle);
    if unread == 0 {
        clear_read_buffer(handle);
        return Ok(());
    }
    match backend {
        MoltFileBackend::File(file) => {
            let offset = -(unread as i64);
            match file.seek(std::io::SeekFrom::Current(offset)) {
                Ok(_) => {
                    clear_read_buffer(handle);
                    Ok(())
                }
                Err(_) => Err(raise_exception::<_>(_py, "OSError", "seek failed")),
            }
        }
        MoltFileBackend::Memory(mem) => {
            mem.pos = mem.pos.saturating_sub(unread);
            clear_read_buffer(handle);
            Ok(())
        }
        MoltFileBackend::Text(_) => {
            clear_read_buffer(handle);
            Ok(())
        }
    }
}

unsafe fn flush_write_buffer(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
) -> Result<(), u64> {
    unsafe {
        if handle.write_buf.is_empty() {
            return Ok(());
        }
        let bytes = handle.write_buf.clone();
        handle.write_buf.clear();
        let mut written = 0usize;
        while written < bytes.len() {
            let n = backend_write_bytes(_py, handle.mem_bits, backend, &bytes[written..])?;
            if n == 0 {
                return Err(raise_exception::<_>(_py, "OSError", "write failed"));
            }
            written += n;
        }
        backend_flush(_py, backend)?;
        Ok(())
    }
}

unsafe fn buffered_read_bytes(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Result<(Vec<u8>, bool), u64> {
    unsafe {
        if handle.buffer_size == 0 {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
            let mut at_eof = false;
            match size {
                Some(0) => return Ok((Vec::new(), false)),
                Some(mut remaining) => {
                    while remaining > 0 {
                        let to_read = remaining.min(tmp.len());
                        let n =
                            backend_read_bytes(_py, handle.mem_bits, backend, &mut tmp[..to_read])?;
                        if n == 0 {
                            at_eof = true;
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        remaining -= n;
                    }
                }
                None => loop {
                    let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut tmp)?;
                    if n == 0 {
                        at_eof = true;
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                },
            }
            return Ok((buf, at_eof));
        }

        let mut out: Vec<u8> = Vec::new();
        let mut at_eof = false;
        let mut remaining = size;
        if let Some(rem) = remaining {
            if rem == 0 {
                return Ok((out, false));
            }
        }
        if !handle.write_buf.is_empty() {
            flush_write_buffer(_py, handle, backend)?;
        }

        while remaining.map(|r| r > 0).unwrap_or(true) {
            let avail = unread_bytes(handle);
            if avail > 0 {
                let take = remaining.map(|r| r.min(avail)).unwrap_or(avail);
                let start = handle.read_pos;
                let end = start + take;
                out.extend_from_slice(&handle.read_buf[start..end]);
                handle.read_pos = end;
                if handle.read_pos >= handle.read_buf.len() {
                    clear_read_buffer(handle);
                }
                if let Some(rem) = remaining {
                    let new_rem = rem.saturating_sub(take);
                    remaining = Some(new_rem);
                    if new_rem == 0 {
                        break;
                    }
                }
                continue;
            }
            let buf_size = handle.buffer_size.max(1) as usize;
            let mut buf = std::mem::take(&mut handle.read_buf);
            buf.resize(buf_size, 0);
            let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf)?;
            if n == 0 {
                at_eof = true;
                handle.read_buf = buf;
                clear_read_buffer(handle);
                break;
            }
            buf.truncate(n);
            handle.read_buf = buf;
            handle.read_pos = 0;
        }
        Ok((out, at_eof))
    }
}

unsafe fn buffered_read_into(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    buf: &mut [u8],
) -> Result<usize, u64> {
    unsafe {
        if buf.is_empty() {
            return Ok(0);
        }
        if !handle.write_buf.is_empty() {
            flush_write_buffer(_py, handle, backend)?;
        }
        let mut written = 0usize;
        let avail = unread_bytes(handle);
        if avail > 0 {
            let take = avail.min(buf.len());
            let start = handle.read_pos;
            let end = start + take;
            buf[..take].copy_from_slice(&handle.read_buf[start..end]);
            handle.read_pos = end;
            if handle.read_pos >= handle.read_buf.len() {
                clear_read_buffer(handle);
            }
            written += take;
        }
        if written >= buf.len() {
            return Ok(written);
        }
        let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf[written..])?;
        written += n;
        Ok(written)
    }
}

unsafe fn file_read1_bytes(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Result<(Vec<u8>, bool), u64> {
    unsafe {
        if let Some(0) = size {
            return Ok((Vec::new(), false));
        }
        if !handle.write_buf.is_empty() {
            flush_write_buffer(_py, handle, backend)?;
        }
        let avail = unread_bytes(handle);
        if avail > 0 {
            let take = size.unwrap_or(avail).min(avail);
            let start = handle.read_pos;
            let end = start + take;
            let out = handle.read_buf[start..end].to_vec();
            handle.read_pos = end;
            if handle.read_pos >= handle.read_buf.len() {
                clear_read_buffer(handle);
            }
            return Ok((out, false));
        }
        let read_size = size.unwrap_or({
            if handle.buffer_size > 0 {
                handle.buffer_size as usize
            } else {
                8192
            }
        });
        if read_size == 0 {
            return Ok((Vec::new(), false));
        }
        let mut buf = vec![0u8; read_size];
        let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf)?;
        buf.truncate(n);
        Ok((buf, n == 0))
    }
}

unsafe fn handle_read_byte(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
) -> Result<Option<u8>, u64> {
    unsafe {
        if let Some(pending) = handle.pending_byte.take() {
            return Ok(Some(pending));
        }
        if !handle.text_pending_bytes.is_empty() {
            let byte = handle.text_pending_bytes.remove(0);
            return Ok(Some(byte));
        }
        if handle.buffer_size > 0 {
            if unread_bytes(handle) == 0 {
                let buf_size = handle.buffer_size.max(1) as usize;
                let mut buf = std::mem::take(&mut handle.read_buf);
                buf.resize(buf_size, 0);
                let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf)?;
                if n == 0 {
                    handle.read_buf = buf;
                    clear_read_buffer(handle);
                    return Ok(None);
                }
                buf.truncate(n);
                handle.read_buf = buf;
                handle.read_pos = 0;
            }
            if unread_bytes(handle) == 0 {
                return Ok(None);
            }
            let byte = handle.read_buf[handle.read_pos];
            handle.read_pos += 1;
            if handle.read_pos >= handle.read_buf.len() {
                clear_read_buffer(handle);
            }
            Ok(Some(byte))
        } else {
            let mut buf = [0u8; 1];
            let n = backend_read_bytes(_py, handle.mem_bits, backend, &mut buf)?;
            if n == 0 { Ok(None) } else { Ok(Some(buf[0])) }
        }
    }
}

pub(crate) unsafe fn file_handle_enter(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    unsafe {
        let bits = MoltObject::from_ptr(ptr).bits();
        let handle_ptr = file_handle_ptr(ptr);
        if !handle_ptr.is_null() {
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            handle.closed = false;
        }
        inc_ref_bits(_py, bits);
        bits
    }
}

pub(crate) unsafe fn file_handle_exit(_py: &PyToken<'_>, ptr: *mut u8, _exc_bits: u64) -> u64 {
    unsafe {
        let handle_ptr = file_handle_ptr(ptr);
        if !handle_ptr.is_null() {
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            let backend_state = Arc::clone(&handle.state);
            {
                let mut guard = backend_state.backend.lock().unwrap();
                if let Some(backend) = guard.as_mut() {
                    if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                        return bits;
                    }
                }
            }
            file_handle_close_ptr(ptr);
            handle.closed = true;
        }
        MoltObject::from_bool(false).bits()
    }
}

#[allow(dead_code)]
pub(crate) fn close_payload(_py: &PyToken<'_>, payload_bits: u64) {
    let payload = obj_from_bits(payload_bits);
    let Some(ptr) = payload.as_ptr() else {
        return raise_exception::<()>(_py, "AttributeError", "object has no attribute 'close'");
    };
    unsafe {
        if object_type_id(ptr) == TYPE_ID_FILE_HANDLE {
            let handle_ptr = file_handle_ptr(ptr);
            if !handle_ptr.is_null() {
                let handle = &*handle_ptr;
                file_handle_require_attached!(_py, handle);
            }
            file_handle_close_ptr(ptr);
            return;
        }
    }
    let Some(close_name_bits) = attr_name_bits_from_bytes(_py, b"close") else {
        return;
    };
    let missing = missing_bits(_py);
    let close_bits = molt_getattr_builtin(payload_bits, close_name_bits, missing);
    dec_ref_bits(_py, close_name_bits);
    if exception_pending(_py) {
        return;
    }
    let out = unsafe { call_callable0(_py, close_bits) };
    dec_ref_bits(_py, close_bits);
    if !obj_from_bits(out).is_none() {
        dec_ref_bits(_py, out);
    }
}

struct FileMode {
    options: OpenOptions,
    readable: bool,
    writable: bool,
    append: bool,
    create: bool,
    truncate: bool,
    create_new: bool,
    text: bool,
}

fn parse_file_mode(mode: &str) -> Result<FileMode, String> {
    let mut kind: Option<char> = None;
    let mut kind_dup = false;
    let mut read = false;
    let mut write = false;
    let mut append = false;
    let mut truncate = false;
    let mut create = false;
    let mut create_new = false;
    let mut saw_plus = 0usize;
    let mut saw_text = false;
    let mut saw_binary = false;

    for ch in mode.chars() {
        match ch {
            'r' | 'w' | 'a' | 'x' => {
                if let Some(prev) = kind {
                    if prev == ch {
                        kind_dup = true;
                    } else {
                        return Err(
                            "must have exactly one of create/read/write/append mode".to_string()
                        );
                    }
                } else {
                    kind = Some(ch);
                }
                match ch {
                    'r' => read = true,
                    'w' => {
                        write = true;
                        truncate = true;
                        create = true;
                    }
                    'a' => {
                        write = true;
                        append = true;
                        create = true;
                    }
                    'x' => {
                        write = true;
                        create = true;
                        create_new = true;
                    }
                    _ => {}
                }
            }
            '+' => {
                saw_plus += 1;
                read = true;
                write = true;
            }
            'b' => saw_binary = true,
            't' => saw_text = true,
            _ => return Err(format!("invalid mode: '{mode}'")),
        }
    }

    if saw_binary && saw_text {
        return Err("can't have text and binary mode at once".to_string());
    }
    if saw_plus > 1 {
        return Err(format!("invalid mode: '{mode}'"));
    }
    if kind.is_none() {
        return Err(
            "Must have exactly one of create/read/write/append mode and at most one plus"
                .to_string(),
        );
    }
    if kind_dup {
        return Err(format!("invalid mode: '{mode}'"));
    }

    let mut options = OpenOptions::new();
    options
        .read(read)
        .write(write)
        .append(append)
        .truncate(truncate)
        .create(create);
    if create_new {
        options.create_new(true);
    }
    Ok(FileMode {
        options,
        readable: read,
        writable: write,
        append,
        create,
        truncate,
        create_new,
        text: !saw_binary,
    })
}

fn open_arg_type(_py: &PyToken<'_>, bits: u64, name: &str, allow_none: bool) -> Option<String> {
    let obj = obj_from_bits(bits);
    if allow_none && obj.is_none() {
        return None;
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Some(text);
    }
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = if allow_none {
        format!("open() argument '{name}' must be str or None, not {type_name}")
    } else {
        format!("open() argument '{name}' must be str, not {type_name}")
    };
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn open_arg_newline(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("open() argument 'newline' must be str or None, not {type_name}");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    match text.as_str() {
        "" | "\n" | "\r" | "\r\n" => Some(text),
        _ => {
            let msg = format!("illegal newline value: {text}");
            raise_exception::<_>(_py, "ValueError", &msg)
        }
    }
}

fn reconfigure_arg_type(_py: &PyToken<'_>, bits: u64, name: &str) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    if let Some(text) = string_obj_to_owned(obj) {
        return Some(text);
    }
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("reconfigure() argument '{name}' must be str or None, not {type_name}");
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn reconfigure_arg_newline(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return None;
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("reconfigure() argument 'newline' must be str or None, not {type_name}");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    match text.as_str() {
        "" | "\n" | "\r" | "\r\n" => Some(text),
        _ => {
            let msg = format!("illegal newline value: {text}");
            raise_exception::<_>(_py, "ValueError", &msg)
        }
    }
}

fn open_arg_encoding(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    open_arg_type(_py, bits, "encoding", true)
}

fn open_arg_errors(_py: &PyToken<'_>, bits: u64) -> Option<String> {
    open_arg_type(_py, bits, "errors", true)
}

fn file_mode_to_flags(mode: &FileMode) -> i32 {
    #[allow(clippy::useless_conversion)]
    let mut flags = 0;
    if mode.readable && !mode.writable {
        flags |= libc::O_RDONLY;
    } else if mode.writable && !mode.readable {
        flags |= libc::O_WRONLY;
    } else {
        flags |= libc::O_RDWR;
    }
    if mode.append {
        flags |= libc::O_APPEND;
    }
    if mode.create {
        flags |= libc::O_CREAT;
    }
    if mode.truncate {
        flags |= libc::O_TRUNC;
    }
    if mode.create_new {
        flags |= libc::O_EXCL;
    }
    flags
}

#[cfg(unix)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_fd(fd as i32) })
}

#[cfg(windows)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
    if handle == -1 {
        return None;
    }
    let dup = duplicate_handle(handle as *mut std::ffi::c_void)?;
    Some(unsafe { std::fs::File::from_raw_handle(dup as *mut _) })
}

#[cfg(target_arch = "wasm32")]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::wasi::io::FromRawFd;
    if fd < 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_fd(fd as std::os::wasi::io::RawFd) })
}

#[cfg(all(not(any(unix, windows)), not(target_arch = "wasm32")))]
fn file_from_fd(_fd: i64) -> Option<std::fs::File> {
    None
}

#[cfg(unix)]
fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd as libc::c_int) };
    if duped < 0 { None } else { Some(duped as i64) }
}

#[cfg(windows)]
fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::_dup(fd as libc::c_int) };
    if duped < 0 { None } else { Some(duped as i64) }
}

#[cfg(not(any(unix, windows)))]
fn dup_fd(_fd: i64) -> Option<i64> {
    None
}

#[cfg(windows)]
const FILE_TYPE_CHAR: u32 = 0x0002;

#[cfg(windows)]
const HANDLE_FLAG_INHERIT: u32 = 0x00000001;

#[cfg(windows)]
const DUPLICATE_SAME_ACCESS: u32 = 0x00000002;

#[cfg(windows)]
const FILE_NAME_NORMALIZED: u32 = 0x0000000;

#[cfg(windows)]
const VOLUME_NAME_DOS: u32 = 0x0000000;

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
    fn GetFileType(hFile: *mut std::ffi::c_void) -> u32;
    fn GetConsoleMode(hConsoleHandle: *mut std::ffi::c_void, lpMode: *mut u32) -> i32;
    fn GetHandleInformation(hObject: *mut std::ffi::c_void, lpdwFlags: *mut u32) -> i32;
    fn SetHandleInformation(hObject: *mut std::ffi::c_void, dwMask: u32, dwFlags: u32) -> i32;
    fn DuplicateHandle(
        hSourceProcessHandle: *mut std::ffi::c_void,
        hSourceHandle: *mut std::ffi::c_void,
        hTargetProcessHandle: *mut std::ffi::c_void,
        lpTargetHandle: *mut *mut std::ffi::c_void,
        dwDesiredAccess: u32,
        bInheritHandle: i32,
        dwOptions: u32,
    ) -> i32;
    fn GetFinalPathNameByHandleW(
        hFile: *mut std::ffi::c_void,
        lpszFilePath: *mut u16,
        cchFilePath: u32,
        dwFlags: u32,
    ) -> u32;
    fn CloseHandle(hObject: *mut std::ffi::c_void) -> i32;
}

#[cfg(windows)]
fn windows_handle_isatty(handle: *mut std::ffi::c_void) -> bool {
    if handle.is_null() || handle as isize == -1 {
        return false;
    }
    unsafe {
        let file_type = GetFileType(handle);
        if file_type != FILE_TYPE_CHAR {
            return false;
        }
        let mut mode: u32 = 0;
        GetConsoleMode(handle, &mut mode as *mut u32) != 0
    }
}

#[cfg(windows)]
fn duplicate_handle(handle: *mut std::ffi::c_void) -> Option<*mut std::ffi::c_void> {
    if handle.is_null() || handle as isize == -1 {
        return None;
    }
    unsafe {
        let process = GetCurrentProcess();
        let mut dup: *mut std::ffi::c_void = std::ptr::null_mut();
        let ok = DuplicateHandle(
            process,
            handle,
            process,
            &mut dup as *mut *mut std::ffi::c_void,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        );
        if ok == 0 { None } else { Some(dup) }
    }
}

#[cfg(windows)]
fn windows_path_from_handle(handle: *mut std::ffi::c_void) -> Option<String> {
    if handle.is_null() || handle as isize == -1 {
        return None;
    }
    let flags = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS;
    let needed = unsafe { GetFinalPathNameByHandleW(handle, std::ptr::null_mut(), 0, flags) };
    if needed == 0 {
        return None;
    }
    let mut buf: Vec<u16> = vec![0u16; needed as usize + 1];
    let wrote =
        unsafe { GetFinalPathNameByHandleW(handle, buf.as_mut_ptr(), buf.len() as u32, flags) };
    if wrote == 0 {
        return None;
    }
    let mut text = String::from_utf16_lossy(&buf[..wrote as usize]);
    if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        text = format!("\\\\{rest}");
    } else if let Some(rest) = text.strip_prefix("\\\\?\\") {
        text = rest.to_string();
    }
    Some(text)
}

#[cfg(windows)]
fn windows_crt_fd_from_handle(
    handle: *mut std::ffi::c_void,
    readable: bool,
    writable: bool,
) -> Option<i64> {
    let dup = duplicate_handle(handle)?;
    let mut flags = libc::O_BINARY;
    if readable && writable {
        flags |= libc::O_RDWR;
    } else if readable {
        flags |= libc::O_RDONLY;
    } else {
        flags |= libc::O_WRONLY;
    }
    let fd = unsafe { libc::_open_osfhandle(dup as isize, flags) };
    if fd < 0 {
        unsafe {
            CloseHandle(dup);
        }
        None
    } else {
        Some(fd as i64)
    }
}

fn stdio_isatty(fd: i64) -> bool {
    #[cfg(unix)]
    {
        if fd < 0 {
            return false;
        }
        unsafe { libc::isatty(fd as libc::c_int) == 1 }
    }
    #[cfg(windows)]
    {
        if fd < 0 {
            return false;
        }
        unsafe { libc::isatty(fd as libc::c_int) == 1 }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = fd;
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathFlavor {
    Str,
    Bytes,
}

fn path_from_bits_with_flavor(
    _py: &PyToken<'_>,
    file_bits: u64,
) -> Result<(std::path::PathBuf, PathFlavor), String> {
    let obj = obj_from_bits(file_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        return Ok((std::path::PathBuf::from(text), PathFlavor::Str));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    let path = std::ffi::OsString::from_vec(bytes.to_vec());
                    return Ok((std::path::PathBuf::from(path), PathFlavor::Bytes));
                }
                #[cfg(windows)]
                {
                    let path = std::str::from_utf8(bytes)
                        .map_err(|_| "open path bytes must be utf-8".to_string())?;
                    return Ok((std::path::PathBuf::from(path), PathFlavor::Bytes));
                }
            }
            let fspath_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.fspath_name, b"__fspath__");
            if let Some(call_bits) = attr_lookup_ptr(_py, ptr, fspath_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return Err("open failed".to_string());
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(text) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(_py, res_bits);
                    return Ok((std::path::PathBuf::from(text), PathFlavor::Str));
                }
                if let Some(res_ptr) = res_obj.as_ptr() {
                    if object_type_id(res_ptr) == TYPE_ID_BYTES {
                        let len = bytes_len(res_ptr);
                        let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                        #[cfg(unix)]
                        {
                            use std::os::unix::ffi::OsStringExt;
                            let path = std::ffi::OsString::from_vec(bytes.to_vec());
                            dec_ref_bits(_py, res_bits);
                            return Ok((std::path::PathBuf::from(path), PathFlavor::Bytes));
                        }
                        #[cfg(windows)]
                        {
                            let path = std::str::from_utf8(bytes)
                                .map_err(|_| "open path bytes must be utf-8".to_string())?;
                            dec_ref_bits(_py, res_bits);
                            return Ok((std::path::PathBuf::from(path), PathFlavor::Bytes));
                        }
                    }
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                dec_ref_bits(_py, res_bits);
                let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
                return Err(format!(
                    "expected {obj_type}.__fspath__() to return str or bytes, not {res_type}"
                ));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
    Err(format!(
        "expected str, bytes or os.PathLike object, not {obj_type}"
    ))
}

fn fspath_bits_with_flavor(_py: &PyToken<'_>, file_bits: u64) -> Result<(u64, PathFlavor), u64> {
    let obj = obj_from_bits(file_bits);
    let Some(ptr) = obj.as_ptr() else {
        let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
        let msg = format!("expected str, bytes or os.PathLike object, not {obj_type}");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };

    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_STRING {
            inc_ref_bits(_py, file_bits);
            return Ok((file_bits, PathFlavor::Str));
        }
        if type_id == TYPE_ID_BYTES {
            inc_ref_bits(_py, file_bits);
            return Ok((file_bits, PathFlavor::Bytes));
        }
        let fspath_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.fspath_name, b"__fspath__");
        if let Some(call_bits) = attr_lookup_ptr(_py, ptr, fspath_name_bits) {
            let res_bits = call_callable0(_py, call_bits);
            dec_ref_bits(_py, call_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            let res_obj = obj_from_bits(res_bits);
            if let Some(res_ptr) = res_obj.as_ptr() {
                let res_type_id = object_type_id(res_ptr);
                if res_type_id == TYPE_ID_STRING {
                    return Ok((res_bits, PathFlavor::Str));
                }
                if res_type_id == TYPE_ID_BYTES {
                    return Ok((res_bits, PathFlavor::Bytes));
                }
            }
            let res_type = class_name_for_error(type_of_bits(_py, res_bits));
            dec_ref_bits(_py, res_bits);
            let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
            let msg =
                format!("expected {obj_type}.__fspath__() to return str or bytes, not {res_type}");
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
        }
    }

    let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
    let msg = format!("expected str, bytes or os.PathLike object, not {obj_type}");
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

pub(crate) fn path_from_bits(
    _py: &PyToken<'_>,
    file_bits: u64,
) -> Result<std::path::PathBuf, String> {
    path_from_bits_with_flavor(_py, file_bits).map(|(path, _flavor)| path)
}

fn filesystem_encoding() -> &'static str {
    "utf-8"
}

fn filesystem_encode_errors() -> &'static str {
    #[cfg(windows)]
    {
        "surrogatepass"
    }
    #[cfg(not(windows))]
    {
        "surrogateescape"
    }
}

fn path_sep_char() -> char {
    std::path::MAIN_SEPARATOR
}

#[cfg(unix)]
fn bytes_text_from_raw(raw: &[u8]) -> String {
    raw.iter().map(|byte| char::from(*byte)).collect()
}

#[cfg(unix)]
fn raw_from_bytes_text(text: &str) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let code = ch as u32;
        if code > 0xFF {
            return None;
        }
        out.push(code as u8);
    }
    Some(out)
}

#[cfg(not(unix))]
fn bytes_text_from_raw(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).into_owned()
}

#[cfg(not(unix))]
fn raw_from_bytes_text(text: &str) -> Option<Vec<u8>> {
    Some(text.as_bytes().to_vec())
}

#[cfg(unix)]
fn path_text_with_flavor(path: &std::path::Path, flavor: PathFlavor) -> String {
    if flavor == PathFlavor::Bytes {
        use std::os::unix::ffi::OsStrExt;
        return bytes_text_from_raw(path.as_os_str().as_bytes());
    }
    path.to_string_lossy().into_owned()
}

#[cfg(not(unix))]
fn path_text_with_flavor(path: &std::path::Path, _flavor: PathFlavor) -> String {
    path.to_string_lossy().into_owned()
}

fn path_string_with_flavor_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
) -> Result<(String, PathFlavor), u64> {
    match path_from_bits_with_flavor(_py, bits) {
        Ok((path, flavor)) => Ok((path_text_with_flavor(path.as_path(), flavor), flavor)),
        Err(msg) => Err(raise_exception::<_>(_py, "TypeError", &msg)),
    }
}

fn path_string_from_bits(_py: &PyToken<'_>, bits: u64) -> Result<String, u64> {
    path_string_with_flavor_from_bits(_py, bits).map(|(path, _flavor)| path)
}

fn path_str_arg_from_bits(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<String, u64> {
    if let Some(text) = string_obj_to_owned(obj_from_bits(bits)) {
        return Ok(text);
    }
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("{label} must be str, not {type_name}");
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

fn path_sequence_from_bits(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<Vec<String>, u64> {
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        let msg = format!("{label} must be tuple or list, not NoneType");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("{label} must be tuple or list, not {type_name}");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    }
    let elems = unsafe { seq_vec_ref(ptr) };
    let mut out = Vec::with_capacity(elems.len());
    for item_bits in elems {
        let value = path_string_from_bits(_py, *item_bits)?;
        out.push(value);
    }
    Ok(out)
}

pub(crate) fn path_join_text(mut base: String, part: &str, sep: char) -> String {
    if part.starts_with(sep) {
        return part.to_string();
    }
    if !base.is_empty() && !base.ends_with(sep) {
        base.push(sep);
    }
    base.push_str(part);
    base
}

fn path_join_many_text(mut base: String, parts: &[String], sep: char) -> String {
    for part in parts {
        base = path_join_text(base, part, sep);
    }
    base
}

fn alloc_string_list_bits(_py: &PyToken<'_>, values: &[String]) -> u64 {
    let mut out_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = alloc_string(_py, value.as_bytes());
        if ptr.is_null() {
            for bits in out_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        out_bits.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, out_bits.as_slice());
    for bits in out_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

fn alloc_path_list_bits(_py: &PyToken<'_>, values: &[String], bytes_out: bool) -> u64 {
    let mut out_bits: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = if bytes_out {
            match raw_from_bytes_text(value) {
                Some(raw) => alloc_bytes(_py, raw.as_slice()),
                None => alloc_bytes(_py, value.as_bytes()),
            }
        } else {
            alloc_string(_py, value.as_bytes())
        };
        if ptr.is_null() {
            for bits in out_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        out_bits.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, out_bits.as_slice());
    for bits in out_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

pub(crate) fn path_basename_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return String::new();
    }
    let stripped = path.trim_end_matches(sep);
    if stripped.is_empty() {
        return sep.to_string();
    }
    match stripped.rfind(sep) {
        Some(idx) => stripped[idx + sep.len_utf8()..].to_string(),
        None => stripped.to_string(),
    }
}

pub(crate) fn path_dirname_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return String::new();
    }
    let stripped = path.trim_end_matches(sep);
    if stripped.is_empty() {
        return sep.to_string();
    }
    match stripped.rfind(sep) {
        Some(0) => sep.to_string(),
        Some(idx) => stripped[..idx].to_string(),
        None => String::new(),
    }
}

fn path_splitext_text(path: &str, sep: char) -> (String, String) {
    let base = path_basename_text(path, sep);
    if !base.contains('.') || base == "." || base == ".." {
        return (path.to_string(), String::new());
    }
    let idx = match base.rfind('.') {
        Some(idx) => idx,
        None => return (path.to_string(), String::new()),
    };
    let root_len = path.len().saturating_sub(base.len()) + idx;
    let root = path[..root_len].to_string();
    let ext = base[idx..].to_string();
    (root, ext)
}

fn path_name_text(path: &str, sep: char) -> String {
    let parts = path_parts_text(path, sep);
    if parts.is_empty() {
        return String::new();
    }
    let sep_s = sep.to_string();
    if parts.len() == 1 && parts[0] == sep_s {
        return String::new();
    }
    parts.last().cloned().unwrap_or_default()
}

fn path_suffix_text(path: &str, sep: char) -> String {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return String::new();
    }
    let (_, suffix) = path_splitext_text(&name, sep);
    suffix
}

fn path_suffixes_text(path: &str, sep: char) -> Vec<String> {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return Vec::new();
    }
    let mut suffixes: Vec<String> = Vec::new();
    let mut stem = name;
    loop {
        let (next_stem, suffix) = path_splitext_text(&stem, sep);
        if suffix.is_empty() {
            break;
        }
        suffixes.push(suffix);
        stem = next_stem;
    }
    suffixes.reverse();
    suffixes
}

fn path_stem_text(path: &str, sep: char) -> String {
    let name = path_name_text(path, sep);
    if name.is_empty() || name == "." {
        return String::new();
    }
    let (stem, _) = path_splitext_text(&name, sep);
    stem
}

fn path_as_uri_text(path: &str, sep: char) -> Result<String, String> {
    if !path.starts_with(sep) {
        return Err("relative path can't be expressed as a file URI".to_string());
    }
    let mut posix = if sep == '/' {
        path.to_string()
    } else {
        path.replace(sep, "/")
    };
    if !posix.starts_with('/') {
        posix.insert(0, '/');
    }
    Ok(format!("file://{posix}"))
}

pub(crate) fn path_normpath_text(path: &str, sep: char) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let absolute = path.starts_with(sep);
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(sep) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if parts.last().is_some_and(|last| *last != "..") {
                parts.pop();
            } else if !absolute {
                parts.push(part);
            }
            continue;
        }
        parts.push(part);
    }
    let sep_s = sep.to_string();
    if absolute {
        let normalized = format!("{sep}{}", parts.join(&sep_s));
        if normalized.is_empty() {
            sep.to_string()
        } else {
            normalized
        }
    } else {
        let normalized = parts.join(&sep_s);
        if normalized.is_empty() {
            ".".to_string()
        } else {
            normalized
        }
    }
}

fn path_abspath_text(_py: &PyToken<'_>, path: &str, sep: char) -> Result<String, u64> {
    let mut current = path.to_string();
    if !path_isabs_text(&current, sep) {
        if !has_capability(_py, "fs.read") {
            return Err(raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read capability",
            ));
        }
        let cwd = match std::env::current_dir() {
            Ok(path) => path.to_string_lossy().into_owned(),
            Err(err) => {
                let msg = err.to_string();
                let bits = match err.kind() {
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::NotADirectory => {
                        raise_exception::<_>(_py, "NotADirectoryError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                };
                return Err(bits);
            }
        };
        current = path_join_text(cwd, &current, sep);
    }
    Ok(path_normpath_text(&current, sep))
}

fn path_resolve_text(
    _py: &PyToken<'_>,
    path: &str,
    sep: char,
    strict: bool,
) -> Result<String, u64> {
    let absolute = path_abspath_text(_py, path, sep)?;
    if !has_capability(_py, "fs.read") {
        if strict {
            return Err(raise_exception::<_>(
                _py,
                "PermissionError",
                "missing fs.read capability",
            ));
        }
        return Ok(absolute);
    }
    let resolved = std::path::Path::new(&absolute);
    match std::fs::canonicalize(resolved) {
        Ok(path_buf) => Ok(path_normpath_text(&path_buf.to_string_lossy(), sep)),
        Err(err)
            if !strict && matches!(err.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
        {
            Ok(path_normpath_text(&absolute, sep))
        }
        Err(err) => {
            let msg = err.to_string();
            let bits = match err.kind() {
                ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                ErrorKind::PermissionDenied => raise_exception::<_>(_py, "PermissionError", &msg),
                ErrorKind::NotADirectory => raise_exception::<_>(_py, "NotADirectoryError", &msg),
                _ => raise_exception::<_>(_py, "OSError", &msg),
            };
            Err(bits)
        }
    }
}

fn path_parts_text(path: &str, sep: char) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let (drive, root, tail) = path_splitroot_text(path, sep);
    if !drive.is_empty() || !root.is_empty() {
        out.push(format!("{drive}{root}"));
    }
    for part in tail.split(sep) {
        if part.is_empty() || part == "." {
            continue;
        }
        out.push(part.to_string());
    }
    out
}

fn path_compare_text(lhs: &str, rhs: &str, sep: char) -> i64 {
    let lhs_parts = path_parts_text(lhs, sep);
    let rhs_parts = path_parts_text(rhs, sep);
    use std::cmp::Ordering;
    match lhs_parts.cmp(&rhs_parts) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

fn path_parents_text(path: &str, sep: char) -> Vec<String> {
    let (drive, root, tail) = path_splitroot_text(path, sep);
    let anchor = format!("{drive}{root}");
    let tail_parts = tail
        .split(sep)
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToOwned::to_owned)
        .collect::<Vec<String>>();
    if tail_parts.is_empty() {
        return Vec::new();
    }
    let sep_s = sep.to_string();
    let mut out: Vec<String> = Vec::new();
    let mut idx = tail_parts.len();
    while idx > 0 {
        idx -= 1;
        if idx == 0 {
            if anchor.is_empty() {
                out.push(".".to_string());
            } else {
                out.push(anchor.clone());
            }
            continue;
        }
        let prefix = tail_parts[..idx].join(&sep_s);
        if anchor.is_empty() {
            out.push(prefix);
        } else {
            out.push(format!("{anchor}{prefix}"));
        }
    }
    out
}

fn path_isabs_text(path: &str, sep: char) -> bool {
    #[cfg(windows)]
    {
        let text = path.replace('/', "\\");
        if text.starts_with("\\\\") || text.starts_with('\\') {
            return true;
        }
        let bytes = text.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
        false
    }
    #[cfg(not(windows))]
    {
        path.starts_with(sep)
    }
}

fn path_match_simple_pattern(name: &str, pat: &str) -> bool {
    fn parse_char_class(
        pat: &[char],
        mut idx: usize,
    ) -> Option<(Vec<char>, Vec<(char, char)>, bool, usize)> {
        if idx >= pat.len() || pat[idx] != '[' {
            return None;
        }
        idx += 1;
        if idx >= pat.len() {
            return None;
        }

        let mut negate = false;
        if pat[idx] == '!' {
            negate = true;
            idx += 1;
        }
        if idx >= pat.len() {
            return None;
        }

        let mut singles: Vec<char> = Vec::new();
        let mut ranges: Vec<(char, char)> = Vec::new();

        if pat[idx] == ']' {
            singles.push(']');
            idx += 1;
        }

        while idx < pat.len() && pat[idx] != ']' {
            if idx + 2 < pat.len() && pat[idx + 1] == '-' && pat[idx + 2] != ']' {
                let start = pat[idx];
                let end = pat[idx + 2];
                if start <= end {
                    ranges.push((start, end));
                }
                idx += 3;
                continue;
            }
            singles.push(pat[idx]);
            idx += 1;
        }
        if idx >= pat.len() || pat[idx] != ']' {
            return None;
        }
        Some((singles, ranges, negate, idx + 1))
    }

    fn char_class_hit(ch: char, singles: &[char], ranges: &[(char, char)], negate: bool) -> bool {
        let mut hit = singles.contains(&ch);
        if !hit {
            hit = ranges.iter().any(|(start, end)| *start <= ch && ch <= *end);
        }
        if negate { !hit } else { hit }
    }

    let name_chars: Vec<char> = name.chars().collect();
    let pat_chars: Vec<char> = pat.chars().collect();
    let mut pi: usize = 0;
    let mut ni: usize = 0;
    let mut star_idx: Option<usize> = None;
    let mut matched_from_star: usize = 0;

    while ni < name_chars.len() {
        if pi < pat_chars.len() && pat_chars[pi] == '*' {
            while pi < pat_chars.len() && pat_chars[pi] == '*' {
                pi += 1;
            }
            if pi == pat_chars.len() {
                return true;
            }
            star_idx = Some(pi);
            matched_from_star = ni;
            continue;
        }
        if pi < pat_chars.len() && pat_chars[pi] == '[' {
            if let Some((singles, ranges, negate, next_idx)) = parse_char_class(&pat_chars, pi) {
                let hit = char_class_hit(name_chars[ni], &singles, &ranges, negate);
                if hit {
                    pi = next_idx;
                    ni += 1;
                    continue;
                }
                if let Some(star) = star_idx {
                    matched_from_star += 1;
                    ni = matched_from_star;
                    pi = star;
                    continue;
                }
                return false;
            }
        }
        if pi < pat_chars.len() && (pat_chars[pi] == '?' || pat_chars[pi] == name_chars[ni]) {
            pi += 1;
            ni += 1;
            continue;
        }
        if let Some(star) = star_idx {
            matched_from_star += 1;
            ni = matched_from_star;
            pi = star;
            continue;
        }
        return false;
    }
    while pi < pat_chars.len() && pat_chars[pi] == '*' {
        pi += 1;
    }
    pi == pat_chars.len()
}

fn path_match_text(path: &str, pattern: &str, sep: char) -> bool {
    #[cfg(windows)]
    let pattern = pattern.replace('/', "\\");
    #[cfg(not(windows))]
    let pattern = pattern.to_string();
    let absolute = pattern.starts_with(sep);
    if absolute && !path.starts_with(sep) {
        return false;
    }
    let pat = if absolute {
        pattern.trim_start_matches(sep)
    } else {
        pattern.as_str()
    };
    let path_trimmed = path.trim_start_matches(sep);
    if !pat.contains(sep) && !pat.contains('/') {
        let name = path_basename_text(path, sep);
        if pat == "*" {
            return !name.is_empty();
        }
        if pat.starts_with("*.") && pat.matches('*').count() == 1 && !pat.contains('?') {
            return name.ends_with(&pat[1..]);
        }
        return path_match_simple_pattern(&name, pat);
    }

    fn split_components(text: &str, sep: char) -> Vec<&str> {
        text.split(sep)
            .filter(|part| !part.is_empty() && *part != ".")
            .collect()
    }

    fn match_components(path_parts: &[&str], pat_parts: &[&str]) -> bool {
        fn inner(path_parts: &[&str], pat_parts: &[&str], pi: usize, pj: usize) -> bool {
            if pj >= pat_parts.len() {
                return pi >= path_parts.len();
            }
            let pat = pat_parts[pj];
            if pat == "**" {
                if inner(path_parts, pat_parts, pi, pj + 1) {
                    return true;
                }
                return pi < path_parts.len() && inner(path_parts, pat_parts, pi + 1, pj);
            }
            if pi >= path_parts.len() {
                return false;
            }
            path_match_simple_pattern(path_parts[pi], pat)
                && inner(path_parts, pat_parts, pi + 1, pj + 1)
        }
        inner(path_parts, pat_parts, 0, 0)
    }

    let pat_parts = split_components(pat, sep);
    let path_parts = split_components(path_trimmed, sep);
    if absolute {
        return match_components(&path_parts, &pat_parts);
    }
    for start in 0..=path_parts.len() {
        if match_components(&path_parts[start..], &pat_parts) {
            return true;
        }
    }
    false
}

fn glob_has_magic_text(pathname: &str) -> bool {
    pathname
        .as_bytes()
        .iter()
        .any(|ch| matches!(*ch, b'*' | b'?' | b'['))
}

#[derive(Clone, Debug)]
enum GlobDirFdArg {
    None,
    Int(i64),
    PathLike {
        path: String,
        flavor: PathFlavor,
        type_name: String,
    },
    BadType {
        type_name: String,
    },
}

fn glob_dir_fd_type_error_bits(_py: &PyToken<'_>, type_name: &str) -> u64 {
    let msg = format!("argument should be integer or None, not {type_name}");
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn glob_scandir_type_error_bits(_py: &PyToken<'_>, type_name: &str) -> u64 {
    let msg = format!(
        "scandir: path should be string, bytes, os.PathLike, integer or None, not {type_name}"
    );
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn glob_dir_fd_arg_from_bits(_py: &PyToken<'_>, dir_fd_bits: u64) -> Result<GlobDirFdArg, u64> {
    if obj_from_bits(dir_fd_bits).is_none() {
        return Ok(GlobDirFdArg::None);
    }
    let type_name = class_name_for_error(type_of_bits(_py, dir_fd_bits));
    let err = format!("argument should be integer or None, not {type_name}");
    if let Some(value) = index_bigint_from_obj(_py, dir_fd_bits, &err) {
        if let Some(fd) = value.to_i64() {
            return Ok(GlobDirFdArg::Int(fd));
        }
        let msg = if value.sign() == Sign::Minus {
            "fd is less than minimum"
        } else {
            "fd is greater than maximum"
        };
        return Err(raise_exception::<_>(_py, "OverflowError", msg));
    }
    if exception_pending(_py) {
        clear_exception(_py);
    }
    match path_string_with_flavor_from_bits(_py, dir_fd_bits) {
        Ok((path, flavor)) => {
            #[cfg(windows)]
            let path = path.replace('/', "\\");
            Ok(GlobDirFdArg::PathLike {
                path,
                flavor,
                type_name,
            })
        }
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            Ok(GlobDirFdArg::BadType { type_name })
        }
    }
}

#[cfg(unix)]
fn glob_text_to_path(text: &str, bytes_mode: bool) -> std::path::PathBuf {
    if bytes_mode {
        if let Some(raw) = raw_from_bytes_text(text) {
            use std::os::unix::ffi::OsStringExt;
            return std::path::PathBuf::from(std::ffi::OsString::from_vec(raw));
        }
    }
    std::path::PathBuf::from(text)
}

#[cfg(not(unix))]
fn glob_text_to_path(text: &str, _bytes_mode: bool) -> std::path::PathBuf {
    std::path::PathBuf::from(text)
}

#[cfg(unix)]
fn glob_dir_entry_name_text(name: &std::ffi::OsStr, bytes_mode: bool) -> String {
    if bytes_mode {
        use std::os::unix::ffi::OsStrExt;
        return bytes_text_from_raw(name.as_bytes());
    }
    name.to_string_lossy().into_owned()
}

#[cfg(not(unix))]
fn glob_dir_entry_name_text(name: &std::ffi::OsStr, _bytes_mode: bool) -> String {
    name.to_string_lossy().into_owned()
}

#[cfg(all(unix, target_vendor = "apple"))]
fn glob_dir_fd_root_text(fd: i64, bytes_mode: bool) -> Option<String> {
    if fd < 0 {
        return None;
    }
    // Apple targets do not provide a stable /proc/self/fd lane; use fcntl(F_GETPATH).
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    let rc = unsafe {
        libc::fcntl(
            fd as libc::c_int,
            libc::F_GETPATH,
            buf.as_mut_ptr() as *mut libc::c_char,
        )
    };
    if rc != -1 {
        if let Some(nul_idx) = buf.iter().position(|byte| *byte == 0) {
            if nul_idx > 0 {
                return Some(bytes_text_from_raw(&buf[..nul_idx]));
            }
        }
    }
    for candidate in [format!("/proc/self/fd/{fd}"), format!("/dev/fd/{fd}")] {
        if let Ok(path) = std::fs::read_link(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
        if let Ok(path) = std::fs::canonicalize(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
    }
    None
}

#[cfg(all(unix, not(target_vendor = "apple")))]
fn glob_dir_fd_root_text(fd: i64, bytes_mode: bool) -> Option<String> {
    if fd < 0 {
        return None;
    }
    for candidate in [format!("/proc/self/fd/{fd}"), format!("/dev/fd/{fd}")] {
        if let Ok(path) = std::fs::read_link(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
        if let Ok(path) = std::fs::canonicalize(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
    }
    None
}

#[cfg(windows)]
fn glob_dir_fd_root_text(fd: i64, _bytes_mode: bool) -> Option<String> {
    if fd < 0 {
        return None;
    }
    let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
    if handle == -1 {
        return None;
    }
    windows_path_from_handle(handle as *mut std::ffi::c_void)
}

#[cfg(target_arch = "wasm32")]
fn glob_dir_fd_root_text(fd: i64, bytes_mode: bool) -> Option<String> {
    if fd < 0 {
        return None;
    }
    for candidate in [format!("/proc/self/fd/{fd}"), format!("/dev/fd/{fd}")] {
        if let Ok(path) = std::fs::read_link(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
        if let Ok(path) = std::fs::canonicalize(&candidate) {
            return Some(path_text_with_flavor(
                path.as_path(),
                if bytes_mode {
                    PathFlavor::Bytes
                } else {
                    PathFlavor::Str
                },
            ));
        }
    }
    None
}

#[cfg(all(not(unix), not(windows), not(target_arch = "wasm32")))]
fn glob_dir_fd_root_text(_fd: i64, _bytes_mode: bool) -> Option<String> {
    None
}

fn glob_is_hidden_text(name: &str) -> bool {
    name.starts_with('.')
}

fn glob_split_path_text(pathname: &str, sep: char) -> (String, String) {
    let (drive, root, tail) = path_splitroot_text(pathname, sep);
    if tail.is_empty() {
        return (format!("{drive}{root}"), String::new());
    }

    let mut head = String::new();
    let mut base = tail.clone();
    if let Some(idx) = tail.rfind(sep) {
        head = tail[..idx + sep.len_utf8()].to_string();
        base = tail[idx + sep.len_utf8()..].to_string();
    }

    if !head.is_empty() {
        let all_sep = head.chars().all(|ch| ch == sep);
        if !all_sep {
            head = head.trim_end_matches(sep).to_string();
        }
    }

    let dirname = format!("{drive}{root}{head}");
    (dirname, base)
}

fn glob_join_text(base: &str, part: &str, sep: char) -> String {
    if base.is_empty() {
        return part.to_string();
    }
    if path_isabs_text(part, sep) {
        return part.to_string();
    }
    #[cfg(windows)]
    {
        let (part_drive, _part_root, _part_tail) = path_splitroot_text(part, sep);
        if !part_drive.is_empty() {
            return part.to_string();
        }
    }
    path_join_text(base.to_string(), part, sep)
}

fn glob_lexists_text(
    _py: &PyToken<'_>,
    path: &str,
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    sep: char,
) -> Result<bool, u64> {
    if path.is_empty() {
        return Ok(false);
    }
    let resolved = match dir_fd {
        GlobDirFdArg::None => path.to_string(),
        GlobDirFdArg::Int(fd) => {
            if path_isabs_text(path, sep) {
                path.to_string()
            } else if let Some(root) = glob_dir_fd_root_text(*fd, bytes_mode) {
                glob_join_text(&root, path, sep)
            } else {
                return Ok(false);
            }
        }
        GlobDirFdArg::PathLike { type_name, .. } | GlobDirFdArg::BadType { type_name } => {
            return Err(glob_dir_fd_type_error_bits(_py, type_name));
        }
    };
    let resolved_path = glob_text_to_path(&resolved, bytes_mode);
    Ok(std::fs::symlink_metadata(resolved_path).is_ok())
}

fn glob_is_dir_text(
    _py: &PyToken<'_>,
    path: &str,
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    sep: char,
) -> Result<bool, u64> {
    if path.is_empty() {
        return Ok(false);
    }
    let resolved = match dir_fd {
        GlobDirFdArg::None => path.to_string(),
        GlobDirFdArg::Int(fd) => {
            if path_isabs_text(path, sep) {
                path.to_string()
            } else if let Some(root) = glob_dir_fd_root_text(*fd, bytes_mode) {
                glob_join_text(&root, path, sep)
            } else {
                return Ok(false);
            }
        }
        GlobDirFdArg::PathLike { type_name, .. } | GlobDirFdArg::BadType { type_name } => {
            return Err(glob_dir_fd_type_error_bits(_py, type_name));
        }
    };
    let resolved_path = glob_text_to_path(&resolved, bytes_mode);
    Ok(std::fs::metadata(resolved_path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false))
}

struct GlobListdirResult {
    names: Vec<String>,
    names_are_bytes: bool,
}

fn glob_listdir_text(
    _py: &PyToken<'_>,
    dirname: &str,
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<GlobListdirResult, u64> {
    let target: String;
    let mut target_bytes_mode = bytes_mode;
    let arg_is_bytes;

    match dir_fd {
        GlobDirFdArg::None => {
            if dirname.is_empty() {
                target = ".".to_string();
                arg_is_bytes = bytes_mode;
            } else {
                target = dirname.to_string();
                arg_is_bytes = bytes_mode;
            }
        }
        GlobDirFdArg::Int(fd) => {
            if dirname.is_empty() {
                if let Some(root) = glob_dir_fd_root_text(*fd, bytes_mode) {
                    target = root;
                } else if *fd == -1 {
                    // CPython's scandir(-1) can expose CWD on some hosts.
                    target = ".".to_string();
                } else {
                    return Ok(GlobListdirResult {
                        names: Vec::new(),
                        names_are_bytes: bytes_mode,
                    });
                }
                arg_is_bytes = false;
            } else if path_isabs_text(dirname, sep) {
                target = dirname.to_string();
                arg_is_bytes = bytes_mode;
            } else if let Some(root) = glob_dir_fd_root_text(*fd, bytes_mode) {
                target = glob_join_text(&root, dirname, sep);
                arg_is_bytes = bytes_mode;
            } else {
                return Ok(GlobListdirResult {
                    names: Vec::new(),
                    names_are_bytes: bytes_mode,
                });
            }
        }
        GlobDirFdArg::PathLike {
            path,
            flavor,
            type_name,
        } => {
            if !dirname.is_empty() {
                return Err(glob_dir_fd_type_error_bits(_py, type_name));
            }
            target = path.clone();
            target_bytes_mode = *flavor == PathFlavor::Bytes;
            arg_is_bytes = *flavor == PathFlavor::Bytes;
        }
        GlobDirFdArg::BadType { type_name } => {
            if dirname.is_empty() {
                return Err(glob_scandir_type_error_bits(_py, type_name));
            }
            return Err(glob_dir_fd_type_error_bits(_py, type_name));
        }
    }

    let names_are_bytes = bytes_mode || arg_is_bytes;
    let target_path = glob_text_to_path(&target, target_bytes_mode);
    let mut out: Vec<String> = Vec::new();
    let iter = match std::fs::read_dir(target_path) {
        Ok(iter) => iter,
        Err(_) => {
            return Ok(GlobListdirResult {
                names: out,
                names_are_bytes,
            });
        }
    };
    for entry_res in iter {
        let Ok(entry) = entry_res else {
            continue;
        };
        if dironly {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
        }
        out.push(glob_dir_entry_name_text(
            entry.file_name().as_os_str(),
            names_are_bytes,
        ));
    }
    Ok(GlobListdirResult {
        names: out,
        names_are_bytes,
    })
}

fn glob1_text(
    _py: &PyToken<'_>,
    dirname: &str,
    pattern: &str,
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let listed = glob_listdir_text(_py, dirname, dir_fd, dironly, bytes_mode, sep)?;
    if listed.names_are_bytes != bytes_mode {
        let msg = if bytes_mode {
            "cannot use a bytes pattern on a string-like object"
        } else {
            "cannot use a string pattern on a bytes-like object"
        };
        return Err(raise_exception::<_>(_py, "TypeError", msg));
    }
    let mut names = listed.names;
    if !(pattern.starts_with('.') || include_hidden) {
        names.retain(|name| !glob_is_hidden_text(name));
    }
    names.retain(|name| path_match_simple_pattern(name, pattern));
    Ok(names)
}

fn glob0_text(
    _py: &PyToken<'_>,
    dirname: &str,
    basename: &str,
    dir_fd: &GlobDirFdArg,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    if !basename.is_empty() {
        let full = glob_join_text(dirname, basename, sep);
        if glob_lexists_text(_py, &full, dir_fd, bytes_mode, sep)? {
            return Ok(vec![basename.to_string()]);
        }
        return Ok(Vec::new());
    }
    if glob_is_dir_text(_py, dirname, dir_fd, bytes_mode, sep)? {
        return Ok(vec![String::new()]);
    }
    Ok(Vec::new())
}

fn glob_rlistdir_text(
    _py: &PyToken<'_>,
    dirname: &str,
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let mut out: Vec<String> = Vec::new();
    let listed = glob_listdir_text(_py, dirname, dir_fd, dironly, bytes_mode, sep)?;
    let names = listed.names;
    for name in names {
        if !include_hidden && glob_is_hidden_text(&name) {
            continue;
        }
        out.push(name.clone());
        let path = if dirname.is_empty() {
            name.clone()
        } else {
            glob_join_text(dirname, &name, sep)
        };
        for child in
            glob_rlistdir_text(_py, &path, dir_fd, dironly, include_hidden, bytes_mode, sep)?
        {
            out.push(glob_join_text(&name, &child, sep));
        }
    }
    Ok(out)
}

fn glob2_text(
    _py: &PyToken<'_>,
    dirname: &str,
    dir_fd: &GlobDirFdArg,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let mut out: Vec<String> = Vec::new();
    if dirname.is_empty() || glob_is_dir_text(_py, dirname, dir_fd, bytes_mode, sep)? {
        out.push(String::new());
    }
    out.extend(glob_rlistdir_text(
        _py,
        dirname,
        dir_fd,
        dironly,
        include_hidden,
        bytes_mode,
        sep,
    )?);
    Ok(out)
}

fn glob_iglob_text(
    _py: &PyToken<'_>,
    pathname: &str,
    root_dir: Option<&str>,
    dir_fd: &GlobDirFdArg,
    recursive: bool,
    dironly: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let (dirname, basename) = glob_split_path_text(pathname, sep);
    if !glob_has_magic_text(pathname) {
        if !basename.is_empty() {
            let full = match root_dir {
                Some(root) => glob_join_text(root, pathname, sep),
                None => pathname.to_string(),
            };
            if glob_lexists_text(_py, &full, dir_fd, bytes_mode, sep)? {
                return Ok(vec![pathname.to_string()]);
            }
        } else {
            let full_dir = match root_dir {
                Some(root) => glob_join_text(root, &dirname, sep),
                None => dirname.clone(),
            };
            if glob_is_dir_text(_py, &full_dir, dir_fd, bytes_mode, sep)? {
                return Ok(vec![pathname.to_string()]);
            }
        }
        return Ok(Vec::new());
    }

    if dirname.is_empty() {
        let in_dir = root_dir.unwrap_or("");
        if recursive && basename == "**" {
            return glob2_text(
                _py,
                in_dir,
                dir_fd,
                dironly,
                include_hidden,
                bytes_mode,
                sep,
            );
        }
        return glob1_text(
            _py,
            in_dir,
            &basename,
            dir_fd,
            dironly,
            include_hidden,
            bytes_mode,
            sep,
        );
    }

    let mut dirs: Vec<String> = Vec::new();
    if dirname != pathname && glob_has_magic_text(&dirname) {
        dirs = glob_iglob_text(
            _py,
            &dirname,
            root_dir,
            dir_fd,
            recursive,
            true,
            include_hidden,
            bytes_mode,
            sep,
        )?;
    } else {
        dirs.push(dirname.clone());
    }

    let basename_has_magic = glob_has_magic_text(&basename);
    let basename_recursive = recursive && basename == "**";
    let mut out: Vec<String> = Vec::new();
    for parent in dirs {
        let search_dir = match root_dir {
            Some(root) => glob_join_text(root, &parent, sep),
            None => parent.clone(),
        };
        let names = if basename_has_magic {
            if basename_recursive {
                glob2_text(
                    _py,
                    &search_dir,
                    dir_fd,
                    dironly,
                    include_hidden,
                    bytes_mode,
                    sep,
                )?
            } else {
                glob1_text(
                    _py,
                    &search_dir,
                    &basename,
                    dir_fd,
                    dironly,
                    include_hidden,
                    bytes_mode,
                    sep,
                )?
            }
        } else {
            glob0_text(_py, &search_dir, &basename, dir_fd, bytes_mode, sep)?
        };
        for name in names {
            out.push(glob_join_text(&parent, &name, sep));
        }
    }
    Ok(out)
}

fn glob_matches_text(
    _py: &PyToken<'_>,
    pathname: &str,
    root_dir: Option<&str>,
    dir_fd: &GlobDirFdArg,
    recursive: bool,
    include_hidden: bool,
    bytes_mode: bool,
    sep: char,
) -> Result<Vec<String>, u64> {
    let mut out = glob_iglob_text(
        _py,
        pathname,
        root_dir,
        dir_fd,
        recursive,
        false,
        include_hidden,
        bytes_mode,
        sep,
    )?;
    if (pathname.is_empty() || (recursive && pathname.starts_with("**")))
        && out.first().is_some_and(String::is_empty)
    {
        out.remove(0);
    }
    Ok(out)
}

fn glob_escape_text(pathname: &str, sep: char) -> String {
    let (drive, root, tail) = path_splitroot_text(pathname, sep);
    let mut out = String::new();
    out.push_str(&drive);
    out.push_str(&root);
    for ch in tail.chars() {
        if matches!(ch, '*' | '?' | '[') {
            out.push('[');
            out.push(ch);
            out.push(']');
        } else {
            out.push(ch);
        }
    }
    out
}

fn glob_regex_escape_char(out: &mut String, ch: char) {
    if matches!(
        ch,
        '.' | '^' | '$' | '+' | '{' | '}' | '(' | ')' | '|' | '\\' | '[' | ']'
    ) {
        out.push('\\');
    }
    out.push(ch);
}

fn glob_regex_escape_text(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        glob_regex_escape_char(&mut out, ch);
    }
    out
}

fn glob_split_on_seps(pat: &str, seps: &[char]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in pat.chars() {
        if seps.contains(&ch) {
            out.push(cur);
            cur = String::new();
        } else {
            cur.push(ch);
        }
    }
    out.push(cur);
    out
}

fn glob_translate_parse_char_class(
    pat: &[char],
    mut idx: usize,
) -> Option<(Vec<char>, Vec<(char, char)>, bool, usize)> {
    if idx >= pat.len() || pat[idx] != '[' {
        return None;
    }
    idx += 1;
    if idx >= pat.len() {
        return None;
    }

    let mut negate = false;
    if pat[idx] == '!' {
        negate = true;
        idx += 1;
    }
    if idx >= pat.len() {
        return None;
    }

    let mut singles: Vec<char> = Vec::new();
    let mut ranges: Vec<(char, char)> = Vec::new();

    if pat[idx] == ']' {
        singles.push(']');
        idx += 1;
    }
    while idx < pat.len() && pat[idx] != ']' {
        if idx + 2 < pat.len() && pat[idx + 1] == '-' && pat[idx + 2] != ']' {
            let start = pat[idx];
            let end = pat[idx + 2];
            if start <= end {
                ranges.push((start, end));
            }
            idx += 3;
            continue;
        }
        singles.push(pat[idx]);
        idx += 1;
    }
    if idx >= pat.len() || pat[idx] != ']' {
        return None;
    }
    Some((singles, ranges, negate, idx + 1))
}

fn glob_translate_char_class(
    singles: Vec<char>,
    ranges: Vec<(char, char)>,
    negate: bool,
) -> String {
    let mut out = String::new();
    out.push('[');
    if negate {
        out.push('^');
    }
    for ch in singles {
        if matches!(ch, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(ch);
    }
    for (start, end) in ranges {
        if matches!(start, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(start);
        out.push('-');
        if matches!(end, '\\' | '^' | '-' | ']') {
            out.push('\\');
        }
        out.push(end);
    }
    out.push(']');
    out
}

fn glob_translate_segment(part: &str, star_expr: &str, ques_expr: &str) -> String {
    let chars: Vec<char> = part.chars().collect();
    let mut out = String::new();
    let mut idx = 0usize;
    while idx < chars.len() {
        match chars[idx] {
            '*' => out.push_str(star_expr),
            '?' => out.push_str(ques_expr),
            '[' => {
                if let Some((singles, ranges, negate, next_idx)) =
                    glob_translate_parse_char_class(&chars, idx)
                {
                    out.push_str(&glob_translate_char_class(singles, ranges, negate));
                    idx = next_idx;
                    continue;
                } else {
                    out.push_str("\\[");
                }
            }
            ch => glob_regex_escape_char(&mut out, ch),
        }
        idx += 1;
    }
    out
}

fn glob_default_seps_text() -> String {
    #[cfg(windows)]
    {
        "\\/".to_string()
    }
    #[cfg(not(windows))]
    {
        "/".to_string()
    }
}

fn glob_translate_text(
    pat: &str,
    recursive: bool,
    include_hidden: bool,
    seps: Option<&str>,
) -> String {
    let seps_text = if let Some(raw) = seps {
        if raw.is_empty() {
            glob_default_seps_text()
        } else {
            raw.to_string()
        }
    } else {
        glob_default_seps_text()
    };
    let sep_chars: Vec<char> = seps_text.chars().collect();
    let escaped_seps = glob_regex_escape_text(&seps_text);
    let any_sep = if sep_chars.len() > 1 {
        format!("[{escaped_seps}]")
    } else {
        escaped_seps.clone()
    };
    let not_sep = format!("[^{escaped_seps}]");
    let (one_last_segment, one_segment, any_segments, any_last_segments) = if include_hidden {
        let one_last_segment = format!("{not_sep}+");
        let one_segment = format!("{one_last_segment}{any_sep}");
        let any_segments = format!("(?:.+{any_sep})?");
        let any_last_segments = ".*".to_string();
        (
            one_last_segment,
            one_segment,
            any_segments,
            any_last_segments,
        )
    } else {
        let one_last_segment = format!("[^{escaped_seps}.]{not_sep}*");
        let one_segment = format!("{one_last_segment}{any_sep}");
        let any_segments = format!("(?:{one_segment})*");
        let any_last_segments = format!("{any_segments}(?:{one_last_segment})?");
        (
            one_last_segment,
            one_segment,
            any_segments,
            any_last_segments,
        )
    };

    let parts = glob_split_on_seps(pat, &sep_chars);
    let last_part_idx = parts.len().saturating_sub(1);
    let mut results: Vec<String> = Vec::new();
    for (idx, part) in parts.iter().enumerate() {
        if part == "*" {
            if idx < last_part_idx {
                results.push(one_segment.clone());
            } else {
                results.push(one_last_segment.clone());
            }
        } else if recursive && part == "**" {
            if idx < last_part_idx {
                if parts[idx + 1] != "**" {
                    results.push(any_segments.clone());
                }
            } else {
                results.push(any_last_segments.clone());
            }
        } else {
            if !part.is_empty() {
                if !include_hidden && part.chars().next().is_some_and(|ch| ch == '*' || ch == '?') {
                    results.push(r"(?!\.)".to_string());
                }
                let star_expr = format!("{not_sep}*");
                results.push(glob_translate_segment(part, &star_expr, &not_sep));
            }
            if idx < last_part_idx {
                results.push(any_sep.clone());
            }
        }
    }
    let body = results.join("");
    format!("(?s:{body})\\Z")
}

fn glob_split_components(text: &str, sep: char) -> Vec<String> {
    text.split(sep)
        .filter(|part| !part.is_empty() && *part != ".")
        .map(ToOwned::to_owned)
        .collect()
}

fn glob_walk(
    dir: &std::path::Path,
    rel_parts: &mut Vec<String>,
    pat_parts: &[String],
    pi: usize,
    sep: char,
    out: &mut Vec<String>,
) -> std::io::Result<()> {
    let sep_s = sep.to_string();
    if pi >= pat_parts.len() {
        if !rel_parts.is_empty() {
            out.push(rel_parts.join(&sep_s));
        }
        return Ok(());
    }
    let pat = &pat_parts[pi];
    if pat == "**" {
        glob_walk(dir, rel_parts, pat_parts, pi + 1, sep, out)?;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            rel_parts.push(name);
            glob_walk(&entry.path(), rel_parts, pat_parts, pi, sep, out)?;
            rel_parts.pop();
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !path_match_simple_pattern(&name, pat) {
            continue;
        }
        let file_type = entry.file_type()?;
        rel_parts.push(name);
        if pi + 1 >= pat_parts.len() {
            out.push(rel_parts.join(&sep_s));
        } else if file_type.is_dir() {
            glob_walk(&entry.path(), rel_parts, pat_parts, pi + 1, sep, out)?;
        }
        rel_parts.pop();
    }
    Ok(())
}

fn path_glob_matches(
    dir: &std::path::Path,
    pattern: &str,
    sep: char,
) -> std::io::Result<Vec<String>> {
    #[cfg(windows)]
    let pattern = pattern.replace('/', "\\");
    #[cfg(not(windows))]
    let pattern = pattern.to_string();
    let pat_parts = glob_split_components(&pattern, sep);
    let mut matches: Vec<String> = Vec::new();
    if !pat_parts.is_empty() {
        let mut rel_parts: Vec<String> = Vec::new();
        glob_walk(dir, &mut rel_parts, &pat_parts, 0, sep, &mut matches)?;
    }
    Ok(matches)
}

fn raise_io_error_for_glob(_py: &PyToken<'_>, err: std::io::Error) -> u64 {
    let msg = err.to_string();
    match err.kind() {
        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
        ErrorKind::PermissionDenied => raise_exception::<_>(_py, "PermissionError", &msg),
        ErrorKind::NotADirectory => raise_exception::<_>(_py, "NotADirectoryError", &msg),
        _ => raise_exception::<_>(_py, "OSError", &msg),
    }
}

#[cfg(unix)]
fn create_symlink_path(
    src: &std::path::Path,
    dst: &std::path::Path,
    _target_is_directory: bool,
) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn create_symlink_path(
    src: &std::path::Path,
    dst: &std::path::Path,
    target_is_directory: bool,
) -> std::io::Result<()> {
    if target_is_directory {
        std::os::windows::fs::symlink_dir(src, dst)
    } else {
        std::os::windows::fs::symlink_file(src, dst)
    }
}

#[cfg(not(any(unix, windows)))]
fn create_symlink_path(
    _src: &std::path::Path,
    _dst: &std::path::Path,
    _target_is_directory: bool,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        ErrorKind::Unsupported,
        "symlink is not supported on this host",
    ))
}

fn path_splitroot_text(path: &str, sep: char) -> (String, String, String) {
    #[cfg(windows)]
    {
        let text = path.replace('/', "\\");
        if text.is_empty() {
            return (String::new(), String::new(), String::new());
        }
        let mut drive = String::new();
        let mut root = String::new();
        let mut rest = text.as_str();
        if rest.starts_with("\\\\") {
            let unc = &rest[2..];
            let mut parts = unc.split('\\');
            let server = parts.next().unwrap_or_default();
            let share = parts.next().unwrap_or_default();
            if !server.is_empty() && !share.is_empty() {
                drive = format!("\\\\{server}\\{share}");
                let consumed = 2 + server.len() + 1 + share.len();
                rest = &rest[consumed..];
            }
        } else {
            let bytes = rest.as_bytes();
            if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
                drive = rest[..2].to_string();
                rest = &rest[2..];
            }
        }
        if rest.starts_with('\\') {
            root = sep.to_string();
            rest = rest.trim_start_matches('\\');
        }
        return (drive, root, rest.to_string());
    }
    #[cfg(not(windows))]
    {
        if path.is_empty() {
            return (String::new(), String::new(), String::new());
        }
        if path.starts_with("//") && !path.starts_with("///") {
            let tail = path.trim_start_matches('/').to_string();
            return (String::new(), "//".to_string(), tail);
        }
        if path.starts_with(sep) {
            return (
                String::new(),
                sep.to_string(),
                path.trim_start_matches(sep).to_string(),
            );
        }
        (String::new(), String::new(), path.to_string())
    }
}

fn path_relpath_text(_py: &PyToken<'_>, path: &str, start: &str, sep: char) -> Result<String, u64> {
    if path.is_empty() {
        return Err(raise_exception::<_>(_py, "ValueError", "no path specified"));
    }
    let start_abs = path_abspath_text(_py, start, sep)?;
    let path_abs = path_abspath_text(_py, path, sep)?;
    let start_parts = start_abs
        .split(sep)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect::<Vec<_>>();
    let path_parts = path_abs
        .split(sep)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect::<Vec<_>>();
    let mut common = 0usize;
    let limit = start_parts.len().min(path_parts.len());
    while common < limit && start_parts[common] == path_parts[common] {
        common += 1;
    }
    let mut rel_parts: Vec<String> = Vec::new();
    for _ in common..start_parts.len() {
        rel_parts.push("..".to_string());
    }
    for part in &path_parts[common..] {
        rel_parts.push(part.clone());
    }
    if rel_parts.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(rel_parts.join(&sep.to_string()))
    }
}

fn path_relative_to_text(path: &str, base: &str, sep: char) -> Result<String, String> {
    let sep_s = sep.to_string();
    let target_parts = path_parts_text(path, sep);
    let base_parts = path_parts_text(base, sep);
    let target_abs = target_parts.first().is_some_and(|part| part == &sep_s);
    let base_abs = base_parts.first().is_some_and(|part| part == &sep_s);
    if (base_abs && !target_abs) || (!base_abs && target_abs) {
        return Err(format!("{path:?} is not in the subpath of {base:?}"));
    }
    if base_parts.len() > target_parts.len() {
        return Err(format!("{path:?} is not in the subpath of {base:?}"));
    }
    for (idx, part) in base_parts.iter().enumerate() {
        if target_parts.get(idx) != Some(part) {
            return Err(format!("{path:?} is not in the subpath of {base:?}"));
        }
    }
    let rel_parts = &target_parts[base_parts.len()..];
    if rel_parts.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(rel_parts.join(&sep_s))
    }
}

fn path_expandvars_with_lookup(
    path: &str,
    mut lookup: impl FnMut(&str) -> Option<String>,
) -> String {
    if !path.contains('$') {
        return path.to_string();
    }
    let is_var_char = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let chars: Vec<char> = path.chars().collect();
    let mut out = String::with_capacity(path.len());
    let mut idx = 0usize;
    while idx < chars.len() {
        let ch = chars[idx];
        if ch != '$' {
            out.push(ch);
            idx += 1;
            continue;
        }
        if idx + 1 >= chars.len() {
            out.push('$');
            idx += 1;
            continue;
        }
        let next = chars[idx + 1];
        if next == '{' {
            let mut end = idx + 2;
            while end < chars.len() && chars[end] != '}' {
                end += 1;
            }
            if end >= chars.len() {
                for c in &chars[idx..] {
                    out.push(*c);
                }
                break;
            }
            let name: String = chars[idx + 2..end].iter().collect();
            if name.is_empty() {
                for c in &chars[idx..=end] {
                    out.push(*c);
                }
            } else if let Some(value) = lookup(&name) {
                out.push_str(&value);
            } else {
                for c in &chars[idx..=end] {
                    out.push(*c);
                }
            }
            idx = end + 1;
            continue;
        }
        if next == '$' {
            out.push('$');
            out.push('$');
            idx += 2;
            continue;
        }
        let start = idx + 1;
        let mut end = start;
        while end < chars.len() && is_var_char(chars[end]) {
            end += 1;
        }
        if end == start {
            out.push('$');
            idx += 1;
            continue;
        }
        let name: String = chars[start..end].iter().collect();
        if let Some(value) = lookup(&name) {
            out.push_str(&value);
        } else {
            for c in &chars[idx..end] {
                out.push(*c);
            }
        }
        idx = end;
    }
    out
}

fn path_expandvars_text(_py: &PyToken<'_>, path: &str) -> Result<String, u64> {
    if !has_capability(_py, "env.read") {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing env.read capability",
        ));
    }
    Ok(path_expandvars_with_lookup(path, |name| {
        std::env::var(name).ok()
    }))
}

fn open_arg_path(_py: &PyToken<'_>, file_bits: u64) -> Result<(std::path::PathBuf, u64), String> {
    let obj = obj_from_bits(file_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        let name_ptr = alloc_string(_py, text.as_bytes());
        if name_ptr.is_null() {
            return Err("open failed".to_string());
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        return Ok((std::path::PathBuf::from(text), name_bits));
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let name_ptr = alloc_bytes(_py, bytes);
                if name_ptr.is_null() {
                    return Err("open failed".to_string());
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt;
                    let path = std::ffi::OsString::from_vec(bytes.to_vec());
                    return Ok((std::path::PathBuf::from(path), name_bits));
                }
                #[cfg(windows)]
                {
                    let path = std::str::from_utf8(bytes)
                        .map_err(|_| "open path bytes must be utf-8".to_string())?;
                    return Ok((std::path::PathBuf::from(path), name_bits));
                }
            }
            let fspath_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.fspath_name, b"__fspath__");
            if let Some(call_bits) = attr_lookup_ptr(_py, ptr, fspath_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return Err("open failed".to_string());
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(text) = string_obj_to_owned(res_obj) {
                    let name_ptr = alloc_string(_py, text.as_bytes());
                    if name_ptr.is_null() {
                        return Err("open failed".to_string());
                    }
                    let name_bits = MoltObject::from_ptr(name_ptr).bits();
                    dec_ref_bits(_py, res_bits);
                    return Ok((std::path::PathBuf::from(text), name_bits));
                }
                if let Some(res_ptr) = res_obj.as_ptr() {
                    if object_type_id(res_ptr) == TYPE_ID_BYTES {
                        let len = bytes_len(res_ptr);
                        let bytes = std::slice::from_raw_parts(bytes_data(res_ptr), len);
                        let name_ptr = alloc_bytes(_py, bytes);
                        if name_ptr.is_null() {
                            return Err("open failed".to_string());
                        }
                        let name_bits = MoltObject::from_ptr(name_ptr).bits();
                        #[cfg(unix)]
                        {
                            use std::os::unix::ffi::OsStringExt;
                            let path = std::ffi::OsString::from_vec(bytes.to_vec());
                            dec_ref_bits(_py, res_bits);
                            return Ok((std::path::PathBuf::from(path), name_bits));
                        }
                        #[cfg(windows)]
                        {
                            let path = std::str::from_utf8(bytes)
                                .map_err(|_| "open path bytes must be utf-8".to_string())?;
                            dec_ref_bits(_py, res_bits);
                            return Ok((std::path::PathBuf::from(path), name_bits));
                        }
                    }
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                dec_ref_bits(_py, res_bits);
                let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
                return Err(format!(
                    "expected {obj_type}.__fspath__() to return str or bytes, not {res_type}"
                ));
            }
        }
    }
    let obj_type = class_name_for_error(type_of_bits(_py, file_bits));
    Err(format!(
        "expected str, bytes or os.PathLike object, not {obj_type}"
    ))
}

#[allow(clippy::too_many_arguments)]
fn open_impl(
    _py: &PyToken<'_>,
    file_bits: u64,
    mode_bits: u64,
    buffering_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    struct BitsGuard<'a> {
        py: &'a PyToken<'a>,
        bits: u64,
    }
    impl<'a> Drop for BitsGuard<'a> {
        fn drop(&mut self) {
            if self.bits != 0 {
                dec_ref_bits(self.py, self.bits);
            }
        }
    }

    let mode_obj = obj_from_bits(mode_bits);
    if mode_obj.is_none() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "open() argument 'mode' must be str, not NoneType",
        );
    }
    let mode = match string_obj_to_owned(mode_obj) {
        Some(mode) => mode,
        None => {
            let type_name = class_name_for_error(type_of_bits(_py, mode_bits));
            let msg = format!("open() argument 'mode' must be str, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    };
    let mode_info = match parse_file_mode(&mode) {
        Ok(parsed) => parsed,
        Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
    };
    if mode_info.readable && !has_capability(_py, "fs.read") {
        return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
    }
    if mode_info.writable && !has_capability(_py, "fs.write") {
        return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
    }

    let buffering = {
        let obj = obj_from_bits(buffering_bits);
        if obj.is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "'NoneType' object cannot be interpreted as an integer",
            );
        }
        let type_name = class_name_for_error(type_of_bits(_py, buffering_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        index_i64_from_obj(_py, buffering_bits, &msg)
    };
    if buffering < -1 {
        return raise_exception::<_>(_py, "ValueError", "buffering must be >= -1");
    }
    let buffering = if buffering < 0 { -1 } else { buffering };
    let line_buffering = buffering == 1 && mode_info.text;
    if buffering == 0 && mode_info.text {
        return raise_exception::<_>(_py, "ValueError", "can't have unbuffered text I/O");
    }

    let encoding = if mode_info.text {
        open_arg_encoding(_py, encoding_bits)
    } else if !obj_from_bits(encoding_bits).is_none() {
        return raise_exception::<_>(
            _py,
            "ValueError",
            "binary mode doesn't take an encoding argument",
        );
    } else {
        None
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let errors = if mode_info.text {
        open_arg_errors(_py, errors_bits)
    } else if !obj_from_bits(errors_bits).is_none() {
        return raise_exception::<_>(
            _py,
            "ValueError",
            "binary mode doesn't take an errors argument",
        );
    } else {
        None
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let newline = if mode_info.text {
        open_arg_newline(_py, newline_bits)
    } else if !obj_from_bits(newline_bits).is_none() {
        return raise_exception::<_>(
            _py,
            "ValueError",
            "binary mode doesn't take a newline argument",
        );
    } else {
        None
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }

    let closefd = is_truthy(_py, obj_from_bits(closefd_bits));
    let opener_obj = obj_from_bits(opener_bits);
    let opener_is_none = opener_obj.is_none();

    let mut path_guard = BitsGuard { py: _py, bits: 0 };
    let mut path = None;
    let mut fd: Option<i64> = None;
    let path_name_bits = if let Some(i) = to_i64(obj_from_bits(file_bits)) {
        fd = Some(i);
        let bits = MoltObject::from_int(i).bits();
        path_guard.bits = bits;
        bits
    } else {
        match open_arg_path(_py, file_bits) {
            Ok((resolved, name_bits)) => {
                if !closefd {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "Cannot use closefd=False with file name",
                    );
                }
                path = Some(resolved);
                path_guard.bits = name_bits;
                name_bits
            }
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        }
    };

    let mut file = None;
    #[cfg(windows)]
    let mut crt_fd: Option<i64> = None;
    if let Some(fd_val) = fd {
        if !opener_is_none {
            return raise_exception::<_>(_py, "ValueError", "opener only works with file path");
        }
        let effective_fd = if closefd {
            fd_val
        } else {
            match dup_fd(fd_val) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(_py, "OSError", "open failed");
                }
            }
        };
        if let Some(handle) = file_from_fd(effective_fd) {
            file = Some(handle);
            #[cfg(windows)]
            {
                crt_fd = Some(effective_fd);
            }
        } else {
            return raise_exception::<_>(_py, "OSError", "open failed");
        }
    } else if let Some(path) = path {
        let flags = file_mode_to_flags(&mode_info);
        if !opener_is_none {
            if !is_truthy(_py, obj_from_bits(molt_is_callable(opener_bits))) {
                let type_name = class_name_for_error(type_of_bits(_py, opener_bits));
                let msg = format!("'{type_name}' object is not callable");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let path_bits = path_name_bits;
            let flags_bits = MoltObject::from_int(flags as i64).bits();
            let fd_bits = unsafe { call_callable2(_py, opener_bits, path_bits, flags_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Some(fd_val) = to_i64(obj_from_bits(fd_bits)) {
                if let Some(handle) = file_from_fd(fd_val) {
                    file = Some(handle);
                } else {
                    return raise_exception::<_>(_py, "OSError", "open failed");
                }
            } else {
                let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
                let msg = format!("expected opener to return int, got {type_name}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            dec_ref_bits(_py, fd_bits);
        } else {
            file = match mode_info.options.open(&path) {
                Ok(file) => Some(file),
                Err(err) => {
                    let short = match err.kind() {
                        ErrorKind::NotFound => "No such file or directory".to_string(),
                        ErrorKind::PermissionDenied => "Permission denied".to_string(),
                        ErrorKind::AlreadyExists => "File exists".to_string(),
                        ErrorKind::InvalidInput => "Invalid argument".to_string(),
                        ErrorKind::IsADirectory => "Is a directory".to_string(),
                        ErrorKind::NotADirectory => "Not a directory".to_string(),
                        _ => err.to_string(),
                    };
                    let path_display = path.to_string_lossy();
                    let msg = if let Some(code) = err.raw_os_error() {
                        format!("[Errno {code}] {short}: '{path_display}'")
                    } else {
                        format!("{short}: '{path_display}'")
                    };
                    match err.kind() {
                        ErrorKind::AlreadyExists => {
                            return raise_exception::<_>(_py, "FileExistsError", &msg);
                        }
                        ErrorKind::NotFound => {
                            return raise_exception::<_>(_py, "FileNotFoundError", &msg);
                        }
                        ErrorKind::PermissionDenied => {
                            return raise_exception::<_>(_py, "PermissionError", &msg);
                        }
                        ErrorKind::IsADirectory => {
                            return raise_exception::<_>(_py, "IsADirectoryError", &msg);
                        }
                        ErrorKind::NotADirectory => {
                            return raise_exception::<_>(_py, "NotADirectoryError", &msg);
                        }
                        _ => return raise_exception::<_>(_py, "OSError", &msg),
                    }
                }
            };
        }
    }
    #[cfg(windows)]
    if crt_fd.is_none() {
        if let Some(file_ref) = file.as_ref() {
            use std::os::windows::io::AsRawHandle;
            let handle = file_ref.as_raw_handle();
            crt_fd = windows_crt_fd_from_handle(
                handle as *mut std::ffi::c_void,
                mode_info.readable,
                mode_info.writable,
            );
        }
    }
    let Some(file) = file else {
        return raise_exception::<_>(_py, "OSError", "open failed");
    };

    // Keep text-I/O encoding normalization explicit so open()/TextIOWrapper
    // remains deterministic across native and wasm builds.
    let encoding = if mode_info.text {
        let encoding = encoding.unwrap_or_else(|| "utf-8".to_string());
        let (label, _kind) = match normalize_text_encoding(&encoding) {
            Ok(val) => val,
            Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
        };
        Some(label)
    } else {
        None
    };
    let errors = if mode_info.text {
        Some(errors.unwrap_or_else(|| "strict".to_string()))
    } else {
        None
    };

    let encoding_original = encoding.clone();
    let state = Arc::new(MoltFileState {
        backend: Mutex::new(Some(MoltFileBackend::File(file))),
        #[cfg(windows)]
        crt_fd: Mutex::new(crt_fd),
    });
    let builtins = builtin_classes(_py);
    let buffered_class_bits = if mode_info.readable && mode_info.writable {
        builtins.buffered_random
    } else if mode_info.writable {
        builtins.buffered_writer
    } else {
        builtins.buffered_reader
    };
    let binary_class_bits = if buffering == 0 {
        builtins.file_io
    } else {
        buffered_class_bits
    };
    let handle_class_bits = if mode_info.text {
        builtins.text_io_wrapper
    } else {
        binary_class_bits
    };
    let buffer_class_bits = if mode_info.text {
        buffered_class_bits
    } else {
        0
    };
    let buffer_size = if buffering == 0 {
        0
    } else if line_buffering || buffering < 0 {
        DEFAULT_BUFFER_SIZE
    } else {
        buffering
    };
    let buffer_bits = if mode_info.text {
        let buffer_ptr = alloc_file_handle_with_state(
            _py,
            Arc::clone(&state),
            mode_info.readable,
            mode_info.writable,
            false,
            false,
            true,
            false,
            false,
            buffer_size,
            buffer_class_bits,
            path_name_bits,
            mode.clone(),
            None,
            None,
            None,
            None,
            0,
            0,
        );
        if buffer_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(buffer_ptr).bits()
    } else {
        0
    };
    let ptr = alloc_file_handle_with_state(
        _py,
        state,
        mode_info.readable,
        mode_info.writable,
        mode_info.text,
        closefd,
        true,
        line_buffering,
        false,
        buffer_size,
        handle_class_bits,
        path_name_bits,
        mode,
        encoding,
        encoding_original,
        errors,
        newline,
        buffer_bits,
        0,
    );
    if buffer_bits != 0 {
        dec_ref_bits(_py, buffer_bits);
    }
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn alloc_stdio_handle(
    _py: &PyToken<'_>,
    fd: i64,
    readable: bool,
    writable: bool,
    name: &str,
    errors: &str,
    write_through: bool,
) -> u64 {
    let effective_fd = if cfg!(target_arch = "wasm32") {
        fd
    } else {
        match dup_fd(fd) {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        }
    };
    let Some(file) = file_from_fd(effective_fd) else {
        return MoltObject::none().bits();
    };
    let mode = if readable && writable {
        "r+"
    } else if readable {
        "r"
    } else {
        "w"
    };
    let mode_info = match parse_file_mode(mode) {
        Ok(parsed) => parsed,
        Err(_) => return MoltObject::none().bits(),
    };
    let buffering = -1;
    let line_buffering = if writable { stdio_isatty(fd) } else { false };
    let buffer_size = if buffering == 0 {
        0
    } else if line_buffering || buffering < 0 {
        DEFAULT_BUFFER_SIZE
    } else {
        buffering
    };

    let state = Arc::new(MoltFileState {
        backend: Mutex::new(Some(MoltFileBackend::File(file))),
        #[cfg(windows)]
        crt_fd: Mutex::new(Some(effective_fd)),
    });
    let builtins = builtin_classes(_py);
    let buffered_class_bits = if mode_info.readable && mode_info.writable {
        builtins.buffered_random
    } else if mode_info.writable {
        builtins.buffered_writer
    } else {
        builtins.buffered_reader
    };
    let binary_class_bits = if buffering == 0 {
        builtins.file_io
    } else {
        buffered_class_bits
    };
    let handle_class_bits = if mode_info.text {
        builtins.text_io_wrapper
    } else {
        binary_class_bits
    };
    let buffer_class_bits = if mode_info.text {
        buffered_class_bits
    } else {
        0
    };
    let name_ptr = alloc_string(_py, name.as_bytes());
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let mode_string = mode.to_string();
    let buffer_bits = if mode_info.text {
        let buffer_ptr = alloc_file_handle_with_state(
            _py,
            Arc::clone(&state),
            mode_info.readable,
            mode_info.writable,
            false,
            false,
            true,
            false,
            false,
            buffer_size,
            buffer_class_bits,
            name_bits,
            mode_string.clone(),
            None,
            None,
            None,
            None,
            0,
            0,
        );
        if buffer_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(buffer_ptr).bits()
    } else {
        0
    };
    let ptr = alloc_file_handle_with_state(
        _py,
        state,
        mode_info.readable,
        mode_info.writable,
        mode_info.text,
        true,
        true,
        line_buffering,
        write_through,
        buffer_size,
        handle_class_bits,
        name_bits,
        mode_string,
        Some("utf-8".to_string()),
        Some("utf-8".to_string()),
        Some(errors.to_string()),
        None,
        buffer_bits,
        0,
    );
    if buffer_bits != 0 {
        dec_ref_bits(_py, buffer_bits);
    }
    dec_ref_bits(_py, name_bits);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

fn cached_stdio_handle(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    make_handle: impl FnOnce() -> u64,
) -> u64 {
    let cached_bits = slot.load(Ordering::Acquire);
    if cached_bits != 0 && !obj_from_bits(cached_bits).is_none() {
        inc_ref_bits(_py, cached_bits);
        return cached_bits;
    }

    let handle_bits = make_handle();
    if obj_from_bits(handle_bits).is_none() {
        return handle_bits;
    }

    // Keep one pinned reference so repeated sys stdio lookups share the same
    // handle object instead of allocating/closing duplicate descriptors.
    inc_ref_bits(_py, handle_bits);
    let prev = slot.swap(handle_bits, Ordering::AcqRel);
    if prev != 0 && prev != handle_bits && !obj_from_bits(prev).is_none() {
        dec_ref_bits(_py, prev);
    }
    handle_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stdin() -> u64 {
    crate::with_gil_entry!(_py, {
        cached_stdio_handle(_py, &SYS_STDIN_HANDLE_BITS, || {
            alloc_stdio_handle(_py, 0, true, false, "<stdin>", "surrogateescape", false)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stdout() -> u64 {
    crate::with_gil_entry!(_py, {
        cached_stdio_handle(_py, &SYS_STDOUT_HANDLE_BITS, || {
            alloc_stdio_handle(_py, 1, false, true, "<stdout>", "surrogateescape", false)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stderr() -> u64 {
    crate::with_gil_entry!(_py, {
        cached_stdio_handle(_py, &SYS_STDERR_HANDLE_BITS, || {
            alloc_stdio_handle(_py, 2, false, true, "<stderr>", "backslashreplace", true)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_open(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none = MoltObject::none().bits();
        open_impl(
            _py,
            path_bits,
            mode_bits,
            MoltObject::from_int(-1).bits(),
            none,
            none,
            none,
            MoltObject::from_bool(true).bits(),
            none,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_open_ex(
    file_bits: u64,
    mode_bits: u64,
    buffering_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        open_impl(
            _py,
            file_bits,
            mode_bits,
            buffering_bits,
            encoding_bits,
            errors_bits,
            newline_bits,
            closefd_bits,
            opener_bits,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_io_class(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_obj_to_owned(obj_from_bits(name_bits)) {
            Some(name) => name,
            None => return raise_exception::<_>(_py, "TypeError", "io class name must be str"),
        };
        let builtins = builtin_classes(_py);
        let bits = match name.as_str() {
            "IOBase" => builtins.io_base,
            "RawIOBase" => builtins.raw_io_base,
            "BufferedIOBase" => builtins.buffered_io_base,
            "TextIOBase" => builtins.text_io_base,
            "FileIO" => builtins.file_io,
            "BufferedReader" => builtins.buffered_reader,
            "BufferedWriter" => builtins.buffered_writer,
            "BufferedRandom" => builtins.buffered_random,
            "TextIOWrapper" => builtins.text_io_wrapper,
            "BytesIO" => builtins.bytes_io,
            "StringIO" => builtins.string_io,
            _ => {
                let msg = format!("unknown io class '{name}'");
                return raise_exception::<_>(_py, "AttributeError", &msg);
            }
        };
        inc_ref_bits(_py, bits);
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_io_new(
    _cls_bits: u64,
    name_bits: u64,
    mode_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode_obj = obj_from_bits(mode_bits);
        let mut mode = if mode_obj.is_none() {
            "r".to_string()
        } else if let Some(mode) = string_obj_to_owned(mode_obj) {
            mode
        } else {
            let type_name = class_name_for_error(type_of_bits(_py, mode_bits));
            let msg = format!("FileIO() argument 'mode' must be str, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if mode.contains('t') {
            return raise_exception::<_>(_py, "ValueError", "FileIO() doesn't take text mode");
        }
        if !mode.contains('b') {
            mode.push('b');
        }
        let mode_ptr = alloc_string(_py, mode.as_bytes());
        if mode_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mode_bits = MoltObject::from_ptr(mode_ptr).bits();
        let buffering_bits = MoltObject::from_int(0).bits();
        let none = MoltObject::none().bits();
        let closefd_bits = if obj_from_bits(closefd_bits).is_none() {
            MoltObject::from_bool(true).bits()
        } else {
            closefd_bits
        };
        let out = open_impl(
            _py,
            name_bits,
            mode_bits,
            buffering_bits,
            none,
            none,
            none,
            closefd_bits,
            opener_bits,
        );
        dec_ref_bits(_py, mode_bits);
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_io_init(
    _self_bits: u64,
    _name_bits: u64,
    _mode_bits: u64,
    _closefd_bits: u64,
    _opener_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffered_new(cls_bits: u64, raw_bits: u64, buffer_size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let raw_handle_ptr = match resolve_file_handle_ptr(_py, raw_bits) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        unsafe {
            let raw_handle = &mut *raw_handle_ptr;
            file_handle_require_attached!(_py, raw_handle);
            if file_handle_is_closed(raw_handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if raw_handle.text {
                return raise_exception::<_>(_py, "ValueError", "raw stream must be binary");
            }
            let size_obj = obj_from_bits(buffer_size_bits);
            let mut buffer_size = if size_obj.is_none() {
                DEFAULT_BUFFER_SIZE
            } else {
                match to_i64(size_obj) {
                    Some(val) => val,
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, buffer_size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            if buffer_size < 0 {
                buffer_size = DEFAULT_BUFFER_SIZE;
            }
            if buffer_size == 0 {
                return raise_exception::<_>(_py, "ValueError", "buffer size must be > 0");
            }
            let builtins = builtin_classes(_py);
            let (want_read, want_write) = if cls_bits == builtins.buffered_reader {
                (true, false)
            } else if cls_bits == builtins.buffered_writer {
                (false, true)
            } else {
                (true, true)
            };
            if want_read && !raw_handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            if want_write && !raw_handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not writable");
            }
            let ptr = alloc_file_handle_with_state(
                _py,
                Arc::clone(&raw_handle.state),
                want_read,
                want_write,
                false,
                raw_handle.closefd,
                raw_handle.owns_fd,
                false,
                false,
                buffer_size,
                cls_bits,
                raw_handle.name_bits,
                raw_handle.mode.clone(),
                None,
                None,
                None,
                None,
                0,
                raw_handle.mem_bits,
            );
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffered_init(
    _self_bits: u64,
    _raw_bits: u64,
    _buffer_size_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_text_io_wrapper_new(
    _cls_bits: u64,
    buffer_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    line_buffering_bits: u64,
    write_through_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let buffer_handle_ptr = match resolve_file_handle_ptr(_py, buffer_bits) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        unsafe {
            let buffer_handle = &mut *buffer_handle_ptr;
            file_handle_require_attached!(_py, buffer_handle);
            if file_handle_is_closed(buffer_handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if buffer_handle.text {
                return raise_exception::<_>(_py, "ValueError", "buffer must be binary");
            }
            let encoding = if obj_from_bits(encoding_bits).is_none() {
                Some("utf-8".to_string())
            } else {
                let encoding = reconfigure_arg_type(_py, encoding_bits, "encoding");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                encoding
            };
            let encoding = if let Some(label) = encoding {
                let (label, _kind) = match normalize_text_encoding(&label) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
                };
                Some(label)
            } else {
                None
            };
            let encoding_original = encoding.clone();
            let errors = if obj_from_bits(errors_bits).is_none() {
                Some("strict".to_string())
            } else {
                reconfigure_arg_type(_py, errors_bits, "errors")
            };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let newline = if obj_from_bits(newline_bits).is_none() {
                None
            } else {
                open_arg_newline(_py, newline_bits)
            };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let line_buffering = if obj_from_bits(line_buffering_bits).is_none() {
                false
            } else {
                is_truthy(_py, obj_from_bits(line_buffering_bits))
            };
            let write_through = if obj_from_bits(write_through_bits).is_none() {
                false
            } else {
                is_truthy(_py, obj_from_bits(write_through_bits))
            };
            let ptr = alloc_file_handle_with_state(
                _py,
                Arc::clone(&buffer_handle.state),
                buffer_handle.readable,
                buffer_handle.writable,
                true,
                buffer_handle.closefd,
                buffer_handle.owns_fd,
                line_buffering,
                write_through,
                buffer_handle.buffer_size,
                builtin_classes(_py).text_io_wrapper,
                buffer_handle.name_bits,
                buffer_handle.mode.clone(),
                encoding,
                encoding_original,
                errors,
                newline,
                buffer_bits,
                buffer_handle.mem_bits,
            );
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_text_io_wrapper_init(
    _self_bits: u64,
    _buffer_bits: u64,
    _encoding_bits: u64,
    _errors_bits: u64,
    _newline_bits: u64,
    _line_buffering_bits: u64,
    _write_through_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytesio_new(_cls_bits: u64, initial_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let payload = match unsafe { collect_bytes_like(_py, initial_bits) } {
            Ok(payload) => payload,
            Err(bits) => return bits,
        };
        let bytearray_ptr = alloc_bytearray(_py, &payload);
        if bytearray_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mem_bits = MoltObject::from_ptr(bytearray_ptr).bits();
        let state = Arc::new(MoltFileState {
            backend: Mutex::new(Some(MoltFileBackend::Memory(MoltMemoryBackend { pos: 0 }))),
            #[cfg(windows)]
            crt_fd: Mutex::new(None),
        });
        let ptr = alloc_file_handle_with_state(
            _py,
            state,
            true,
            true,
            false,
            true,
            true,
            false,
            false,
            0,
            builtin_classes(_py).bytes_io,
            0,
            "rb+".to_string(),
            None,
            None,
            None,
            None,
            0,
            mem_bits,
        );
        dec_ref_bits(_py, mem_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytesio_init(_self_bits: u64, _initial_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stringio_new(_cls_bits: u64, initial_bits: u64, newline_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let initial = if obj_from_bits(initial_bits).is_none() {
            String::new()
        } else if let Some(text) = string_obj_to_owned(obj_from_bits(initial_bits)) {
            text
        } else {
            let type_name = class_name_for_error(type_of_bits(_py, initial_bits));
            let msg = format!("initial_value must be str, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let newline = match open_arg_newline(_py, newline_bits) {
            Some(val) => Some(val),
            None => Some("\n".to_string()),
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let state = Arc::new(MoltFileState {
            backend: Mutex::new(Some(MoltFileBackend::Text(MoltTextBackend {
                data: initial.chars().collect(),
                pos: 0,
            }))),
            #[cfg(windows)]
            crt_fd: Mutex::new(None),
        });
        let ptr = alloc_file_handle_with_state(
            _py,
            state,
            true,
            true,
            true,
            true,
            true,
            false,
            false,
            0,
            builtin_classes(_py).string_io,
            0,
            "r+".to_string(),
            None,
            None,
            None,
            newline,
            0,
            0,
        );
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stringio_init(
    _self_bits: u64,
    _initial_bits: u64,
    _newline_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_exists(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        MoltObject::from_bool(std::fs::metadata(path).is_ok()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let is_dir = std::fs::metadata(path)
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        MoltObject::from_bool(is_dir).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isfile(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let is_file = std::fs::metadata(path)
            .map(|meta| meta.is_file())
            .unwrap_or(false);
        MoltObject::from_bool(is_file).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_islink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let is_link = std::fs::symlink_metadata(path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        MoltObject::from_bool(is_link).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_readlink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::read_link(&path) {
            Ok(target) => {
                let text = target.to_string_lossy();
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_symlink(
    src_bits: u64,
    dst_bits: u64,
    target_is_directory_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let src = match path_from_bits(_py, src_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let dst = match path_from_bits(_py, dst_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let target_is_directory = is_truthy(_py, obj_from_bits(target_is_directory_bits));
        match create_symlink_path(&src, &dst, target_is_directory) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => raise_exception::<_>(_py, "FileExistsError", &msg),
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::Unsupported => {
                        raise_exception::<_>(_py, "NotImplementedError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_listdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mut entries: Vec<u64> = Vec::new();
        let read_dir = match std::fs::read_dir(&path) {
            Ok(dir) => dir,
            Err(err) => {
                let msg = err.to_string();
                return match err.kind() {
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::NotADirectory => {
                        raise_exception::<_>(_py, "NotADirectoryError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                };
            }
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    let msg = err.to_string();
                    return raise_exception::<_>(_py, "OSError", &msg);
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                for bits in entries {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            entries.push(MoltObject::from_ptr(name_ptr).bits());
        }
        let list_ptr = alloc_list(_py, entries.as_slice());
        if list_ptr.is_null() {
            for bits in entries {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        for bits in entries {
            dec_ref_bits(_py, bits);
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_mkdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::create_dir(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => raise_exception::<_>(_py, "FileExistsError", &msg),
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_unlink(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::remove_file(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::IsADirectory => raise_exception::<_>(_py, "IsADirectoryError", &msg),
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_rmdir(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        match std::fs::remove_dir(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::DirectoryNotEmpty => raise_exception::<_>(_py, "OSError", &msg),
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_join(base_bits: u64, part_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let part = match path_string_from_bits(_py, part_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_join_text(base, &part, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_join_many(base_bits: u64, parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let parts = match path_sequence_from_bits(_py, parts_bits, "parts") {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let out = path_join_many_text(base, &parts, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_isabs(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(path_isabs_text(&path, sep)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_dirname(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_dirname_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_basename(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_basename_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_split(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let head = path_dirname_text(&path, sep);
        let tail = path_basename_text(&path, sep);
        let head_ptr = alloc_string(_py, head.as_bytes());
        if head_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let head_bits = MoltObject::from_ptr(head_ptr).bits();
        let tail_ptr = alloc_string(_py, tail.as_bytes());
        if tail_ptr.is_null() {
            dec_ref_bits(_py, head_bits);
            return MoltObject::none().bits();
        }
        let tail_bits = MoltObject::from_ptr(tail_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[head_bits, tail_bits]);
        dec_ref_bits(_py, head_bits);
        dec_ref_bits(_py, tail_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_splitext(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let (root, ext) = path_splitext_text(&path, sep);
        let root_ptr = alloc_string(_py, root.as_bytes());
        if root_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let root_bits = MoltObject::from_ptr(root_ptr).bits();
        let ext_ptr = alloc_string(_py, ext.as_bytes());
        if ext_ptr.is_null() {
            dec_ref_bits(_py, root_bits);
            return MoltObject::none().bits();
        }
        let ext_bits = MoltObject::from_ptr(ext_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[root_bits, ext_bits]);
        dec_ref_bits(_py, root_bits);
        dec_ref_bits(_py, ext_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_normpath(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_normpath_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_abspath(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_abspath_text(_py, &path, sep) {
            Ok(out) => out,
            Err(bits) => return bits,
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_resolve(path_bits: u64, strict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let strict = is_truthy(_py, obj_from_bits(strict_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let out = match path_resolve_text(_py, &path, sep, strict) {
            Ok(out) => out,
            Err(bits) => return bits,
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_relpath(path_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let start = if obj_from_bits(start_bits).is_none() {
            ".".to_string()
        } else {
            match path_string_from_bits(_py, start_bits) {
                Ok(path) => path,
                Err(bits) => return bits,
            }
        };
        let out = match path_relpath_text(_py, &path, &start, sep) {
            Ok(out) => out,
            Err(bits) => return bits,
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_expandvars(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_expandvars_text(_py, &path) {
            Ok(out) => out,
            Err(bits) => return bits,
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_expandvars_env(path_bits: u64, env_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let Some(env_ptr) = obj_from_bits(env_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "env must be dict[str, str]");
        };
        if unsafe { object_type_id(env_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "env must be dict[str, str]");
        }
        let mut env_map: HashMap<String, String> = HashMap::new();
        let pairs = unsafe { dict_order(env_ptr) };
        for chunk in pairs.chunks(2) {
            if chunk.len() < 2 {
                continue;
            }
            let Some(key) = string_obj_to_owned(obj_from_bits(chunk[0])) else {
                return raise_exception::<_>(_py, "TypeError", "env keys must be str");
            };
            let Some(value) = string_obj_to_owned(obj_from_bits(chunk[1])) else {
                return raise_exception::<_>(_py, "TypeError", "env values must be str");
            };
            env_map.insert(key, value);
        }
        let out = path_expandvars_with_lookup(&path, |name| env_map.get(name).cloned());
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_makedirs(path_bits: u64, exist_ok_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        if path.as_os_str().is_empty() {
            return MoltObject::none().bits();
        }
        let exist_ok = is_truthy(_py, obj_from_bits(exist_ok_bits));
        match std::fs::metadata(&path) {
            Ok(meta) => {
                if meta.is_dir() {
                    if exist_ok {
                        return MoltObject::none().bits();
                    }
                    let msg = format!("File exists: {}", path.to_string_lossy());
                    return raise_exception::<_>(_py, "FileExistsError", &msg);
                }
                let msg = format!("File exists: {}", path.to_string_lossy());
                return raise_exception::<_>(_py, "FileExistsError", &msg);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => {
                let msg = err.to_string();
                return match err.kind() {
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                };
            }
        }
        match std::fs::create_dir_all(&path) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::AlreadyExists => {
                        if exist_ok {
                            MoltObject::none().bits()
                        } else {
                            raise_exception::<_>(_py, "FileExistsError", &msg)
                        }
                    }
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_parts(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let parts = path_parts_text(&path, sep);
        let mut out_bits: Vec<u64> = Vec::with_capacity(parts.len());
        for part in parts {
            let ptr = alloc_string(_py, part.as_bytes());
            if ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, out_bits.as_slice());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_splitroot(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let (drive, root, tail) = path_splitroot_text(&path, sep);
        let drive_ptr = alloc_string(_py, drive.as_bytes());
        if drive_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let root_ptr = alloc_string(_py, root.as_bytes());
        if root_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(drive_ptr).bits());
            return MoltObject::none().bits();
        }
        let tail_ptr = alloc_string(_py, tail.as_bytes());
        if tail_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(drive_ptr).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(root_ptr).bits());
            return MoltObject::none().bits();
        }
        let drive_bits = MoltObject::from_ptr(drive_ptr).bits();
        let root_bits = MoltObject::from_ptr(root_ptr).bits();
        let tail_bits = MoltObject::from_ptr(tail_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[drive_bits, root_bits, tail_bits]);
        dec_ref_bits(_py, drive_bits);
        dec_ref_bits(_py, root_bits);
        dec_ref_bits(_py, tail_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_compare(lhs_bits: u64, rhs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let lhs = match path_string_from_bits(_py, lhs_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let rhs = match path_string_from_bits(_py, rhs_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        MoltObject::from_int(path_compare_text(&lhs, &rhs, sep)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_parents(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let parents = path_parents_text(&path, sep);
        let mut out_bits: Vec<u64> = Vec::with_capacity(parents.len());
        for parent in parents {
            let ptr = alloc_string(_py, parent.as_bytes());
            if ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, out_bits.as_slice());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_relative_to(path_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_relative_to_text(&path, &base, sep) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_relative_to_many(
    path_bits: u64,
    base_bits: u64,
    parts_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let parts = match path_sequence_from_bits(_py, parts_bits, "parts") {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let joined_base = path_join_many_text(base, &parts, sep);
        let out = match path_relative_to_text(&path, &joined_base, sep) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_with_name(path_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let name = match path_str_arg_from_bits(_py, name_bits, "name") {
            Ok(name) => name,
            Err(bits) => return bits,
        };
        #[cfg(windows)]
        let invalid_sep = name.contains('/') || name.contains('\\');
        #[cfg(not(windows))]
        let invalid_sep = name.contains(sep);
        if name.is_empty() || name == "." || invalid_sep {
            let msg = format!("Invalid name {name:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let current = path_basename_text(&path, sep);
        if current.is_empty() || current == "." {
            let msg = format!("{path:?} has an empty name");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let parent = path_dirname_text(&path, sep);
        let out = if parent.is_empty() || parent == "." {
            name
        } else {
            path_join_text(parent, &name, sep)
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_with_suffix(path_bits: u64, suffix_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let suffix = match path_str_arg_from_bits(_py, suffix_bits, "suffix") {
            Ok(suffix) => suffix,
            Err(bits) => return bits,
        };
        if !suffix.is_empty() && !suffix.starts_with('.') {
            let msg = format!("Invalid suffix {suffix:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let name = path_basename_text(&path, sep);
        let (stem, _) = path_splitext_text(&name, sep);
        if stem.is_empty() {
            let msg = format!("{path:?} has an empty name");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let new_name = format!("{stem}{suffix}");
        let parent = path_dirname_text(&path, sep);
        let out = if parent.is_empty() || parent == "." {
            new_name
        } else {
            path_join_text(parent, &new_name, sep)
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_with_stem(path_bits: u64, stem_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let stem = match path_str_arg_from_bits(_py, stem_bits, "stem") {
            Ok(stem) => stem,
            Err(bits) => return bits,
        };
        let name = path_basename_text(&path, sep);
        if name.is_empty() || name == "." {
            let msg = format!("{path:?} has an empty name");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let suffix = path_suffix_text(&name, sep);
        let new_name = format!("{stem}{suffix}");
        #[cfg(windows)]
        let invalid_sep = new_name.contains('/') || new_name.contains('\\');
        #[cfg(not(windows))]
        let invalid_sep = new_name.contains(sep);
        if new_name.is_empty() || new_name == "." || invalid_sep {
            let msg = format!("Invalid name {new_name:?}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let parent = path_dirname_text(&path, sep);
        let out = if parent.is_empty() || parent == "." {
            new_name
        } else {
            path_join_text(parent, &new_name, sep)
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_is_relative_to(path_bits: u64, base_bits: u64, parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let base = match path_string_from_bits(_py, base_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let target_base = if obj_from_bits(parts_bits).is_none() {
            base
        } else {
            let parts = match path_sequence_from_bits(_py, parts_bits, "parts") {
                Ok(parts) => parts,
                Err(bits) => return bits,
            };
            path_join_many_text(base, &parts, sep)
        };
        let is_relative = path_relative_to_text(&path, &target_base, sep).is_ok();
        MoltObject::from_bool(is_relative).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_expanduser(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        if !path.starts_with('~') {
            let ptr = alloc_string(_py, path.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        let rest = if path == "~" {
            ""
        } else if path.starts_with(&format!("~{sep}")) {
            &path[2..]
        } else {
            let ptr = alloc_string(_py, path.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        };
        if !has_capability(_py, "env.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing env.read capability");
        }
        let mut home = std::env::var("HOME").ok();
        if home.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
            home = std::env::var("USERPROFILE").ok();
        }
        if home.as_ref().map(|v| v.is_empty()).unwrap_or(true) {
            let drive = std::env::var("HOMEDRIVE").ok();
            let homepath = std::env::var("HOMEPATH").ok();
            if let (Some(drive), Some(homepath)) = (drive, homepath) {
                if !drive.is_empty() && !homepath.is_empty() {
                    home = Some(format!("{drive}{homepath}"));
                }
            }
        }
        let out = if let Some(mut home) = home {
            if !rest.is_empty() {
                home = home.trim_end_matches(sep).to_string();
                home.push(sep);
                home.push_str(rest);
            }
            home
        } else {
            path
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_name(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_name_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_suffix(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_suffix_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_stem(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = path_stem_text(&path, sep);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_suffixes(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let suffixes = path_suffixes_text(&path, sep);
        let mut out_bits: Vec<u64> = Vec::with_capacity(suffixes.len());
        for suffix in suffixes {
            let ptr = alloc_string(_py, suffix.as_bytes());
            if ptr.is_null() {
                for bits in out_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            out_bits.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, out_bits.as_slice());
        for bits in out_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_as_uri(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let out = match path_as_uri_text(&path, sep) {
            Ok(out) => out,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_match(path_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let path = match path_string_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        let pattern = match path_str_arg_from_bits(_py, pattern_bits, "pattern") {
            Ok(pattern) => pattern,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(path_match_text(&path, &pattern, sep)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_glob(path_bits: u64, pattern_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let dir = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let pattern = match path_str_arg_from_bits(_py, pattern_bits, "pattern") {
            Ok(pattern) => pattern,
            Err(bits) => return bits,
        };
        let sep = path_sep_char();
        let matches = match path_glob_matches(&dir, &pattern, sep) {
            Ok(values) => values,
            Err(err) => return raise_io_error_for_glob(_py, err),
        };
        alloc_string_list_bits(_py, &matches)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_has_magic(pathname_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let pathname = match path_string_from_bits(_py, pathname_bits) {
            Ok(path) => path,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(glob_has_magic_text(&pathname)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_escape(pathname_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = path_sep_char();
        let (pathname, flavor) = match path_string_with_flavor_from_bits(_py, pathname_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let escaped = glob_escape_text(&pathname, sep);
        if flavor == PathFlavor::Bytes {
            let raw = raw_from_bytes_text(&escaped).unwrap_or_else(|| escaped.as_bytes().to_vec());
            let ptr = alloc_bytes(_py, raw.as_slice());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        } else {
            let ptr = alloc_string(_py, escaped.as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob_translate(
    pathname_bits: u64,
    recursive_bits: u64,
    include_hidden_bits: u64,
    seps_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let pattern = if let Some(text) = string_obj_to_owned(obj_from_bits(pathname_bits)) {
            text
        } else {
            let type_id = obj_from_bits(pathname_bits)
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) });
            if matches!(type_id, Some(TYPE_ID_BYTES) | Some(TYPE_ID_BYTEARRAY)) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cannot use a string pattern on a bytes-like object",
                );
            }
            let type_name = class_name_for_error(type_of_bits(_py, pathname_bits));
            let msg = format!("expected string or bytes-like object, got '{type_name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };

        let recursive = is_truthy(_py, obj_from_bits(recursive_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let include_hidden = is_truthy(_py, obj_from_bits(include_hidden_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let seps = if obj_from_bits(seps_bits).is_none() {
            None
        } else if let Some(text) = string_obj_to_owned(obj_from_bits(seps_bits)) {
            Some(text)
        } else {
            return raise_exception::<_>(_py, "TypeError", "seps must be str or None");
        };

        let out = glob_translate_text(&pattern, recursive, include_hidden, seps.as_deref());
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_glob(
    pathname_bits: u64,
    root_dir_bits: u64,
    dir_fd_bits: u64,
    recursive_bits: u64,
    include_hidden_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let sep = path_sep_char();

        #[cfg(windows)]
        let (pathname, pathname_flavor) =
            match path_string_with_flavor_from_bits(_py, pathname_bits) {
                Ok((path, flavor)) => (path.replace('/', "\\"), flavor),
                Err(bits) => return bits,
            };
        #[cfg(not(windows))]
        let (pathname, pathname_flavor) =
            match path_string_with_flavor_from_bits(_py, pathname_bits) {
                Ok((path, flavor)) => (path, flavor),
                Err(bits) => return bits,
            };

        let root_dir = if obj_from_bits(root_dir_bits).is_none() {
            None
        } else {
            #[cfg(windows)]
            {
                match path_string_with_flavor_from_bits(_py, root_dir_bits) {
                    Ok((path, flavor)) => Some((path.replace('/', "\\"), flavor)),
                    Err(bits) => return bits,
                }
            }
            #[cfg(not(windows))]
            {
                match path_string_with_flavor_from_bits(_py, root_dir_bits) {
                    Ok((path, flavor)) => Some((path, flavor)),
                    Err(bits) => return bits,
                }
            }
        };

        if let Some((_, root_dir_flavor)) = root_dir.as_ref() {
            if *root_dir_flavor != pathname_flavor {
                let msg = if path_isabs_text(&pathname, sep) {
                    "Can't mix strings and bytes in path components"
                } else if pathname_flavor == PathFlavor::Bytes {
                    "cannot use a bytes pattern on a string-like object"
                } else {
                    "cannot use a string pattern on a bytes-like object"
                };
                return raise_exception::<_>(_py, "TypeError", msg);
            }
        }

        let dir_fd = match glob_dir_fd_arg_from_bits(_py, dir_fd_bits) {
            Err(bits) => return bits,
            Ok(value) => value,
        };

        let bytes_mode = pathname_flavor == PathFlavor::Bytes;
        #[cfg(target_arch = "wasm32")]
        {
            let root_dir_is_absolute = root_dir
                .as_ref()
                .is_some_and(|(path, _)| path_isabs_text(path, sep));
            if let GlobDirFdArg::Int(fd) = dir_fd {
                if glob_dir_fd_root_text(fd, bytes_mode).is_none()
                    && !path_isabs_text(&pathname, sep)
                    && !root_dir_is_absolute
                {
                    return raise_exception::<_>(
                        _py,
                        "NotImplementedError",
                        "glob(dir_fd=...) requires fd-backed path resolution on this wasm host",
                    );
                }
            }
        }

        let recursive = is_truthy(_py, obj_from_bits(recursive_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let include_hidden = is_truthy(_py, obj_from_bits(include_hidden_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let root_ref = if let Some((root, _)) = root_dir.as_ref() {
            Some(root.as_str())
        } else {
            None
        };

        let out = match glob_matches_text(
            _py,
            &pathname,
            root_ref,
            &dir_fd,
            recursive,
            include_hidden,
            bytes_mode,
            sep,
        ) {
            Ok(values) => values,
            Err(bits) => return bits,
        };
        alloc_path_list_bits(_py, &out, bytes_mode)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_path_chmod(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.write") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.write capability");
        }
        let path = match path_from_bits(_py, path_bits) {
            Ok(path) => path,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let mode = index_i64_from_obj(_py, mode_bits, "chmod() mode must be int");
        if exception_pending(_py) {
            return MoltObject::none().bits();
        };
        #[cfg(unix)]
        {
            let perms = std::fs::Permissions::from_mode(mode as u32);
            match std::fs::set_permissions(&path, perms) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => {
                    let msg = err.to_string();
                    match err.kind() {
                        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, "PermissionError", &msg)
                        }
                        _ => raise_exception::<_>(_py, "OSError", &msg),
                    }
                }
            }
        }
        #[cfg(windows)]
        {
            let readonly = ((mode as u32) & 0o222) == 0;
            let meta = match std::fs::metadata(&path) {
                Ok(meta) => meta,
                Err(err) => {
                    let msg = err.to_string();
                    return match err.kind() {
                        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, "PermissionError", &msg)
                        }
                        _ => raise_exception::<_>(_py, "OSError", &msg),
                    };
                }
            };
            let mut perms = meta.permissions();
            perms.set_readonly(readonly);
            match std::fs::set_permissions(&path, perms) {
                Ok(()) => MoltObject::none().bits(),
                Err(err) => {
                    let msg = err.to_string();
                    match err.kind() {
                        ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                        ErrorKind::PermissionDenied => {
                            raise_exception::<_>(_py, "PermissionError", &msg)
                        }
                        _ => raise_exception::<_>(_py, "OSError", &msg),
                    }
                }
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = mode;
            return raise_exception::<_>(
                _py,
                "NotImplementedError",
                "chmod is unsupported on this platform",
            );
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getcwd() -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        match std::env::current_dir() {
            Ok(path) => {
                let text = path.to_string_lossy();
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            Err(err) => {
                let msg = err.to_string();
                match err.kind() {
                    ErrorKind::NotFound => raise_exception::<_>(_py, "FileNotFoundError", &msg),
                    ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &msg)
                    }
                    ErrorKind::NotADirectory => {
                        raise_exception::<_>(_py, "NotADirectoryError", &msg)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &msg),
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_fsencode(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (fspath_bits, flavor) = match fspath_bits_with_flavor(_py, path_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if flavor == PathFlavor::Bytes {
            return fspath_bits;
        }

        let obj = obj_from_bits(fspath_bits);
        let Some(ptr) = obj.as_ptr() else {
            dec_ref_bits(_py, fspath_bits);
            return raise_exception::<_>(_py, "RuntimeError", "os fsencode received invalid path");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                dec_ref_bits(_py, fspath_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "os fsencode received invalid path",
                );
            }
            let raw = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
            let encoded = match crate::object::ops::encode_string_with_errors(
                raw,
                filesystem_encoding(),
                Some(filesystem_encode_errors()),
            ) {
                Ok(bytes) => bytes,
                Err(crate::object::ops::EncodeError::UnknownEncoding(name)) => {
                    dec_ref_bits(_py, fspath_bits);
                    let msg = format!("unknown encoding: {name}");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                Err(crate::object::ops::EncodeError::UnknownErrorHandler(name)) => {
                    dec_ref_bits(_py, fspath_bits);
                    let msg = format!("unknown error handler name '{name}'");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                Err(crate::object::ops::EncodeError::InvalidChar {
                    encoding,
                    code,
                    pos,
                    limit,
                }) => {
                    let reason = crate::object::ops::encode_error_reason(encoding, code, limit);
                    let exc_bits = raise_unicode_encode_error::<_>(
                        _py,
                        encoding,
                        fspath_bits,
                        pos,
                        pos + 1,
                        &reason,
                    );
                    dec_ref_bits(_py, fspath_bits);
                    return exc_bits;
                }
            };
            dec_ref_bits(_py, fspath_bits);
            let bytes_ptr = alloc_bytes(_py, encoded.as_slice());
            if bytes_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(bytes_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_getfilesystemencodeerrors() -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_string(_py, filesystem_encode_errors().as_bytes());
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_close(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "close");
        }
        #[cfg(target_arch = "wasm32")]
        {
            let rc = unsafe { crate::molt_os_close_host(fd) };
            if rc < 0 {
                return raise_os_error_errno::<u64>(_py, (-rc) as i64, "close");
            }
            return MoltObject::none().bits();
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            {
                let rc = unsafe { libc::close(fd as libc::c_int) };
                if rc == 0 {
                    return MoltObject::none().bits();
                }
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "close");
                }
                raise_os_error::<u64>(_py, err, "close")
            }
            #[cfg(windows)]
            {
                let sock_rc = unsafe { libc::closesocket(fd as libc::SOCKET) };
                if sock_rc == 0 {
                    return MoltObject::none().bits();
                }
                let sock_err = unsafe { libc::WSAGetLastError() };
                if sock_err == libc::WSAENOTSOCK {
                    let rc = unsafe { libc::_close(fd as libc::c_int) };
                    if rc == 0 {
                        return MoltObject::none().bits();
                    }
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "close");
                    }
                    return raise_os_error::<u64>(_py, err, "close");
                }
                return raise_os_error_errno::<u64>(_py, sock_err as i64, "close");
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_read(fd_bits: u64, len_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let Some(len) = to_i64(obj_from_bits(len_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, len_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "read");
        }
        if len < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EINVAL as i64, "read");
        }
        let mut buf = vec![0u8; len as usize];
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "read");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            let rc = unsafe {
                libc::read(
                    fd as libc::c_int,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                )
            };
            #[cfg(windows)]
            let rc = unsafe {
                libc::_read(
                    fd as libc::c_int,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len().min(i32::MAX as usize) as u32,
                )
            } as isize;
            #[cfg(not(any(unix, windows)))]
            let rc = -1isize;
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "read");
                }
                return raise_os_error::<u64>(_py, err, "read");
            }
            buf.truncate(rc as usize);
            let ptr = alloc_bytes(_py, &buf);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_write(fd_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "write");
        }
        let bytes = match unsafe { collect_bytes_like(_py, data_bits) } {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "write");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            let rc = unsafe {
                libc::write(
                    fd as libc::c_int,
                    bytes.as_ptr() as *const libc::c_void,
                    bytes.len(),
                )
            };
            #[cfg(windows)]
            let rc = unsafe {
                libc::_write(
                    fd as libc::c_int,
                    bytes.as_ptr() as *const libc::c_void,
                    bytes.len().min(i32::MAX as usize) as u32,
                )
            } as isize;
            #[cfg(not(any(unix, windows)))]
            let rc = -1isize;
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if let Some(errno) = err.raw_os_error() {
                    return raise_os_error_errno::<u64>(_py, errno as i64, "write");
                }
                return raise_os_error::<u64>(_py, err, "write");
            }
            int_bits_from_i64(_py, rc as i64)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_pipe() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "pipe");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            {
                let mut fds = [0 as libc::c_int; 2];
                if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "pipe");
                    }
                    return raise_os_error::<u64>(_py, err, "pipe");
                }

                let set_cloexec = |fd: libc::c_int| -> Result<(), std::io::Error> {
                    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
                    if flags < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if (flags & libc::FD_CLOEXEC) != 0 {
                        return Ok(());
                    }
                    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                };

                if let Err(err) = set_cloexec(fds[0]).and_then(|_| set_cloexec(fds[1])) {
                    let _ = unsafe { libc::close(fds[0]) };
                    let _ = unsafe { libc::close(fds[1]) };
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "pipe");
                    }
                    return raise_os_error::<u64>(_py, err, "pipe");
                }

                let read_bits = int_bits_from_i64(_py, fds[0] as i64);
                let write_bits = int_bits_from_i64(_py, fds[1] as i64);
                let tuple_ptr = alloc_tuple(_py, &[read_bits, write_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
            #[cfg(not(unix))]
            {
                return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "pipe");
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_dup(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "dup");
        }
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "dup");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let duped = dup_fd(fd);
            if let Some(new_fd) = duped {
                return int_bits_from_i64(_py, new_fd);
            }
            let err = std::io::Error::last_os_error();
            if let Some(errno) = err.raw_os_error() {
                return raise_os_error_errno::<u64>(_py, errno as i64, "dup");
            }
            raise_os_error::<u64>(_py, err, "dup")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_get_inheritable(fd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "get_inheritable");
        }
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_inheritable");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            {
                let flags = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFD) };
                if flags < 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "get_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "get_inheritable");
                }
                let inheritable = (flags & libc::FD_CLOEXEC) == 0;
                MoltObject::from_bool(inheritable).bits()
            }
            #[cfg(windows)]
            {
                let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
                if handle == -1 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "get_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "get_inheritable");
                }
                let mut flags: u32 = 0;
                let ok =
                    unsafe { GetHandleInformation(handle as *mut std::ffi::c_void, &mut flags) };
                if ok == 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "get_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "get_inheritable");
                }
                return MoltObject::from_bool((flags & HANDLE_FLAG_INHERIT) != 0).bits();
            }
            #[cfg(not(any(unix, windows)))]
            {
                raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "get_inheritable")
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_set_inheritable(fd_bits: u64, inheritable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(fd) = to_i64(obj_from_bits(fd_bits)) else {
            let type_name = class_name_for_error(type_of_bits(_py, fd_bits));
            let msg = format!("an integer is required (got {type_name})");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fd < 0 {
            return raise_os_error_errno::<u64>(_py, libc::EBADF as i64, "set_inheritable");
        }
        let inheritable = is_truthy(_py, obj_from_bits(inheritable_bits));
        #[cfg(target_arch = "wasm32")]
        {
            return raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "set_inheritable");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(unix)]
            {
                let flags = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFD) };
                if flags < 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "set_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "set_inheritable");
                }
                let mut new_flags = flags;
                if inheritable {
                    new_flags &= !libc::FD_CLOEXEC;
                } else {
                    new_flags |= libc::FD_CLOEXEC;
                }
                let rc = unsafe { libc::fcntl(fd as libc::c_int, libc::F_SETFD, new_flags) };
                if rc < 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "set_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "set_inheritable");
                }
                MoltObject::none().bits()
            }
            #[cfg(windows)]
            {
                let handle = unsafe { libc::_get_osfhandle(fd as libc::c_int) };
                if handle == -1 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "set_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "set_inheritable");
                }
                let flags = if inheritable { HANDLE_FLAG_INHERIT } else { 0 };
                let ok = unsafe {
                    SetHandleInformation(
                        handle as *mut std::ffi::c_void,
                        HANDLE_FLAG_INHERIT,
                        flags,
                    )
                };
                if ok == 0 {
                    let err = std::io::Error::last_os_error();
                    if let Some(errno) = err.raw_os_error() {
                        return raise_os_error_errno::<u64>(_py, errno as i64, "set_inheritable");
                    }
                    return raise_os_error::<u64>(_py, err, "set_inheritable");
                }
                return MoltObject::none().bits();
            }
            #[cfg(not(any(unix, windows)))]
            {
                raise_os_error_errno::<u64>(_py, libc::ENOSYS as i64, "set_inheritable")
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_os_urandom(len_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, len_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(len) = index_i64_with_overflow(
            _py,
            len_bits,
            &msg,
            Some("Python int too large to convert to C ssize_t"),
        ) else {
            return MoltObject::none().bits();
        };
        if len < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative argument not allowed");
        }
        let len = match usize::try_from(len) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "Python int too large to convert to C ssize_t",
                );
            }
        };
        let mut buf = Vec::new();
        if buf.try_reserve_exact(len).is_err() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        buf.resize(len, 0);
        if let Err(err) = getrandom_fill(&mut buf) {
            let msg = format!("urandom failed: {err}");
            return raise_exception::<_>(_py, "OSError", &msg);
        }
        let ptr = alloc_bytes(_py, &buf);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_open_builtin(
    file_bits: u64,
    mode_bits: u64,
    buffering_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    closefd_bits: u64,
    opener_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        open_impl(
            _py,
            file_bits,
            mode_bits,
            buffering_bits,
            encoding_bits,
            errors_bits,
            newline_bits,
            closefd_bits,
            opener_bits,
        )
    })
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct DecodeError {
    pub(crate) pos: usize,
    pub(crate) byte: u8,
    pub(crate) message: &'static str,
}

#[derive(Debug)]
pub(crate) enum DecodeFailure {
    Byte {
        pos: usize,
        byte: u8,
        message: &'static str,
    },
    Range {
        start: usize,
        end: usize,
        message: &'static str,
    },
    UnknownErrorHandler(String),
}

#[derive(Clone, Copy, Debug)]
enum TextEncodingKind {
    Utf8,
    Ascii,
    Latin1,
    Utf16,
    Utf32,
}

fn normalize_text_encoding(encoding: &str) -> Result<(String, TextEncodingKind), String> {
    let normalized = encoding.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "utf-8" | "utf8" => Ok(("utf-8".to_string(), TextEncodingKind::Utf8)),
        "utf-8-sig" | "utf8-sig" => Ok(("utf-8-sig".to_string(), TextEncodingKind::Utf8)),
        "cp1252" | "cp-1252" | "windows-1252" => {
            Ok(("cp1252".to_string(), TextEncodingKind::Latin1))
        }
        "cp437" | "ibm437" | "437" => Ok(("cp437".to_string(), TextEncodingKind::Latin1)),
        "cp850" | "ibm850" | "850" | "cp-850" => {
            Ok(("cp850".to_string(), TextEncodingKind::Latin1))
        }
        "cp860" | "ibm860" | "860" | "cp-860" => {
            Ok(("cp860".to_string(), TextEncodingKind::Latin1))
        }
        "cp862" | "ibm862" | "862" | "cp-862" => {
            Ok(("cp862".to_string(), TextEncodingKind::Latin1))
        }
        "cp863" | "ibm863" | "863" | "cp-863" => {
            Ok(("cp863".to_string(), TextEncodingKind::Latin1))
        }
        "cp865" | "ibm865" | "865" | "cp-865" => {
            Ok(("cp865".to_string(), TextEncodingKind::Latin1))
        }
        "cp866" | "ibm866" | "866" | "cp-866" => {
            Ok(("cp866".to_string(), TextEncodingKind::Latin1))
        }
        "cp874" | "cp-874" | "windows-874" => Ok(("cp874".to_string(), TextEncodingKind::Latin1)),
        "cp1250" | "cp-1250" | "windows-1250" => {
            Ok(("cp1250".to_string(), TextEncodingKind::Latin1))
        }
        "cp1251" | "cp-1251" | "windows-1251" => {
            Ok(("cp1251".to_string(), TextEncodingKind::Latin1))
        }
        "cp1253" | "cp-1253" | "windows-1253" => {
            Ok(("cp1253".to_string(), TextEncodingKind::Latin1))
        }
        "cp1254" | "cp-1254" | "windows-1254" => {
            Ok(("cp1254".to_string(), TextEncodingKind::Latin1))
        }
        "cp1255" | "cp-1255" | "windows-1255" => {
            Ok(("cp1255".to_string(), TextEncodingKind::Latin1))
        }
        "cp1256" | "cp-1256" | "windows-1256" => {
            Ok(("cp1256".to_string(), TextEncodingKind::Latin1))
        }
        "cp1257" | "cp-1257" | "windows-1257" => {
            Ok(("cp1257".to_string(), TextEncodingKind::Latin1))
        }
        "koi8-r" | "koi8r" | "koi8_r" => Ok(("koi8-r".to_string(), TextEncodingKind::Latin1)),
        "koi8-u" | "koi8u" | "koi8_u" => Ok(("koi8-u".to_string(), TextEncodingKind::Latin1)),
        "iso-8859-2" | "iso8859-2" | "latin2" | "latin-2" => {
            Ok(("iso8859-2".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-3" | "iso8859-3" | "latin3" | "latin-3" | "latin_3" => {
            Ok(("iso8859-3".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-4" | "iso8859-4" | "latin4" | "latin-4" | "latin_4" => {
            Ok(("iso8859-4".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-5" | "iso8859-5" | "cyrillic" => {
            Ok(("iso8859-5".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-6" | "iso8859-6" | "arabic" => {
            Ok(("iso8859-6".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-7" | "iso8859-7" | "greek" => {
            Ok(("iso8859-7".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-8" | "iso8859-8" | "hebrew" => {
            Ok(("iso8859-8".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-10" | "iso8859-10" | "latin6" | "latin-6" | "latin_6" => {
            Ok(("iso8859-10".to_string(), TextEncodingKind::Latin1))
        }
        "iso-8859-15" | "iso8859-15" | "latin9" | "latin-9" | "latin_9" => {
            Ok(("iso8859-15".to_string(), TextEncodingKind::Latin1))
        }
        "mac-roman" | "macroman" | "mac_roman" => {
            Ok(("mac-roman".to_string(), TextEncodingKind::Latin1))
        }
        "ascii" | "us-ascii" => Ok(("ascii".to_string(), TextEncodingKind::Ascii)),
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" => {
            Ok(("latin-1".to_string(), TextEncodingKind::Latin1))
        }
        "utf-16" | "utf16" => Ok(("utf-16".to_string(), TextEncodingKind::Utf16)),
        "utf-16-le" | "utf-16le" | "utf16le" => {
            Ok(("utf-16-le".to_string(), TextEncodingKind::Utf16))
        }
        "utf-16-be" | "utf-16be" | "utf16be" => {
            Ok(("utf-16-be".to_string(), TextEncodingKind::Utf16))
        }
        "utf-32" | "utf32" => Ok(("utf-32".to_string(), TextEncodingKind::Utf32)),
        "utf-32-le" | "utf-32le" | "utf32le" => {
            Ok(("utf-32-le".to_string(), TextEncodingKind::Utf32))
        }
        "utf-32-be" | "utf-32be" | "utf32be" => {
            Ok(("utf-32-be".to_string(), TextEncodingKind::Utf32))
        }
        _ => Err(format!("unknown encoding: {encoding}")),
    }
}

fn text_encoding_kind(label: &str) -> TextEncodingKind {
    match label {
        "ascii" => TextEncodingKind::Ascii,
        "latin-1" => TextEncodingKind::Latin1,
        "cp1252" => TextEncodingKind::Latin1,
        "cp437" => TextEncodingKind::Latin1,
        "cp850" => TextEncodingKind::Latin1,
        "cp860" => TextEncodingKind::Latin1,
        "cp862" => TextEncodingKind::Latin1,
        "cp863" => TextEncodingKind::Latin1,
        "cp865" => TextEncodingKind::Latin1,
        "cp866" => TextEncodingKind::Latin1,
        "cp874" => TextEncodingKind::Latin1,
        "cp1250" => TextEncodingKind::Latin1,
        "cp1251" => TextEncodingKind::Latin1,
        "cp1253" => TextEncodingKind::Latin1,
        "cp1254" => TextEncodingKind::Latin1,
        "cp1255" => TextEncodingKind::Latin1,
        "cp1256" => TextEncodingKind::Latin1,
        "cp1257" => TextEncodingKind::Latin1,
        "koi8-r" => TextEncodingKind::Latin1,
        "koi8-u" => TextEncodingKind::Latin1,
        "iso8859-2" => TextEncodingKind::Latin1,
        "iso8859-3" => TextEncodingKind::Latin1,
        "iso8859-4" => TextEncodingKind::Latin1,
        "iso8859-5" => TextEncodingKind::Latin1,
        "iso8859-6" => TextEncodingKind::Latin1,
        "iso8859-7" => TextEncodingKind::Latin1,
        "iso8859-8" => TextEncodingKind::Latin1,
        "iso8859-10" => TextEncodingKind::Latin1,
        "iso8859-15" => TextEncodingKind::Latin1,
        "mac-roman" => TextEncodingKind::Latin1,
        "utf-8-sig" => TextEncodingKind::Utf8,
        _ if label.starts_with("utf-16") => TextEncodingKind::Utf16,
        _ if label.starts_with("utf-32") => TextEncodingKind::Utf32,
        _ => TextEncodingKind::Utf8,
    }
}

fn text_encoding_is_multibyte(kind: TextEncodingKind) -> bool {
    matches!(kind, TextEncodingKind::Utf16 | TextEncodingKind::Utf32)
}

fn text_encoding_is_variable(kind: TextEncodingKind) -> bool {
    matches!(
        kind,
        TextEncodingKind::Utf8 | TextEncodingKind::Utf16 | TextEncodingKind::Utf32
    )
}

fn split_fixed_pending(
    handle: &mut MoltFileHandle,
    bytes: &mut Vec<u8>,
    at_eof: bool,
    unit: usize,
) {
    if at_eof {
        handle.text_pending_bytes.clear();
        return;
    }
    let rem = bytes.len() % unit;
    if rem == 0 {
        handle.text_pending_bytes.clear();
        return;
    }
    let split = bytes.len().saturating_sub(rem);
    let pending = bytes.split_off(split);
    handle.text_pending_bytes = pending;
}

fn split_text_pending_bytes(
    handle: &mut MoltFileHandle,
    bytes: &mut Vec<u8>,
    at_eof: bool,
    kind: TextEncodingKind,
) {
    match kind {
        TextEncodingKind::Utf8 => split_utf8_pending(handle, bytes, at_eof),
        TextEncodingKind::Utf16 => split_fixed_pending(handle, bytes, at_eof, 2),
        TextEncodingKind::Utf32 => split_fixed_pending(handle, bytes, at_eof, 4),
        TextEncodingKind::Ascii | TextEncodingKind::Latin1 => {
            handle.text_pending_bytes.clear();
        }
    }
}

fn decode_text_bytes(
    _py: &PyToken<'_>,
    encoding_label: &str,
    errors: &str,
    bytes: &[u8],
) -> Result<(Vec<u8>, String), u64> {
    match crate::object::ops::decode_bytes_text(encoding_label, errors, bytes) {
        Ok((text_bytes, label)) => Ok((text_bytes, label)),
        Err(crate::object::ops::DecodeTextError::UnknownEncoding(name)) => {
            let msg = format!("unknown encoding: {name}");
            Err(raise_exception::<_>(_py, "LookupError", &msg))
        }
        Err(crate::object::ops::DecodeTextError::UnknownErrorHandler(name)) => {
            let msg = format!("unknown error handler name '{name}'");
            Err(raise_exception::<_>(_py, "LookupError", &msg))
        }
        Err(crate::object::ops::DecodeTextError::Failure(
            DecodeFailure::Byte { pos, byte, message },
            label,
        )) => {
            let msg = decode_error_byte(&label, byte, pos, message);
            Err(raise_exception::<_>(_py, "UnicodeDecodeError", &msg))
        }
        Err(crate::object::ops::DecodeTextError::Failure(
            DecodeFailure::Range {
                start,
                end,
                message,
            },
            label,
        )) => {
            let msg = decode_error_range(&label, start, end, message);
            Err(raise_exception::<_>(_py, "UnicodeDecodeError", &msg))
        }
        Err(crate::object::ops::DecodeTextError::Failure(
            DecodeFailure::UnknownErrorHandler(name),
            _label,
        )) => {
            let msg = format!("unknown error handler name '{name}'");
            Err(raise_exception::<_>(_py, "LookupError", &msg))
        }
    }
}

fn decode_text_bytes_for_io(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    encoding_label: &str,
    errors: &str,
    bytes: &[u8],
) -> Result<(Vec<u8>, String), u64> {
    let mut decode_label = encoding_label;
    if encoding_label == "utf-8-sig" && handle.text_bom_seen {
        decode_label = "utf-8";
    }
    let result = decode_text_bytes(_py, decode_label, errors, bytes)?;
    if encoding_label == "utf-8-sig" && !handle.text_bom_seen && !bytes.is_empty() {
        handle.text_bom_seen = true;
    }
    Ok(result)
}

fn decode_multibyte_text(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    encoding_label: &mut String,
    encoding_kind: &mut TextEncodingKind,
    errors: &str,
    bytes: &[u8],
    at_eof: bool,
) -> Result<Vec<u8>, u64> {
    let (mut text_bytes, label) = decode_text_bytes(_py, encoding_label, errors, bytes)?;
    if (encoding_label.as_str() == "utf-16" || encoding_label.as_str() == "utf-32")
        && label != *encoding_label
    {
        *encoding_label = label.clone();
        handle.encoding = Some(label.clone());
        *encoding_kind = text_encoding_kind(encoding_label.as_str());
    }
    let newline_is_none = handle.newline.is_none();
    let combine_crlf = matches!(handle.newline.as_deref(), None | Some(""));
    let mut combined: Vec<u8> = Vec::new();
    if let Some(pending) = handle.pending_byte.take() {
        if combine_crlf && pending == b'\r' {
            if text_bytes.first() == Some(&b'\n') {
                combined.extend_from_slice(b"\r\n");
                text_bytes.remove(0);
            } else {
                combined.push(b'\r');
            }
        } else {
            combined.push(pending);
        }
    }
    combined.extend_from_slice(&text_bytes);
    if combine_crlf && !at_eof && combined.last() == Some(&b'\r') {
        combined.pop();
        handle.pending_byte = Some(b'\r');
    }
    update_newlines_from_bytes(handle, &combined);
    if newline_is_none {
        Ok(translate_universal_newlines(&combined))
    } else {
        Ok(combined)
    }
}

fn utf8_expected_len(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if (0xC2..=0xDF).contains(&byte) {
        2
    } else if (0xE0..=0xEF).contains(&byte) {
        3
    } else if (0xF0..=0xF4).contains(&byte) {
        4
    } else {
        1
    }
}

fn utf8_pending_len(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let mut cont = 0usize;
    let mut idx = bytes.len();
    while cont < 3 && idx > 0 {
        let byte = bytes[idx - 1];
        if (byte & 0xC0) == 0x80 {
            cont += 1;
            idx -= 1;
        } else {
            break;
        }
    }
    if cont == 0 {
        let byte = bytes[bytes.len() - 1];
        let needed = utf8_expected_len(byte);
        return if needed > 1 { 1 } else { 0 };
    }
    if idx == 0 {
        return 0;
    }
    let lead = bytes[idx - 1];
    let expected = utf8_expected_len(lead);
    if expected <= 1 {
        return 0;
    }
    let seq_len = cont + 1;
    if expected > seq_len { seq_len } else { 0 }
}

fn split_utf8_pending(handle: &mut MoltFileHandle, bytes: &mut Vec<u8>, at_eof: bool) {
    if at_eof || bytes.is_empty() {
        handle.text_pending_bytes.clear();
        return;
    }
    let pending_len = utf8_pending_len(bytes);
    if pending_len == 0 {
        handle.text_pending_bytes.clear();
        return;
    }
    let split = bytes.len().saturating_sub(pending_len);
    let pending = bytes.split_off(split);
    handle.text_pending_bytes = pending;
}

fn wtf8_char_count(bytes: &[u8]) -> usize {
    let mut count = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let width = utf8_expected_len(bytes[idx]);
        idx = idx.saturating_add(width).min(bytes.len());
        count += 1;
    }
    count
}

fn wtf8_split_index(bytes: &[u8], limit: usize) -> usize {
    if limit == 0 {
        return 0;
    }
    let mut count = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() && count < limit {
        let width = utf8_expected_len(bytes[idx]);
        idx = idx.saturating_add(width).min(bytes.len());
        count += 1;
    }
    idx
}

fn pending_text_line_end(bytes: &[u8], newline: Option<&str>) -> Option<usize> {
    match newline {
        None | Some("\n") => bytes.iter().position(|&b| b == b'\n').map(|idx| idx + 1),
        Some("") => {
            let mut idx = 0usize;
            while idx < bytes.len() {
                let byte = bytes[idx];
                if byte == b'\n' {
                    return Some(idx + 1);
                }
                if byte == b'\r' {
                    if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                        return Some(idx + 2);
                    }
                    return Some(idx + 1);
                }
                idx += 1;
            }
            None
        }
        Some("\r") => bytes.iter().position(|&b| b == b'\r').map(|idx| idx + 1),
        Some("\r\n") => {
            let mut idx = 0usize;
            while idx + 1 < bytes.len() {
                if bytes[idx] == b'\r' && bytes[idx + 1] == b'\n' {
                    return Some(idx + 2);
                }
                idx += 1;
            }
            None
        }
        Some(_) => bytes.iter().position(|&b| b == b'\n').map(|idx| idx + 1),
    }
}

fn validate_decode_error_handler(errors: &str) -> Result<(), String> {
    if matches!(
        errors,
        "strict" | "ignore" | "replace" | "backslashreplace" | "surrogateescape" | "surrogatepass"
    ) {
        Ok(())
    } else {
        Err(format!("unknown error handler name '{errors}'"))
    }
}

fn validate_encode_error_handler(errors: &str) -> Result<(), String> {
    if matches!(
        errors,
        "strict"
            | "ignore"
            | "replace"
            | "backslashreplace"
            | "surrogateescape"
            | "surrogatepass"
            | "namereplace"
            | "xmlcharrefreplace"
    ) {
        Ok(())
    } else {
        Err(format!("unknown error handler name '{errors}'"))
    }
}

fn decode_error_byte(label: &str, byte: u8, pos: usize, message: &str) -> String {
    format!("'{label}' codec can't decode byte 0x{byte:02x} in position {pos}: {message}")
}

fn decode_error_range(label: &str, start: usize, end: usize, message: &str) -> String {
    format!("'{label}' codec can't decode bytes in position {start}-{end}: {message}")
}

const TEXT_COOKIE_VERSION: u8 = 2;
const TEXT_COOKIE_MAX_PENDING: usize = 4;
const TEXT_COOKIE_FIXED_LEN: usize = 16;

struct TextCookie {
    pos: u64,
    pending_byte: Option<u8>,
    pending_bytes: Vec<u8>,
    pending_text: Vec<u8>,
}

fn text_cookie_encode_bits(
    _py: &PyToken<'_>,
    pos: u64,
    pending_byte: Option<u8>,
    pending_bytes: &[u8],
    pending_text: &[u8],
) -> Result<u64, String> {
    if pos == 0 && pending_byte.is_none() && pending_bytes.is_empty() && pending_text.is_empty() {
        return Ok(MoltObject::from_int(0).bits());
    }
    if pending_bytes.len() > TEXT_COOKIE_MAX_PENDING {
        return Err("tell overflow".to_string());
    }
    let pending_text_len: u32 = pending_text
        .len()
        .try_into()
        .map_err(|_| "tell overflow".to_string())?;
    let mut bytes =
        Vec::with_capacity(TEXT_COOKIE_FIXED_LEN + pending_bytes.len() + pending_text.len());
    bytes.push(TEXT_COOKIE_VERSION);
    if let Some(byte) = pending_byte {
        bytes.push(1);
        bytes.push(byte);
    } else {
        bytes.push(0);
        bytes.push(0);
    }
    bytes.push(pending_bytes.len() as u8);
    bytes.extend_from_slice(pending_bytes);
    bytes.extend_from_slice(&pending_text_len.to_le_bytes());
    bytes.extend_from_slice(pending_text);
    bytes.extend_from_slice(&pos.to_le_bytes());
    let value = BigInt::from_bytes_le(Sign::Plus, &bytes);
    Ok(int_bits_from_bigint(_py, value))
}

fn text_cookie_decode_value(value: BigInt) -> Result<TextCookie, String> {
    if value.sign() == Sign::Minus {
        return Err("negative seek position".to_string());
    }
    if value.is_zero() {
        return Ok(TextCookie {
            pos: 0,
            pending_byte: None,
            pending_bytes: Vec::new(),
            pending_text: Vec::new(),
        });
    }
    let (_, mut bytes) = value.to_bytes_le();
    if bytes.len() < TEXT_COOKIE_FIXED_LEN {
        bytes.resize(TEXT_COOKIE_FIXED_LEN, 0);
    }
    if bytes[0] != TEXT_COOKIE_VERSION {
        return Err("invalid seek position".to_string());
    }
    let pending_flag = bytes[1] != 0;
    let pending_byte = if pending_flag { Some(bytes[2]) } else { None };
    let pending_len = bytes[3] as usize;
    if pending_len > TEXT_COOKIE_MAX_PENDING {
        return Err("invalid seek position".to_string());
    }
    let pending_bytes = if pending_len == 0 {
        Vec::new()
    } else {
        bytes[4..4 + pending_len].to_vec()
    };
    let text_len_offset = 4 + pending_len;
    if bytes.len() < text_len_offset + 4 {
        return Err("invalid seek position".to_string());
    }
    let pending_text_len = u32::from_le_bytes(
        bytes[text_len_offset..text_len_offset + 4]
            .try_into()
            .map_err(|_| "invalid seek position".to_string())?,
    ) as usize;
    let text_offset = text_len_offset + 4;
    let pos_offset = text_offset + pending_text_len;
    if bytes.len() < pos_offset + 8 {
        bytes.resize(pos_offset + 8, 0);
    }
    let pending_text = if pending_text_len == 0 {
        Vec::new()
    } else {
        bytes[text_offset..text_offset + pending_text_len].to_vec()
    };
    let pos = u64::from_le_bytes(
        bytes[pos_offset..pos_offset + 8]
            .try_into()
            .map_err(|_| "invalid seek position".to_string())?,
    );
    Ok(TextCookie {
        pos,
        pending_byte,
        pending_bytes,
        pending_text,
    })
}

fn translate_universal_newlines(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\r' => {
                if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                    idx += 2;
                } else {
                    idx += 1;
                }
                out.push(b'\n');
            }
            byte => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    out
}

fn should_track_newlines(handle: &MoltFileHandle) -> bool {
    handle.text && matches!(handle.newline.as_deref(), None | Some(""))
}

fn record_newline(handle: &mut MoltFileHandle, kind: u8) {
    if (handle.newlines_mask & kind) != 0 {
        return;
    }
    if (handle.newlines_len as usize) < handle.newlines_seen.len() {
        handle.newlines_seen[handle.newlines_len as usize] = kind;
        handle.newlines_len = handle.newlines_len.saturating_add(1);
    }
    handle.newlines_mask |= kind;
}

fn update_newlines_from_bytes(handle: &mut MoltFileHandle, bytes: &[u8]) {
    if !should_track_newlines(handle) || bytes.is_empty() {
        return;
    }
    let mut idx = 0usize;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if byte == b'\r' {
            if idx + 1 < bytes.len() && bytes[idx + 1] == b'\n' {
                record_newline(handle, NEWLINE_KIND_CRLF);
                idx += 2;
                continue;
            }
            record_newline(handle, NEWLINE_KIND_CR);
            idx += 1;
            continue;
        }
        if byte == b'\n' {
            record_newline(handle, NEWLINE_KIND_LF);
        }
        idx += 1;
    }
}

fn update_newlines_from_chars(handle: &mut MoltFileHandle, chars: &[char]) {
    if !should_track_newlines(handle) || chars.is_empty() {
        return;
    }
    let mut idx = 0usize;
    while idx < chars.len() {
        let ch = chars[idx];
        if ch == '\r' {
            if idx + 1 < chars.len() && chars[idx + 1] == '\n' {
                record_newline(handle, NEWLINE_KIND_CRLF);
                idx += 2;
                continue;
            }
            record_newline(handle, NEWLINE_KIND_CR);
            idx += 1;
            continue;
        }
        if ch == '\n' {
            record_newline(handle, NEWLINE_KIND_LF);
        }
        idx += 1;
    }
}

fn translate_write_newlines_bytes(bytes: &[u8], newline: Option<&str>) -> Vec<u8> {
    let target = match newline {
        None => {
            if cfg!(windows) {
                "\r\n"
            } else {
                "\n"
            }
        }
        Some("") | Some("\n") => "\n",
        Some(value) => value,
    };
    if target == "\n" {
        return bytes.to_vec();
    }
    let target_bytes = target.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    for &byte in bytes {
        if byte == b'\n' {
            out.extend_from_slice(target_bytes);
        } else {
            out.push(byte);
        }
    }
    out
}

fn translate_write_newlines_str(text: &str, newline: Option<&str>) -> String {
    let target = match newline {
        None => {
            if cfg!(windows) {
                "\r\n"
            } else {
                "\n"
            }
        }
        Some("") | Some("\n") => "\n",
        Some(value) => value,
    };
    if target == "\n" {
        return text.to_string();
    }
    text.replace('\n', target)
}

unsafe fn text_backend_read(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    let newline = handle.newline.as_deref();
    let limit = size.unwrap_or(usize::MAX);
    let mut out = String::new();
    let mut count = 0usize;
    let mut idx = text_backend.pos;
    let start = idx;
    let len = text_backend.data.len();
    while idx < len && count < limit {
        let ch = text_backend.data[idx];
        if newline.is_none() && ch == '\r' {
            if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                idx += 2;
            } else {
                idx += 1;
            }
            out.push('\n');
            count += 1;
            continue;
        }
        out.push(ch);
        idx += 1;
        count += 1;
    }
    text_backend.pos = idx;
    update_newlines_from_chars(handle, &text_backend.data[start..idx]);
    Ok(out)
}

unsafe fn text_backend_readline(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    limit: Option<usize>,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    let newline = handle.newline.as_deref();
    let max_len = limit.unwrap_or(usize::MAX);
    let mut out = String::new();
    let mut count = 0usize;
    let mut idx = text_backend.pos;
    let start = idx;
    let len = text_backend.data.len();
    while idx < len && count < max_len {
        let ch = text_backend.data[idx];
        match newline {
            None => {
                if ch == '\n' {
                    out.push('\n');
                    idx += 1;
                    break;
                }
                if ch == '\r' {
                    if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                        idx += 2;
                    } else {
                        idx += 1;
                    }
                    out.push('\n');
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
            }
            Some("") => {
                if ch == '\r' {
                    if count >= max_len {
                        break;
                    }
                    out.push('\r');
                    count += 1;
                    if count >= max_len {
                        idx += 1;
                        break;
                    }
                    if idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                        out.push('\n');
                        idx += 2;
                    } else {
                        idx += 1;
                    }
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
            Some("\n") => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
            Some("\r") => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\r' {
                    break;
                }
            }
            Some("\r\n") => {
                if ch == '\r' && idx + 1 < len && text_backend.data[idx + 1] == '\n' {
                    if count >= max_len {
                        break;
                    }
                    out.push('\r');
                    count += 1;
                    if count >= max_len {
                        idx += 1;
                        break;
                    }
                    out.push('\n');
                    idx += 2;
                    break;
                }
                out.push(ch);
                count += 1;
                idx += 1;
            }
            Some(_) => {
                out.push(ch);
                count += 1;
                idx += 1;
                if ch == '\n' {
                    break;
                }
            }
        }
    }
    text_backend.pos = idx;
    update_newlines_from_chars(handle, &text_backend.data[start..idx]);
    Ok(out)
}

unsafe fn text_backend_write(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    text: &str,
) -> Result<usize, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    let translated = translate_write_newlines_str(text, handle.newline.as_deref());
    let chars: Vec<char> = translated.chars().collect();
    let pos = text_backend.pos;
    if pos > text_backend.data.len() {
        text_backend.data.resize(pos, '\0');
    }
    let end = pos.saturating_add(chars.len());
    if end > text_backend.data.len() {
        text_backend.data.resize(end, '\0');
    }
    text_backend.data[pos..end].copy_from_slice(&chars);
    text_backend.pos = end;
    Ok(chars.len())
}

unsafe fn text_backend_getvalue(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
) -> Result<String, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    Ok(text_backend.data.iter().collect())
}

unsafe fn text_backend_seek(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
    offset: i64,
    whence: i64,
) -> Result<i64, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    let len = text_backend.data.len() as i64;
    let new_pos = match whence {
        0 => offset,
        1 => text_backend.pos as i64 + offset,
        2 => len + offset,
        _ => {
            return Err(raise_exception::<_>(_py, "ValueError", "invalid whence"));
        }
    };
    if new_pos < 0 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "negative seek position",
        ));
    }
    text_backend.pos = new_pos as usize;
    Ok(new_pos)
}

unsafe fn text_backend_truncate(
    _py: &PyToken<'_>,
    backend: &mut MoltFileBackend,
    size: usize,
) -> Result<(), u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    if size < text_backend.data.len() {
        text_backend.data.truncate(size);
    } else if size > text_backend.data.len() {
        text_backend.data.resize(size, '\0');
    }
    if text_backend.pos > text_backend.data.len() {
        text_backend.pos = text_backend.data.len();
    }
    Ok(())
}

unsafe fn text_backend_tell(_py: &PyToken<'_>, backend: &mut MoltFileBackend) -> Result<i64, u64> {
    let MoltFileBackend::Text(text_backend) = backend else {
        return Err(raise_exception::<_>(
            _py,
            "UnsupportedOperation",
            "text backend missing",
        ));
    };
    Ok(text_backend.pos as i64)
}

pub(crate) fn file_handle_detached_message(handle: &MoltFileHandle) -> &'static str {
    if handle.text {
        "underlying buffer has been detached"
    } else {
        "raw stream has been detached"
    }
}

pub(crate) fn file_handle_is_closed(handle: &MoltFileHandle) -> bool {
    if handle.closed {
        return true;
    }
    Arc::clone(&handle.state).backend.lock().unwrap().is_none()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_read(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg = format!("argument should be integer or None, not '{type_name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            if size == Some(0) {
                if handle.text {
                    let out_ptr = alloc_string(_py, b"");
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let out_ptr = alloc_bytes(_py, &[]);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let text = match text_backend_read(_py, handle, backend, size) {
                        Ok(text) => text,
                        Err(bits) => return bits,
                    };
                    let out_ptr = alloc_string(_py, text.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
            if handle.text {
                let errors = handle
                    .errors
                    .clone()
                    .unwrap_or_else(|| "strict".to_string());
                if let Err(msg) = validate_decode_error_handler(errors.as_str()) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let mut encoding_label = handle.encoding.as_deref().unwrap_or("utf-8").to_string();
                let mut encoding_kind = text_encoding_kind(encoding_label.as_str());
                let mut out_text: Vec<u8> = Vec::new();
                let mut remaining = size;
                if let Some(limit) = remaining {
                    if !handle.text_pending_text.is_empty() {
                        let pending_chars = wtf8_char_count(&handle.text_pending_text);
                        if pending_chars >= limit {
                            let split = wtf8_split_index(&handle.text_pending_text, limit);
                            out_text.extend_from_slice(&handle.text_pending_text[..split]);
                            let rest = handle.text_pending_text.split_off(split);
                            handle.text_pending_text = rest;
                            let out_ptr = alloc_string(_py, &out_text);
                            if out_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(out_ptr).bits();
                        }
                        out_text.extend_from_slice(&handle.text_pending_text);
                        remaining = Some(limit - pending_chars);
                        handle.text_pending_text.clear();
                    }
                } else if !handle.text_pending_text.is_empty() {
                    out_text.extend_from_slice(&handle.text_pending_text);
                    handle.text_pending_text.clear();
                }
                loop {
                    let mut buf = Vec::new();
                    let multibyte = text_encoding_is_multibyte(encoding_kind);
                    if !multibyte {
                        if let Some(pending) = handle.pending_byte.take() {
                            buf.push(pending);
                        }
                    }
                    let mut pending_utf8_needed = 0usize;
                    if !handle.text_pending_bytes.is_empty() {
                        let pending = std::mem::take(&mut handle.text_pending_bytes);
                        if matches!(encoding_kind, TextEncodingKind::Utf8) && !pending.is_empty() {
                            let expected = utf8_expected_len(pending[0]);
                            if expected > pending.len() {
                                pending_utf8_needed = expected - pending.len();
                            }
                        }
                        buf.extend_from_slice(&pending);
                    }
                    let mut byte_limit = remaining;
                    if text_encoding_is_variable(encoding_kind) {
                        byte_limit = None;
                    }
                    if let Some(rem) = byte_limit {
                        if pending_utf8_needed > rem {
                            byte_limit = Some(pending_utf8_needed);
                        }
                    }
                    let (mut more, at_eof) =
                        match file_read1_bytes(_py, handle, backend, byte_limit) {
                            Ok(val) => val,
                            Err(bits) => return bits,
                        };
                    buf.append(&mut more);
                    split_text_pending_bytes(handle, &mut buf, at_eof, encoding_kind);
                    let text_bytes = if multibyte {
                        match decode_multibyte_text(
                            _py,
                            handle,
                            &mut encoding_label,
                            &mut encoding_kind,
                            errors.as_str(),
                            &buf,
                            at_eof,
                        ) {
                            Ok(text_bytes) => text_bytes,
                            Err(bits) => return bits,
                        }
                    } else {
                        if handle.newline.is_none() && buf.last() == Some(&b'\r') && !at_eof {
                            handle.pending_byte = Some(b'\r');
                            buf.pop();
                        }
                        update_newlines_from_bytes(handle, &buf);
                        let bytes = if handle.newline.is_none() {
                            translate_universal_newlines(&buf)
                        } else {
                            buf
                        };
                        match decode_text_bytes_for_io(
                            _py,
                            handle,
                            encoding_label.as_str(),
                            errors.as_str(),
                            &bytes,
                        ) {
                            Ok((text_bytes, _label)) => text_bytes,
                            Err(bits) => return bits,
                        }
                    };
                    match remaining {
                        None => {
                            out_text.extend_from_slice(&text_bytes);
                            if at_eof {
                                break;
                            }
                        }
                        Some(rem) => {
                            let text_chars = wtf8_char_count(&text_bytes);
                            if text_chars <= rem {
                                out_text.extend_from_slice(&text_bytes);
                                let new_rem = rem.saturating_sub(text_chars);
                                remaining = Some(new_rem);
                                if new_rem == 0 || at_eof {
                                    break;
                                }
                            } else {
                                let split = wtf8_split_index(&text_bytes, rem);
                                out_text.extend_from_slice(&text_bytes[..split]);
                                handle.text_pending_text = text_bytes[split..].to_vec();
                                break;
                            }
                        }
                    }
                    if at_eof {
                        break;
                    }
                }
                let out_ptr = alloc_string(_py, &out_text);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let mut buf = Vec::new();
                let mut remaining = size;
                if let Some(pending) = handle.pending_byte.take() {
                    if let Some(rem) = remaining {
                        if rem == 0 {
                            handle.pending_byte = Some(pending);
                        } else {
                            buf.push(pending);
                            remaining = Some(rem.saturating_sub(1));
                        }
                    } else {
                        buf.push(pending);
                    }
                }
                let (mut more, _at_eof) = match buffered_read_bytes(_py, handle, backend, remaining)
                {
                    Ok(val) => val,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                let out_ptr = alloc_bytes(_py, &buf);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_read1(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg = format!("argument should be integer or None, not '{type_name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            if size == Some(0) {
                if handle.text {
                    let out_ptr = alloc_string(_py, b"");
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let out_ptr = alloc_bytes(_py, &[]);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let text = match text_backend_read(_py, handle, backend, size) {
                        Ok(text) => text,
                        Err(bits) => return bits,
                    };
                    let out_ptr = alloc_string(_py, text.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
            if handle.text {
                let errors = handle
                    .errors
                    .clone()
                    .unwrap_or_else(|| "strict".to_string());
                if let Err(msg) = validate_decode_error_handler(errors.as_str()) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let mut encoding_label = handle.encoding.as_deref().unwrap_or("utf-8").to_string();
                let mut encoding_kind = text_encoding_kind(encoding_label.as_str());
                let mut out_text: Vec<u8> = Vec::new();
                let mut remaining = size;
                if let Some(limit) = remaining {
                    if !handle.text_pending_text.is_empty() {
                        let pending_chars = wtf8_char_count(&handle.text_pending_text);
                        if pending_chars >= limit {
                            let split = wtf8_split_index(&handle.text_pending_text, limit);
                            out_text.extend_from_slice(&handle.text_pending_text[..split]);
                            let rest = handle.text_pending_text.split_off(split);
                            handle.text_pending_text = rest;
                            let out_ptr = alloc_string(_py, &out_text);
                            if out_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(out_ptr).bits();
                        }
                        out_text.extend_from_slice(&handle.text_pending_text);
                        remaining = Some(limit - pending_chars);
                        handle.text_pending_text.clear();
                    }
                } else if !handle.text_pending_text.is_empty() {
                    out_text.extend_from_slice(&handle.text_pending_text);
                    handle.text_pending_text.clear();
                    let out_ptr = alloc_string(_py, &out_text);
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let mut buf = Vec::new();
                let multibyte = text_encoding_is_multibyte(encoding_kind);
                if !multibyte {
                    if let Some(pending) = handle.pending_byte.take() {
                        buf.push(pending);
                    }
                }
                let mut pending_utf8_needed = 0usize;
                if !handle.text_pending_bytes.is_empty() {
                    let pending = std::mem::take(&mut handle.text_pending_bytes);
                    if matches!(encoding_kind, TextEncodingKind::Utf8) && !pending.is_empty() {
                        let expected = utf8_expected_len(pending[0]);
                        if expected > pending.len() {
                            pending_utf8_needed = expected - pending.len();
                        }
                    }
                    buf.extend_from_slice(&pending);
                }
                let mut byte_limit = remaining;
                if text_encoding_is_variable(encoding_kind) {
                    byte_limit = None;
                }
                if let Some(rem) = byte_limit {
                    if pending_utf8_needed > rem {
                        byte_limit = Some(pending_utf8_needed);
                    }
                }
                let (mut more, at_eof) = match file_read1_bytes(_py, handle, backend, byte_limit) {
                    Ok(more) => more,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                split_text_pending_bytes(handle, &mut buf, at_eof, encoding_kind);
                let text_bytes = if multibyte {
                    match decode_multibyte_text(
                        _py,
                        handle,
                        &mut encoding_label,
                        &mut encoding_kind,
                        errors.as_str(),
                        &buf,
                        at_eof,
                    ) {
                        Ok(text_bytes) => text_bytes,
                        Err(bits) => return bits,
                    }
                } else {
                    if matches!(handle.newline.as_deref(), None | Some(""))
                        && buf.last() == Some(&b'\r')
                        && !at_eof
                    {
                        handle.pending_byte = Some(b'\r');
                        buf.pop();
                    }
                    update_newlines_from_bytes(handle, &buf);
                    let bytes = if handle.newline.is_none() {
                        translate_universal_newlines(&buf)
                    } else {
                        buf
                    };
                    match decode_text_bytes_for_io(
                        _py,
                        handle,
                        encoding_label.as_str(),
                        errors.as_str(),
                        &bytes,
                    ) {
                        Ok((text_bytes, _label)) => text_bytes,
                        Err(bits) => return bits,
                    }
                };
                match remaining {
                    None => {
                        out_text.extend_from_slice(&text_bytes);
                    }
                    Some(rem) => {
                        let text_chars = wtf8_char_count(&text_bytes);
                        if text_chars <= rem {
                            out_text.extend_from_slice(&text_bytes);
                        } else {
                            let split = wtf8_split_index(&text_bytes, rem);
                            out_text.extend_from_slice(&text_bytes[..split]);
                            handle.text_pending_text = text_bytes[split..].to_vec();
                        }
                    }
                }
                let out_ptr = alloc_string(_py, &out_text);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let mut buf = Vec::new();
                let mut remaining = size;
                if let Some(pending) = handle.pending_byte.take() {
                    if let Some(rem) = remaining {
                        if rem == 0 {
                            handle.pending_byte = Some(pending);
                        } else {
                            buf.push(pending);
                            remaining = Some(rem.saturating_sub(1));
                        }
                    } else {
                        buf.push(pending);
                    }
                }
                let (mut more, _at_eof) = match file_read1_bytes(_py, handle, backend, remaining) {
                    Ok(more) => more,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                let out_ptr = alloc_bytes(_py, &buf);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readall(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let text = match text_backend_read(_py, handle, backend, None) {
                        Ok(text) => text,
                        Err(bits) => return bits,
                    };
                    let out_ptr = alloc_string(_py, text.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
            if handle.text {
                let errors = handle
                    .errors
                    .clone()
                    .unwrap_or_else(|| "strict".to_string());
                if let Err(msg) = validate_decode_error_handler(errors.as_str()) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let mut encoding_label = handle.encoding.as_deref().unwrap_or("utf-8").to_string();
                let mut encoding_kind = text_encoding_kind(encoding_label.as_str());
                let mut out_text = Vec::new();
                if !handle.text_pending_text.is_empty() {
                    out_text.extend_from_slice(&handle.text_pending_text);
                    handle.text_pending_text.clear();
                }
                let mut buf = Vec::new();
                let multibyte = text_encoding_is_multibyte(encoding_kind);
                if !multibyte {
                    if let Some(pending) = handle.pending_byte.take() {
                        buf.push(pending);
                    }
                }
                if !handle.text_pending_bytes.is_empty() {
                    let pending = std::mem::take(&mut handle.text_pending_bytes);
                    buf.extend_from_slice(&pending);
                }
                let (mut more, _at_eof) = match buffered_read_bytes(_py, handle, backend, None) {
                    Ok(val) => val,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                split_text_pending_bytes(handle, &mut buf, true, encoding_kind);
                let text_bytes = if multibyte {
                    match decode_multibyte_text(
                        _py,
                        handle,
                        &mut encoding_label,
                        &mut encoding_kind,
                        errors.as_str(),
                        &buf,
                        true,
                    ) {
                        Ok(text_bytes) => text_bytes,
                        Err(bits) => return bits,
                    }
                } else {
                    update_newlines_from_bytes(handle, &buf);
                    let bytes = if handle.newline.is_none() {
                        translate_universal_newlines(&buf)
                    } else {
                        buf
                    };
                    match decode_text_bytes_for_io(
                        _py,
                        handle,
                        encoding_label.as_str(),
                        errors.as_str(),
                        &bytes,
                    ) {
                        Ok((text_bytes, _label)) => text_bytes,
                        Err(bits) => return bits,
                    }
                };
                out_text.extend_from_slice(&text_bytes);
                let out_ptr = alloc_string(_py, &out_text);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let mut buf = Vec::new();
                if let Some(pending) = handle.pending_byte.take() {
                    buf.push(pending);
                }
                let (mut more, _at_eof) = match buffered_read_bytes(_py, handle, backend, None) {
                    Ok(val) => val,
                    Err(bits) => return bits,
                };
                buf.append(&mut more);
                let out_ptr = alloc_bytes(_py, &buf);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[derive(Default)]
struct Utf8CountState {
    remaining: u8,
}

fn utf8_char_width(first: u8) -> u8 {
    if first < 0x80 {
        1
    } else if (0xC0..=0xDF).contains(&first) {
        2
    } else if (0xE0..=0xEF).contains(&first) {
        3
    } else if (0xF0..=0xF7).contains(&first) {
        4
    } else {
        1
    }
}

fn utf8_count_push(state: &mut Utf8CountState, byte: u8, count: &mut usize) {
    if state.remaining == 0 {
        let width = utf8_char_width(byte);
        if width <= 1 {
            *count += 1;
        } else {
            state.remaining = width - 1;
        }
        return;
    }
    state.remaining = state.remaining.saturating_sub(1);
    if state.remaining == 0 {
        *count += 1;
    }
}

fn push_text_byte(
    out: &mut Vec<u8>,
    byte: u8,
    kind: TextEncodingKind,
    limit: Option<usize>,
    count: &mut usize,
    utf8_state: &mut Utf8CountState,
) -> bool {
    out.push(byte);
    match kind {
        TextEncodingKind::Utf8 | TextEncodingKind::Utf16 | TextEncodingKind::Utf32 => {
            utf8_count_push(utf8_state, byte, count)
        }
        TextEncodingKind::Ascii | TextEncodingKind::Latin1 => {
            *count += 1;
        }
    }
    match limit {
        Some(limit) => *count >= limit,
        None => false,
    }
}

fn file_readline_bytes(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    newline: Option<&str>,
    text: bool,
    size: Option<usize>,
    encoding_kind: Option<TextEncodingKind>,
) -> Result<Vec<u8>, u64> {
    let mut out: Vec<u8> = Vec::new();
    let mut char_count = 0usize;
    let mut utf8_state = Utf8CountState::default();
    let text_kind = encoding_kind.unwrap_or(TextEncodingKind::Utf8);
    loop {
        if let Some(limit) = size {
            if text {
                if char_count >= limit {
                    break;
                }
            } else if out.len() >= limit {
                break;
            }
        }
        let Some(byte) = unsafe { handle_read_byte(_py, handle, backend) }? else {
            break;
        };
        if text {
            match newline {
                None => {
                    if byte == b'\n' {
                        record_newline(handle, NEWLINE_KIND_LF);
                        push_text_byte(
                            &mut out,
                            b'\n',
                            text_kind,
                            size,
                            &mut char_count,
                            &mut utf8_state,
                        );
                        break;
                    }
                    if byte == b'\r' {
                        if let Some(next) = unsafe { handle_read_byte(_py, handle, backend) }? {
                            if next == b'\n' {
                                record_newline(handle, NEWLINE_KIND_CRLF);
                            } else {
                                record_newline(handle, NEWLINE_KIND_CR);
                                handle.pending_byte = Some(next);
                            }
                        } else {
                            record_newline(handle, NEWLINE_KIND_CR);
                        }
                        push_text_byte(
                            &mut out,
                            b'\n',
                            text_kind,
                            size,
                            &mut char_count,
                            &mut utf8_state,
                        );
                        break;
                    }
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                }
                Some("") => {
                    if byte == b'\n' {
                        record_newline(handle, NEWLINE_KIND_LF);
                        push_text_byte(
                            &mut out,
                            b'\n',
                            text_kind,
                            size,
                            &mut char_count,
                            &mut utf8_state,
                        );
                        break;
                    }
                    if byte == b'\r' {
                        if let Some(next) = unsafe { handle_read_byte(_py, handle, backend) }? {
                            if next == b'\n' {
                                record_newline(handle, NEWLINE_KIND_CRLF);
                                if push_text_byte(
                                    &mut out,
                                    b'\r',
                                    text_kind,
                                    size,
                                    &mut char_count,
                                    &mut utf8_state,
                                ) {
                                    handle.pending_byte = Some(next);
                                    break;
                                }
                                push_text_byte(
                                    &mut out,
                                    b'\n',
                                    text_kind,
                                    size,
                                    &mut char_count,
                                    &mut utf8_state,
                                );
                                break;
                            }
                            record_newline(handle, NEWLINE_KIND_CR);
                            handle.pending_byte = Some(next);
                        } else {
                            record_newline(handle, NEWLINE_KIND_CR);
                        }
                        push_text_byte(
                            &mut out,
                            b'\r',
                            text_kind,
                            size,
                            &mut char_count,
                            &mut utf8_state,
                        );
                        break;
                    }
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                }
                Some("\n") => {
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                    if byte == b'\n' {
                        break;
                    }
                }
                Some("\r") => {
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                    if byte == b'\r' {
                        break;
                    }
                }
                Some("\r\n") => {
                    if byte == b'\r' {
                        if let Some(next) = unsafe { handle_read_byte(_py, handle, backend) }? {
                            if next == b'\n' {
                                if push_text_byte(
                                    &mut out,
                                    b'\r',
                                    text_kind,
                                    size,
                                    &mut char_count,
                                    &mut utf8_state,
                                ) {
                                    handle.pending_byte = Some(next);
                                    break;
                                }
                                push_text_byte(
                                    &mut out,
                                    b'\n',
                                    text_kind,
                                    size,
                                    &mut char_count,
                                    &mut utf8_state,
                                );
                                break;
                            }
                            handle.pending_byte = Some(next);
                        }
                    }
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                }
                Some(_) => {
                    if push_text_byte(
                        &mut out,
                        byte,
                        text_kind,
                        size,
                        &mut char_count,
                        &mut utf8_state,
                    ) {
                        break;
                    }
                }
            }
        } else {
            out.push(byte);
            if byte == b'\n' {
                break;
            }
            if let Some(limit) = size {
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
    Ok(out)
}

unsafe fn read_text_chunk_multibyte(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    encoding_label: &mut String,
    encoding_kind: &mut TextEncodingKind,
    errors: &str,
) -> Result<(Vec<u8>, bool), u64> {
    unsafe {
        let mut buf = Vec::new();
        if !handle.text_pending_bytes.is_empty() {
            let pending = std::mem::take(&mut handle.text_pending_bytes);
            buf.extend_from_slice(&pending);
        }
        let (mut more, at_eof) = file_read1_bytes(_py, handle, backend, None)?;
        buf.append(&mut more);
        split_text_pending_bytes(handle, &mut buf, at_eof, *encoding_kind);
        let text_bytes = decode_multibyte_text(
            _py,
            handle,
            encoding_label,
            encoding_kind,
            errors,
            &buf,
            at_eof,
        )?;
        Ok((text_bytes, at_eof))
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn read_line_multibyte(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    newline: Option<&str>,
    size: Option<usize>,
    encoding_label: &mut String,
    encoding_kind: &mut TextEncodingKind,
    errors: &str,
) -> Result<Vec<u8>, u64> {
    unsafe {
        let mut out: Vec<u8> = Vec::new();
        let mut remaining = size;
        if !handle.text_pending_text.is_empty() {
            let mut take_len = handle.text_pending_text.len();
            let mut stop = false;
            if let Some(boundary) = pending_text_line_end(&handle.text_pending_text, newline) {
                take_len = boundary;
                stop = true;
            }
            if let Some(limit) = remaining {
                let split = wtf8_split_index(&handle.text_pending_text, limit);
                if split < take_len {
                    take_len = split;
                    stop = true;
                }
            }
            out.extend_from_slice(&handle.text_pending_text[..take_len]);
            let rest = handle.text_pending_text.split_off(take_len);
            handle.text_pending_text = rest;
            if let Some(limit) = remaining {
                let taken = wtf8_char_count(&out);
                remaining = Some(limit.saturating_sub(taken));
            }
            if stop || remaining == Some(0) {
                return Ok(out);
            }
        }
        loop {
            let (chunk, at_eof) = read_text_chunk_multibyte(
                _py,
                handle,
                backend,
                encoding_label,
                encoding_kind,
                errors,
            )?;
            if chunk.is_empty() && at_eof {
                break;
            }
            let mut take_len = chunk.len();
            let mut stop = false;
            if let Some(boundary) = pending_text_line_end(&chunk, newline) {
                take_len = boundary;
                stop = true;
            }
            if let Some(limit) = remaining {
                let split = wtf8_split_index(&chunk, limit);
                if split < take_len {
                    take_len = split;
                    stop = true;
                }
            }
            out.extend_from_slice(&chunk[..take_len]);
            let rest = chunk[take_len..].to_vec();
            if let Some(limit) = remaining {
                let taken = wtf8_char_count(&chunk[..take_len]);
                remaining = Some(limit.saturating_sub(taken));
            }
            if stop {
                handle.text_pending_text = rest;
                break;
            }
            if !rest.is_empty() {
                handle.text_pending_text = rest;
                break;
            }
            if remaining == Some(0) {
                break;
            }
            if at_eof {
                break;
            }
        }
        Ok(out)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readline(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let text = match text_backend_readline(_py, handle, backend, size) {
                        Ok(text) => text,
                        Err(bits) => return bits,
                    };
                    let out_ptr = alloc_string(_py, text.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
            let mut pending_out: Vec<u8> = Vec::new();
            let mut remaining = size;
            if handle.text && !handle.text_pending_text.is_empty() {
                if let Some(0) = remaining {
                    let out_ptr = alloc_string(_py, b"");
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                let newline = handle.newline.as_deref();
                let mut take_len = handle.text_pending_text.len();
                let mut stop = false;
                if let Some(boundary) = pending_text_line_end(&handle.text_pending_text, newline) {
                    take_len = boundary;
                    stop = true;
                }
                if let Some(limit) = remaining {
                    let split = wtf8_split_index(&handle.text_pending_text, limit);
                    if split < take_len {
                        take_len = split;
                        stop = true;
                    }
                }
                pending_out.extend_from_slice(&handle.text_pending_text[..take_len]);
                let rest = handle.text_pending_text.split_off(take_len);
                handle.text_pending_text = rest;
                if let Some(limit) = remaining {
                    let taken = wtf8_char_count(&pending_out);
                    remaining = Some(limit.saturating_sub(taken));
                }
                if stop || remaining == Some(0) {
                    let out_ptr = alloc_string(_py, &pending_out);
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            let text = handle.text;
            let newline_owned = if text {
                handle.newline.clone()
            } else {
                Some("\n".to_string())
            };
            let newline = newline_owned.as_deref();
            let mut encoding_label = if text {
                handle
                    .encoding
                    .clone()
                    .unwrap_or_else(|| "utf-8".to_string())
            } else {
                "utf-8".to_string()
            };
            let encoding_kind = if text {
                Some(text_encoding_kind(&encoding_label))
            } else {
                None
            };
            if text {
                if let Some(kind_value) = encoding_kind {
                    if text_encoding_is_multibyte(kind_value) {
                        let mut kind = kind_value;
                        let errors_owned = handle
                            .errors
                            .clone()
                            .unwrap_or_else(|| "strict".to_string());
                        let errors = errors_owned.as_str();
                        if let Err(msg) = validate_decode_error_handler(errors) {
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                        let line = match read_line_multibyte(
                            _py,
                            handle,
                            backend,
                            newline,
                            remaining,
                            &mut encoding_label,
                            &mut kind,
                            errors,
                        ) {
                            Ok(line) => line,
                            Err(bits) => return bits,
                        };
                        pending_out.extend_from_slice(&line);
                        let out_ptr = alloc_string(_py, &pending_out);
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                }
            }
            let bytes = match file_readline_bytes(
                _py,
                handle,
                backend,
                newline,
                text,
                remaining,
                encoding_kind,
            ) {
                Ok(bytes) => bytes,
                Err(_) => {
                    return raise_exception::<_>(_py, "OSError", "read failed");
                }
            };
            if text {
                let errors = handle.errors.as_deref().unwrap_or("strict");
                if let Err(msg) = validate_decode_error_handler(errors) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let text_bytes =
                    match crate::object::ops::decode_bytes_text(&encoding_label, errors, &bytes) {
                        Ok((text_bytes, _label)) => text_bytes,
                        Err(crate::object::ops::DecodeTextError::UnknownEncoding(name)) => {
                            let msg = format!("unknown encoding: {name}");
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                        Err(crate::object::ops::DecodeTextError::UnknownErrorHandler(name)) => {
                            let msg = format!("unknown error handler name '{name}'");
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                        Err(crate::object::ops::DecodeTextError::Failure(
                            DecodeFailure::Byte { pos, byte, message },
                            label,
                        )) => {
                            let msg = decode_error_byte(&label, byte, pos, message);
                            return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                        }
                        Err(crate::object::ops::DecodeTextError::Failure(
                            DecodeFailure::Range {
                                start,
                                end,
                                message,
                            },
                            label,
                        )) => {
                            let msg = decode_error_range(&label, start, end, message);
                            return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                        }
                        Err(crate::object::ops::DecodeTextError::Failure(
                            DecodeFailure::UnknownErrorHandler(name),
                            _label,
                        )) => {
                            let msg = format!("unknown error handler name '{name}'");
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                    };
                pending_out.extend_from_slice(&text_bytes);
                let out_ptr = alloc_string(_py, &pending_out);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            } else {
                let out_ptr = alloc_bytes(_py, &bytes);
                if out_ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(out_ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readlines(handle_bits: u64, hint_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            let hint_obj = obj_from_bits(hint_bits);
            let hint = if hint_obj.is_none() {
                None
            } else {
                match to_i64(hint_obj) {
                    Some(val) if val <= 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, hint_bits));
                        let msg = format!("argument should be integer or None, not '{type_name}'");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let mut lines: Vec<u64> = Vec::new();
                    let mut total = 0usize;
                    loop {
                        let text = match text_backend_readline(_py, handle, backend, None) {
                            Ok(text) => text,
                            Err(bits) => return bits,
                        };
                        if text.is_empty() {
                            break;
                        }
                        total = total.saturating_add(text.chars().count());
                        let line_ptr = alloc_string(_py, text.as_bytes());
                        if line_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        lines.push(MoltObject::from_ptr(line_ptr).bits());
                        if let Some(limit) = hint {
                            if total >= limit {
                                break;
                            }
                        }
                    }
                    let list_ptr = alloc_list(_py, lines.as_slice());
                    if list_ptr.is_null() {
                        for bits in lines {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    for bits in lines {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::from_ptr(list_ptr).bits();
                }
            }
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            let text = handle.text;
            let newline_owned = if text {
                handle.newline.clone()
            } else {
                Some("\n".to_string())
            };
            let newline = newline_owned.as_deref();
            let mut encoding_label = if text {
                handle.encoding.as_deref().unwrap_or("utf-8").to_string()
            } else {
                "utf-8".to_string()
            };
            let mut encoding_kind = if text {
                Some(text_encoding_kind(&encoding_label))
            } else {
                None
            };
            let mut lines: Vec<u64> = Vec::new();
            let mut total = 0usize;
            loop {
                if text {
                    if let Some(kind_value) = encoding_kind {
                        if text_encoding_is_multibyte(kind_value) {
                            let mut kind = kind_value;
                            let errors_owned = handle
                                .errors
                                .clone()
                                .unwrap_or_else(|| "strict".to_string());
                            let errors = errors_owned.as_str();
                            if let Err(msg) = validate_decode_error_handler(errors) {
                                return raise_exception::<_>(_py, "LookupError", &msg);
                            }
                            let line = match read_line_multibyte(
                                _py,
                                handle,
                                backend,
                                newline,
                                None,
                                &mut encoding_label,
                                &mut kind,
                                errors,
                            ) {
                                Ok(line) => line,
                                Err(bits) => return bits,
                            };
                            encoding_kind = Some(kind);
                            if line.is_empty() {
                                break;
                            }
                            let char_count = match std::str::from_utf8(&line) {
                                Ok(text) => text.chars().count(),
                                Err(_) => line.len(),
                            };
                            let line_ptr = alloc_string(_py, &line);
                            if line_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            total = total.saturating_add(char_count);
                            lines.push(MoltObject::from_ptr(line_ptr).bits());
                            if let Some(limit) = hint {
                                if total >= limit {
                                    break;
                                }
                            }
                            continue;
                        }
                    }
                    let mut pending_out: Vec<u8> = Vec::new();
                    let mut line_complete = false;
                    if !handle.text_pending_text.is_empty() {
                        if let Some(boundary) =
                            pending_text_line_end(&handle.text_pending_text, newline)
                        {
                            pending_out.extend_from_slice(&handle.text_pending_text[..boundary]);
                            let rest = handle.text_pending_text.split_off(boundary);
                            handle.text_pending_text = rest;
                            line_complete = true;
                        } else {
                            pending_out.extend_from_slice(&handle.text_pending_text);
                            handle.text_pending_text.clear();
                        }
                    }
                    if !line_complete {
                        let bytes = match file_readline_bytes(
                            _py,
                            handle,
                            backend,
                            newline,
                            text,
                            None,
                            encoding_kind,
                        ) {
                            Ok(bytes) => bytes,
                            Err(_) => {
                                return raise_exception::<_>(_py, "OSError", "read failed");
                            }
                        };
                        if bytes.is_empty() && pending_out.is_empty() {
                            break;
                        }
                        let errors = handle.errors.as_deref().unwrap_or("strict");
                        if let Err(msg) = validate_decode_error_handler(errors) {
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                        let encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                        let text_bytes = match crate::object::ops::decode_bytes_text(
                            encoding_label,
                            errors,
                            &bytes,
                        ) {
                            Ok((text_bytes, _label)) => text_bytes,
                            Err(crate::object::ops::DecodeTextError::UnknownEncoding(name)) => {
                                let msg = format!("unknown encoding: {name}");
                                return raise_exception::<_>(_py, "LookupError", &msg);
                            }
                            Err(crate::object::ops::DecodeTextError::UnknownErrorHandler(name)) => {
                                let msg = format!("unknown error handler name '{name}'");
                                return raise_exception::<_>(_py, "LookupError", &msg);
                            }
                            Err(crate::object::ops::DecodeTextError::Failure(
                                DecodeFailure::Byte { pos, byte, message },
                                label,
                            )) => {
                                let msg = decode_error_byte(&label, byte, pos, message);
                                return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                            }
                            Err(crate::object::ops::DecodeTextError::Failure(
                                DecodeFailure::Range {
                                    start,
                                    end,
                                    message,
                                },
                                label,
                            )) => {
                                let msg = decode_error_range(&label, start, end, message);
                                return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
                            }
                            Err(crate::object::ops::DecodeTextError::Failure(
                                DecodeFailure::UnknownErrorHandler(name),
                                _label,
                            )) => {
                                let msg = format!("unknown error handler name '{name}'");
                                return raise_exception::<_>(_py, "LookupError", &msg);
                            }
                        };
                        pending_out.extend_from_slice(&text_bytes);
                    }
                    let char_count = match std::str::from_utf8(&pending_out) {
                        Ok(text) => text.chars().count(),
                        Err(_) => pending_out.len(),
                    };
                    let line_ptr = alloc_string(_py, &pending_out);
                    if line_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    total = total.saturating_add(char_count);
                    lines.push(MoltObject::from_ptr(line_ptr).bits());
                } else {
                    let bytes = match file_readline_bytes(
                        _py,
                        handle,
                        backend,
                        newline,
                        text,
                        None,
                        encoding_kind,
                    ) {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            return raise_exception::<_>(_py, "OSError", "read failed");
                        }
                    };
                    if bytes.is_empty() {
                        break;
                    }
                    let line_ptr = alloc_bytes(_py, &bytes);
                    if line_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    total = total.saturating_add(bytes.len());
                    lines.push(MoltObject::from_ptr(line_ptr).bits());
                }
                if let Some(limit) = hint {
                    if total >= limit {
                        break;
                    }
                }
            }
            let list_ptr = alloc_list(_py, lines.as_slice());
            if list_ptr.is_null() {
                for bits in lines {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            for bits in lines {
                dec_ref_bits(_py, bits);
            }
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

fn file_readinto_impl(_py: &PyToken<'_>, handle_bits: u64, buffer_bits: u64, name: &str) -> u64 {
    let handle_obj = obj_from_bits(handle_bits);
    let Some(ptr) = handle_obj.as_ptr() else {
        return raise_exception::<_>(_py, "TypeError", "expected file handle");
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        }
        let handle_ptr = file_handle_ptr(ptr);
        if handle_ptr.is_null() {
            return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
        }
        let handle = &mut *handle_ptr;
        file_handle_require_attached!(_py, handle);
        if file_handle_is_closed(handle) {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        }
        if !handle.readable {
            return raise_exception::<_>(_py, "UnsupportedOperation", "read");
        }
        if handle.text {
            let msg = format!("{name}() unsupported for text files");
            return raise_exception::<_>(_py, "OSError", &msg);
        }
        let mut export = BufferExport {
            ptr: 0,
            len: 0,
            readonly: 0,
            stride: 0,
            itemsize: 0,
        };
        if molt_buffer_export(buffer_bits, &mut export) != 0 || export.readonly != 0 {
            let msg = format!("{name}() argument must be a writable bytes-like object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if export.itemsize != 1 || export.stride != 1 {
            let msg = format!("{name}() argument must be a writable bytes-like object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let len = export.len as usize;
        if len == 0 {
            return MoltObject::from_int(0).bits();
        }
        let buf = std::slice::from_raw_parts_mut(export.ptr as *mut u8, len);
        let backend_state = Arc::clone(&handle.state);
        let mut guard = backend_state.backend.lock().unwrap();
        let Some(backend) = guard.as_mut() else {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        };
        let n = match buffered_read_into(_py, handle, backend, buf) {
            Ok(n) => n,
            Err(bits) => return bits,
        };
        MoltObject::from_int(n as i64).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readinto(handle_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        file_readinto_impl(_py, handle_bits, buffer_bits, "readinto")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readinto1(handle_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        file_readinto_impl(_py, handle_bits, buffer_bits, "readinto1")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_peek(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            if handle.text {
                return raise_exception::<_>(_py, "UnsupportedOperation", "peek");
            }
            if handle.buffer_size <= 0 {
                return raise_exception::<_>(_py, "UnsupportedOperation", "peek");
            }
            let size_obj = obj_from_bits(size_bits);
            let size = if size_obj.is_none() {
                None
            } else {
                match to_i64(size_obj) {
                    Some(val) if val < 0 => None,
                    Some(val) => Some(val as usize),
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            };
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            if unread_bytes(handle) == 0 {
                let buf_size = handle.buffer_size as usize;
                handle.read_buf.resize(buf_size, 0);
                let n =
                    match backend_read_bytes(_py, handle.mem_bits, backend, &mut handle.read_buf) {
                        Ok(n) => n,
                        Err(bits) => return bits,
                    };
                handle.read_buf.truncate(n);
                handle.read_pos = 0;
            }
            let available = unread_bytes(handle);
            let take = size.unwrap_or(available).min(available);
            let out = if take == 0 {
                Vec::new()
            } else {
                let start = handle.read_pos;
                handle.read_buf[start..start + take].to_vec()
            };
            let out_ptr = alloc_bytes(_py, &out);
            if out_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(out_ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_getvalue(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let builtins = builtin_classes(_py);
            if handle.class_bits != builtins.bytes_io && handle.class_bits != builtins.string_io {
                return raise_exception::<_>(_py, "UnsupportedOperation", "getvalue");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            if handle.class_bits == builtins.string_io {
                let text = match text_backend_getvalue(_py, backend) {
                    Ok(text) => text,
                    Err(bits) => return bits,
                };
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            } else {
                let data = match memory_backend_vec_ref(_py, handle) {
                    Ok(data) => data,
                    Err(bits) => return bits,
                };
                let ptr = alloc_bytes(_py, data);
                if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_getbuffer(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let builtins = builtin_classes(_py);
            if handle.class_bits != builtins.bytes_io {
                return raise_exception::<_>(_py, "UnsupportedOperation", "getbuffer");
            }
            let mem_bits = handle.mem_bits;
            if mem_bits == 0 {
                return raise_exception::<_>(_py, "RuntimeError", "memory backend missing");
            }
            molt_memoryview_new(mem_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_detach(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            if handle.detached {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    file_handle_detached_message(handle),
                );
            }
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let builtins = builtin_classes(_py);
            if handle.class_bits == builtins.bytes_io || handle.class_bits == builtins.string_io {
                return raise_exception::<_>(
                    _py,
                    "UnsupportedOperation",
                    "detach is unsupported for in-memory streams",
                );
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            if handle.text {
                let buffer_bits = handle.buffer_bits;
                if buffer_bits == 0 {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        file_handle_detached_message(handle),
                    );
                }
                let buffer_obj = obj_from_bits(buffer_bits);
                if let Some(buffer_ptr) = buffer_obj.as_ptr() {
                    if object_type_id(buffer_ptr) == TYPE_ID_FILE_HANDLE {
                        let buffer_handle_ptr = file_handle_ptr(buffer_ptr);
                        if !buffer_handle_ptr.is_null() {
                            let buffer_handle = &mut *buffer_handle_ptr;
                            let mut prefix = Vec::new();
                            if let Some(pending) = handle.pending_byte.take() {
                                prefix.push(pending);
                            }
                            if !handle.text_pending_bytes.is_empty() {
                                prefix.extend_from_slice(&handle.text_pending_bytes);
                                handle.text_pending_bytes.clear();
                            }
                            handle.text_pending_text.clear();
                            buffer_handle.pending_byte = None;
                            buffer_handle.read_buf = std::mem::take(&mut handle.read_buf);
                            buffer_handle.read_pos = handle.read_pos;
                            handle.read_pos = 0;
                            prepend_read_buffer(buffer_handle, &prefix);
                        }
                    }
                }
                handle.buffer_bits = MoltObject::none().bits();
                handle.detached = true;
                handle.owns_fd = false;
                return buffer_bits;
            }
            let raw_ptr = alloc_file_handle_with_state(
                _py,
                Arc::clone(&handle.state),
                handle.readable,
                handle.writable,
                false,
                handle.closefd,
                handle.owns_fd,
                handle.line_buffering,
                handle.write_through,
                handle.buffer_size,
                handle.class_bits,
                handle.name_bits,
                handle.mode.clone(),
                None,
                None,
                None,
                None,
                0,
                handle.mem_bits,
            );
            if raw_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let raw_handle_ptr = file_handle_ptr(raw_ptr);
            if !raw_handle_ptr.is_null() {
                let raw_handle = &mut *raw_handle_ptr;
                let mut prefix = Vec::new();
                if let Some(pending) = handle.pending_byte.take() {
                    prefix.push(pending);
                }
                if !handle.text_pending_bytes.is_empty() {
                    prefix.extend_from_slice(&handle.text_pending_bytes);
                    handle.text_pending_bytes.clear();
                }
                handle.text_pending_text.clear();
                raw_handle.pending_byte = None;
                raw_handle.read_buf = std::mem::take(&mut handle.read_buf);
                raw_handle.read_pos = handle.read_pos;
                handle.read_pos = 0;
                prepend_read_buffer(raw_handle, &prefix);
            }
            handle.detached = true;
            handle.owns_fd = false;
            MoltObject::from_ptr(raw_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_reconfigure(
    handle_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    line_buffering_bits: u64,
    write_through_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.text {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not a text file");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            if let Err(bits) = backend_flush(_py, backend) {
                return bits;
            }
            drop(guard);

            let missing = missing_bits(_py);
            let mut new_encoding = handle.encoding.clone();
            let mut new_encoding_original = handle.encoding_original.clone();
            if encoding_bits != missing {
                if let Some(encoding) = reconfigure_arg_type(_py, encoding_bits, "encoding") {
                    let (label, _kind) = match normalize_text_encoding(&encoding) {
                        Ok(val) => val,
                        Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
                    };
                    new_encoding = Some(label.clone());
                    new_encoding_original = Some(label);
                }
            }
            let mut new_errors = handle.errors.clone();
            if errors_bits != missing {
                if let Some(errors) = reconfigure_arg_type(_py, errors_bits, "errors") {
                    new_errors = Some(errors);
                }
            }
            let mut new_newline = handle.newline.clone();
            if newline_bits != missing {
                new_newline = reconfigure_arg_newline(_py, newline_bits);
            }
            let mut new_line_buffering = handle.line_buffering;
            if line_buffering_bits != missing {
                let obj = obj_from_bits(line_buffering_bits);
                if !obj.is_none() {
                    let val = match to_i64(obj) {
                        Some(val) => val != 0,
                        None => {
                            let type_name =
                                class_name_for_error(type_of_bits(_py, line_buffering_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    new_line_buffering = val;
                }
            }
            let mut new_write_through = handle.write_through;
            if write_through_bits != missing {
                let obj = obj_from_bits(write_through_bits);
                if !obj.is_none() {
                    let val = match to_i64(obj) {
                        Some(val) => val != 0,
                        None => {
                            let type_name =
                                class_name_for_error(type_of_bits(_py, write_through_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    new_write_through = val;
                }
            }

            handle.encoding = new_encoding;
            handle.errors = new_errors;
            if encoding_bits != missing {
                handle.encoding_original = new_encoding_original;
                handle.text_bom_seen = false;
                handle.text_bom_written = false;
            }
            if encoding_bits != missing || errors_bits != missing {
                handle.text_pending_bytes.clear();
                handle.pending_byte = None;
                handle.text_pending_text.clear();
            }
            if newline_bits != missing {
                handle.pending_byte = None;
                handle.text_pending_bytes.clear();
                handle.text_pending_text.clear();
                handle.newlines_mask = 0;
                handle.newlines_len = 0;
            }
            handle.newline = new_newline;
            handle.line_buffering = new_line_buffering;
            handle.write_through = new_write_through;
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_seek(handle_bits: u64, offset_bits: u64, whence_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let whence = match to_i64(obj_from_bits(whence_bits)) {
                Some(val) => val,
                None => {
                    let type_name = class_name_for_error(type_of_bits(_py, whence_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let offset = match to_i64(obj_from_bits(offset_bits)) {
                        Some(val) => val,
                        None => {
                            let type_name = class_name_for_error(type_of_bits(_py, offset_bits));
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    let pos = match text_backend_seek(_py, backend, offset, whence) {
                        Ok(pos) => pos,
                        Err(bits) => return bits,
                    };
                    handle.pending_byte = None;
                    handle.text_pending_bytes.clear();
                    handle.text_pending_text.clear();
                    clear_read_buffer(handle);
                    clear_write_buffer(handle);
                    return MoltObject::from_int(pos).bits();
                }
                if whence == 0 {
                    let type_name = class_name_for_error(type_of_bits(_py, offset_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    let Some(value) = index_bigint_from_obj(_py, offset_bits, &msg) else {
                        return MoltObject::none().bits();
                    };
                    let cookie = match text_cookie_decode_value(value) {
                        Ok(val) => val,
                        Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
                    };
                    let pos = match backend_seek(
                        _py,
                        handle,
                        backend,
                        std::io::SeekFrom::Start(cookie.pos),
                    ) {
                        Ok(pos) => pos,
                        Err(bits) => return bits,
                    };
                    handle.pending_byte = cookie.pending_byte;
                    handle.text_pending_bytes = cookie.pending_bytes;
                    handle.text_pending_text = cookie.pending_text;
                    clear_read_buffer(handle);
                    clear_write_buffer(handle);
                    let at_start = cookie.pos == 0
                        && handle.pending_byte.is_none()
                        && handle.text_pending_bytes.is_empty()
                        && handle.text_pending_text.is_empty();
                    if let Some(original) = handle.encoding_original.as_ref() {
                        if (original == "utf-16" || original == "utf-32") && at_start {
                            handle.encoding = Some(original.clone());
                        }
                        if original == "utf-8-sig" {
                            handle.text_bom_seen = !at_start;
                            handle.text_bom_written = !at_start;
                        }
                    }
                    return match text_cookie_encode_bits(
                        _py,
                        pos,
                        handle.pending_byte,
                        &handle.text_pending_bytes,
                        &handle.text_pending_text,
                    ) {
                        Ok(bits) => bits,
                        Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
                    };
                }
            }
            let offset = match to_i64(obj_from_bits(offset_bits)) {
                Some(val) => val,
                None => {
                    let type_name = class_name_for_error(type_of_bits(_py, offset_bits));
                    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if handle.text && offset != 0 && (whence == 1 || whence == 2) {
                let msg = if whence == 1 {
                    "can't do nonzero cur-relative seeks"
                } else {
                    "can't do nonzero end-relative seeks"
                };
                return raise_exception::<_>(_py, "UnsupportedOperation", msg);
            }
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            let mut seek_offset = offset;
            if whence == 1 {
                let unread = unread_bytes(handle) as i64;
                let pending = if handle.pending_byte.is_some() { 1 } else { 0 };
                let pending = pending + handle.text_pending_bytes.len() as i64;
                seek_offset = seek_offset.saturating_sub(unread + pending);
            }
            let from = match whence {
                0 => {
                    if seek_offset < 0 {
                        let msg = format!("negative seek position {seek_offset}");
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    std::io::SeekFrom::Start(seek_offset as u64)
                }
                1 => std::io::SeekFrom::Current(seek_offset),
                2 => std::io::SeekFrom::End(seek_offset),
                _ => return raise_exception::<_>(_py, "ValueError", "invalid whence"),
            };
            let pos = match backend_seek(_py, handle, backend, from) {
                Ok(pos) => pos,
                Err(bits) => return bits,
            };
            handle.pending_byte = None;
            handle.text_pending_bytes.clear();
            handle.text_pending_text.clear();
            clear_read_buffer(handle);
            if handle.text {
                match text_cookie_encode_bits(_py, pos, None, &[], &[]) {
                    Ok(bits) => bits,
                    Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
                }
            } else {
                MoltObject::from_int(pos as i64).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_tell(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let pos = match text_backend_tell(_py, backend) {
                        Ok(pos) => pos,
                        Err(bits) => return bits,
                    };
                    return MoltObject::from_int(pos).bits();
                }
            }
            let pos = match backend_tell(_py, backend) {
                Ok(pos) => pos,
                Err(bits) => return bits,
            } as i64;
            let unread = unread_bytes(handle) as i64;
            let buffered_write = handle.write_buf.len() as i64;
            let logical = pos - unread + buffered_write;
            if handle.text {
                if logical < 0 {
                    return raise_exception::<_>(_py, "OSError", "tell failed");
                }
                match text_cookie_encode_bits(
                    _py,
                    logical as u64,
                    handle.pending_byte,
                    &handle.text_pending_bytes,
                    &handle.text_pending_text,
                ) {
                    Ok(bits) => bits,
                    Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
                }
            } else {
                MoltObject::from_int(logical).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_fileno(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_ref() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            match backend {
                MoltFileBackend::File(file) => {
                    #[cfg(unix)]
                    {
                        use std::os::fd::AsRawFd;
                        MoltObject::from_int(file.as_raw_fd() as i64).bits()
                    }
                    #[cfg(windows)]
                    {
                        let fd_guard = backend_state.crt_fd.lock().unwrap();
                        if let Some(fd) = *fd_guard {
                            MoltObject::from_int(fd).bits()
                        } else {
                            raise_exception::<_>(_py, "UnsupportedOperation", "fileno")
                        }
                    }
                    #[cfg(not(any(unix, windows)))]
                    {
                        return raise_exception::<_>(
                            _py,
                            "OSError",
                            "fileno is unsupported on this platform",
                        );
                    }
                }
                MoltFileBackend::Memory(_) | MoltFileBackend::Text(_) => {
                    raise_exception::<_>(_py, "UnsupportedOperation", "fileno")
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_truncate(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "truncate");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let size = if obj_from_bits(size_bits).is_none() {
                        match text_backend_tell(_py, backend) {
                            Ok(pos) => pos as usize,
                            Err(bits) => return bits,
                        }
                    } else {
                        let val = match to_i64(obj_from_bits(size_bits)) {
                            Some(val) => val,
                            None => {
                                let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                                let msg = format!(
                                    "'{type_name}' object cannot be interpreted as an integer"
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if val < 0 {
                            return raise_exception::<_>(_py, "OSError", "Invalid argument");
                        }
                        val as usize
                    };
                    if let Err(bits) = text_backend_truncate(_py, backend, size) {
                        return bits;
                    }
                    return MoltObject::from_int(size as i64).bits();
                }
            }
            if !handle.write_buf.is_empty() {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            let size = if obj_from_bits(size_bits).is_none() {
                let pos = match backend_tell(_py, backend) {
                    Ok(pos) => pos as i64,
                    Err(bits) => return bits,
                };
                let unread = unread_bytes(handle) as i64;
                let buffered_write = handle.write_buf.len() as i64;
                let logical = pos - unread + buffered_write;
                if logical < 0 {
                    return raise_exception::<_>(_py, "OSError", "Invalid argument");
                }
                logical as u64
            } else {
                let val = match to_i64(obj_from_bits(size_bits)) {
                    Some(val) => val,
                    None => {
                        let type_name = class_name_for_error(type_of_bits(_py, size_bits));
                        let msg =
                            format!("'{type_name}' object cannot be interpreted as an integer");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if val < 0 {
                    return raise_exception::<_>(_py, "OSError", "Invalid argument");
                }
                val as u64
            };
            if let Err(bits) = backend_truncate(_py, handle, backend, size) {
                return bits;
            }
            clear_read_buffer(handle);
            MoltObject::from_int(size as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            MoltObject::from_bool(handle.readable).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_writable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            MoltObject::from_bool(handle.writable).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_seekable(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let seekable = match backend {
                MoltFileBackend::File(file) => file.stream_position().is_ok(),
                MoltFileBackend::Memory(_) | MoltFileBackend::Text(_) => true,
            };
            MoltObject::from_bool(seekable).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_isatty(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_ref() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            match backend {
                MoltFileBackend::File(file) => {
                    #[cfg(unix)]
                    {
                        use std::os::fd::AsRawFd;
                        let isatty = libc::isatty(file.as_raw_fd()) == 1;
                        MoltObject::from_bool(isatty).bits()
                    }
                    #[cfg(windows)]
                    {
                        let fd_guard = backend_state.crt_fd.lock().unwrap();
                        if let Some(fd) = *fd_guard {
                            let isatty = unsafe { libc::isatty(fd as libc::c_int) == 1 };
                            return MoltObject::from_bool(isatty).bits();
                        }
                        use std::os::windows::io::AsRawHandle;
                        let handle = file.as_raw_handle();
                        let isatty = windows_handle_isatty(handle as *mut std::ffi::c_void);
                        MoltObject::from_bool(isatty).bits()
                    }
                    #[cfg(not(any(unix, windows)))]
                    {
                        let _ = file;
                        MoltObject::from_bool(false).bits()
                    }
                }
                MoltFileBackend::Memory(_) | MoltFileBackend::Text(_) => {
                    MoltObject::from_bool(false).bits()
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_iter(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
        }
        inc_ref_bits(_py, handle_bits);
        handle_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_next(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let line_bits = molt_file_readline(handle_bits, MoltObject::from_int(-1).bits());
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let line_obj = obj_from_bits(line_bits);
        let empty = if let Some(ptr) = line_obj.as_ptr() {
            unsafe {
                match object_type_id(ptr) {
                    TYPE_ID_STRING => string_len(ptr) == 0,
                    TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => bytes_len(ptr) == 0,
                    _ => false,
                }
            }
        } else {
            false
        };
        if empty {
            dec_ref_bits(_py, line_bits);
            return raise_exception::<_>(_py, "StopIteration", "");
        }
        line_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_enter(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            file_handle_enter(_py, ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_exit(handle_bits: u64, exc_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            file_handle_exit(_py, ptr, exc_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_exit_method(
    handle_bits: u64,
    _exc_type_bits: u64,
    exc_bits: u64,
    _tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { molt_file_exit(handle_bits, exc_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not writable");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let data_obj = obj_from_bits(data_bits);
            if handle.text {
                if let MoltFileBackend::Text(_) = backend {
                    let text = match string_obj_to_owned(data_obj) {
                        Some(text) => text,
                        None => {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "write expects str for text mode",
                            );
                        }
                    };
                    let written = match text_backend_write(_py, handle, backend, &text) {
                        Ok(count) => count,
                        Err(bits) => return bits,
                    };
                    return MoltObject::from_int(written as i64).bits();
                }
            }
            if unread_bytes(handle) > 0 {
                if let Err(bits) = rewind_read_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            let (bytes, written_len, flush_newline): (Vec<u8>, usize, bool) = if handle.text {
                let Some(data_ptr) = data_obj.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects str for text mode",
                    );
                };
                if object_type_id(data_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects str for text mode",
                    );
                }
                let raw = std::slice::from_raw_parts(string_bytes(data_ptr), string_len(data_ptr));
                let errors = handle.errors.as_deref().unwrap_or("strict");
                let newline = handle.newline.as_deref();
                if let Err(msg) = validate_encode_error_handler(errors) {
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                let translated = translate_write_newlines_bytes(raw, newline);
                let mut encoding_label = handle.encoding.as_deref().unwrap_or("utf-8");
                let mut mark_bom_written = false;
                if encoding_label == "utf-8-sig" {
                    if handle.text_bom_written {
                        encoding_label = "utf-8";
                    } else {
                        mark_bom_written = true;
                    }
                }
                let bytes = match crate::object::ops::encode_string_with_errors(
                    &translated,
                    encoding_label,
                    Some(errors),
                ) {
                    Ok(bytes) => bytes,
                    Err(crate::object::ops::EncodeError::UnknownEncoding(name)) => {
                        let msg = format!("unknown encoding: {name}");
                        return raise_exception::<_>(_py, "LookupError", &msg);
                    }
                    Err(crate::object::ops::EncodeError::UnknownErrorHandler(name)) => {
                        let msg = format!("unknown error handler name '{name}'");
                        return raise_exception::<_>(_py, "LookupError", &msg);
                    }
                    Err(crate::object::ops::EncodeError::InvalidChar {
                        encoding,
                        code,
                        pos,
                        limit,
                    }) => {
                        let reason = crate::object::ops::encode_error_reason(encoding, code, limit);
                        return raise_unicode_encode_error::<_>(
                            _py,
                            encoding,
                            data_obj.bits(),
                            pos,
                            pos + 1,
                            &reason,
                        );
                    }
                };
                if mark_bom_written {
                    handle.text_bom_written = true;
                }
                let written_len = crate::object::ops::utf8_codepoint_count_cached(
                    _py,
                    raw,
                    Some(data_ptr as usize),
                ) as usize;
                let flush_newline = translated.contains(&b'\n');
                (bytes, written_len, flush_newline)
            } else {
                let Some(data_ptr) = data_obj.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects bytes or bytearray",
                    );
                };
                let type_id = object_type_id(data_ptr);
                if type_id != TYPE_ID_BYTES && type_id != TYPE_ID_BYTEARRAY {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "write expects bytes or bytearray",
                    );
                }
                let len = bytes_len(data_ptr);
                let raw = std::slice::from_raw_parts(bytes_data(data_ptr), len);
                (raw.to_vec(), len, raw.contains(&b'\n'))
            };
            let should_flush = handle.write_through || (handle.line_buffering && flush_newline);
            if handle.buffer_size == 0 {
                let mut written = 0usize;
                while written < bytes.len() {
                    let n =
                        match backend_write_bytes(_py, handle.mem_bits, backend, &bytes[written..])
                        {
                            Ok(n) => n,
                            Err(bits) => return bits,
                        };
                    if n == 0 {
                        return raise_exception::<_>(_py, "OSError", "write failed");
                    }
                    written += n;
                }
                if should_flush {
                    if let Err(bits) = backend_flush(_py, backend) {
                        return bits;
                    }
                }
            } else {
                handle.write_buf.extend_from_slice(&bytes);
                let need_flush =
                    should_flush || handle.write_buf.len() >= handle.buffer_size as usize;
                if need_flush {
                    if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                        return bits;
                    }
                }
            }
            MoltObject::from_int(written_len as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_writelines(handle_bits: u64, lines_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if !handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not writable");
            }
        }
        let iter_bits = molt_iter(lines_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "writelines() argument must be iterable",
            );
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let line_bits = elems[0];
                let _ = molt_file_write(handle_bits, line_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_flush(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &mut *handle_ptr;
            file_handle_require_attached!(_py, handle);
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if let MoltFileBackend::Text(_) = backend {
                return MoltObject::none().bits();
            }
            if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                return bits;
            }
            if let Err(bits) = backend_flush(_py, backend) {
                return bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_FILE_HANDLE {
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            let handle = &*handle_ptr;
            file_handle_require_attached!(_py, handle);
        }
        let flush_result = unsafe {
            if let Some(handle_ptr) = file_handle_ptr(ptr).as_mut() {
                let handle = &mut *handle_ptr;
                let backend_state = Arc::clone(&handle.state);
                let mut guard = backend_state.backend.lock().unwrap();
                if let Some(backend) = guard.as_mut() {
                    flush_write_buffer(_py, handle, backend)
                } else {
                    Ok(())
                }
            } else {
                Ok(())
            }
        };
        if let Err(bits) = flush_result {
            return bits;
        }
        file_handle_close_ptr(ptr);
        MoltObject::none().bits()
    })
}
