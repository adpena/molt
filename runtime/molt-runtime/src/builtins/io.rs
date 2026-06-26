use crate::PyToken;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
#[cfg(windows)]
use crate::windows_abi::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, FILE_NAME_NORMALIZED, FILE_TYPE_CHAR,
    GetConsoleMode, GetCurrentProcess, GetFileType, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
};

// Re-export path/glob/os functions so that `io::*` includes them
#[allow(unused_imports)]
pub use super::io_path::*;
use crate::object::ops_encoding::DecodeFailure;
use crate::object::{
    MoltFileBackend, MoltMemoryBackend, MoltTextBackend, NEWLINE_KIND_CR, NEWLINE_KIND_CRLF,
    NEWLINE_KIND_LF,
};
use crate::*;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Seek, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[path = "io/text.rs"]
mod text;
use text::*;

const DEFAULT_BUFFER_SIZE: i64 = 8192;
type VfsWritebackEntry = (Arc<dyn crate::vfs::VfsBackend>, String);

pub(crate) struct IoRuntimeState {
    pub(crate) sys_stdin_handle_bits: AtomicU64,
    pub(crate) sys_stdout_handle_bits: AtomicU64,
    pub(crate) sys_stderr_handle_bits: AtomicU64,
    vfs_writebacks: Mutex<HashMap<usize, VfsWritebackEntry>>,
}

impl IoRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            sys_stdin_handle_bits: AtomicU64::new(0),
            sys_stdout_handle_bits: AtomicU64::new(0),
            sys_stderr_handle_bits: AtomicU64::new(0),
            vfs_writebacks: Mutex::new(HashMap::new()),
        }
    }

    fn stdio_slots(&self) -> [&AtomicU64; 3] {
        [
            &self.sys_stdin_handle_bits,
            &self.sys_stdout_handle_bits,
            &self.sys_stderr_handle_bits,
        ]
    }
}

fn vfs_writeback_register(_py: &PyToken<'_>, state: &Arc<MoltFileState>, entry: VfsWritebackEntry) {
    let key = Arc::as_ptr(state) as usize;
    runtime_state(_py)
        .io
        .vfs_writebacks
        .lock()
        .unwrap()
        .insert(key, entry);
}

fn vfs_writeback_take(_py: &PyToken<'_>, state: &Arc<MoltFileState>) -> Option<VfsWritebackEntry> {
    let key = Arc::as_ptr(state) as usize;
    runtime_state(_py)
        .io
        .vfs_writebacks
        .lock()
        .unwrap()
        .remove(&key)
}

pub(crate) fn io_clear_runtime_state(_py: &PyToken<'_>, state: &crate::state::RuntimeState) {
    crate::gil_assert();
    for slot in state.io.stdio_slots() {
        let bits = slot.swap(0, Ordering::AcqRel);
        if bits != 0 && !obj_from_bits(bits).is_none() {
            let _ = molt_file_flush(bits);
            if exception_pending(_py) {
                clear_exception(_py);
            }
            dec_ref_bits(_py, bits);
        }
    }
    state.io.vfs_writebacks.lock().unwrap().clear();
}

macro_rules! file_handle_require_attached {
    ($py:expr, $handle:expr) => {
        if $handle.detached {
            return raise_exception::<_>($py, "ValueError", file_handle_detached_message($handle));
        }
    };
}

fn resolve_file_handle_ptr(_py: &PyToken<'_>, obj_bits: u64) -> Result<*mut MoltFileHandle, u64> {
    let obj = obj_from_bits(obj_bits);
    if let Some(ptr) = obj.as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_FILE_HANDLE
    {
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
    let name_bits = intern_static_name(_py, &runtime_state(_py).interned.handle_name, b"_handle");
    let missing = missing_bits(_py);
    let attr_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !is_missing_bits(_py, attr_bits) {
        let mut resolved = None;
        if let Some(attr_ptr) = obj_from_bits(attr_bits).as_ptr()
            && unsafe { object_type_id(attr_ptr) } == TYPE_ID_FILE_HANDLE
        {
            let handle_ptr = unsafe { file_handle_ptr(attr_ptr) };
            if !handle_ptr.is_null() {
                resolved = Some(handle_ptr);
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
    if class_bits != 0 {
        // Ensure `type(handle)` and attribute resolution go through the intended IO wrapper class
        // (TextIOWrapper / Buffered* / FileIO), rather than falling back to `object`.
        unsafe {
            object_set_class_bits(_py, ptr, class_bits);
        }
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

pub(crate) fn file_handle_close_ptr(ptr: *mut u8) -> bool {
    if ptr.is_null() {
        return false;
    }
    unsafe {
        let debug_close = std::env::var("MOLT_DEBUG_FILE_CLOSE").as_deref() == Ok("1");
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
        if debug_close {
            eprintln!(
                "molt file_handle_close ptr=0x{:x} closefd={} owns_fd={} had_backend={}",
                ptr as usize, handle.closefd, handle.owns_fd, had_backend
            );
        }
        #[cfg(windows)]
        if had_backend {
            let mut fd_guard = backend_state.crt_fd.lock().unwrap();
            if let Some(fd) = fd_guard.take() {
                libc::close(fd as libc::c_int);
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

pub(crate) unsafe fn collect_bytes_like(_py: &PyToken<'_>, bits: u64) -> Result<Vec<u8>, u64> {
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

fn file_remaining_bytes_hint(file: &mut std::fs::File) -> Option<usize> {
    let pos = file.stream_position().ok()?;
    let len = file.metadata().ok()?.len();
    usize::try_from(len.saturating_sub(pos)).ok()
}

unsafe fn buffered_read_reserve_hint(
    _py: &PyToken<'_>,
    handle: &mut MoltFileHandle,
    backend: &mut MoltFileBackend,
    size: Option<usize>,
) -> Option<usize> {
    unsafe {
        if let Some(size) = size {
            return Some(size.saturating_add(unread_bytes(handle)));
        }
        let unread = unread_bytes(handle);
        match backend {
            MoltFileBackend::File(file) => {
                file_remaining_bytes_hint(file).map(|n| n.saturating_add(unread))
            }
            MoltFileBackend::Memory(mem) => memory_backend_vec_ref_from_bits(_py, handle.mem_bits)
                .ok()
                .map(|data| data.len().saturating_sub(mem.pos).saturating_add(unread)),
            MoltFileBackend::Text(_) => None,
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
        if !handle.write_buf.is_empty() {
            flush_write_buffer(_py, handle, backend)?;
        }
        if size.is_none() {
            let unread = unread_bytes(handle);
            if let MoltFileBackend::File(file) = backend {
                let reserve = file_remaining_bytes_hint(file)
                    .unwrap_or(0)
                    .saturating_add(unread);
                let mut out = Vec::with_capacity(reserve);
                if unread > 0 {
                    let start = handle.read_pos;
                    out.extend_from_slice(&handle.read_buf[start..]);
                    clear_read_buffer(handle);
                }
                match file.read_to_end(&mut out) {
                    Ok(_) => return Ok((out, true)),
                    Err(_) => return Err(raise_exception::<_>(_py, "OSError", "read failed")),
                }
            }
        }

        if handle.buffer_size == 0 {
            let mut buf = Vec::with_capacity(
                buffered_read_reserve_hint(_py, handle, backend, size).unwrap_or(0),
            );
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

        let mut out =
            Vec::with_capacity(buffered_read_reserve_hint(_py, handle, backend, size).unwrap_or(0));
        let mut at_eof = false;
        let mut remaining = size;
        if let Some(rem) = remaining
            && rem == 0
        {
            return Ok((out, false));
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
                if let Some(backend) = guard.as_mut()
                    && let Err(bits) = flush_write_buffer(_py, handle, backend)
                {
                    return bits;
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
    // `File::from_raw_fd` will happily wrap an invalid fd; validate upfront so
    // `open(fd)` matches CPython and raises immediately for EBADF.
    let rc = unsafe { libc::fcntl(fd as libc::c_int, libc::F_GETFD) };
    if rc < 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_fd(fd as i32) })
}

#[cfg(windows)]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let handle = unsafe { libc::get_osfhandle(fd as libc::c_int) };
    if handle == -1 {
        return None;
    }
    let dup = duplicate_handle(handle as *mut std::ffi::c_void)?;
    Some(unsafe { std::fs::File::from_raw_handle(dup as *mut _) })
}

#[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
fn file_from_fd(fd: i64) -> Option<std::fs::File> {
    use std::os::wasi::io::FromRawFd;
    if fd < 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_fd(fd as std::os::wasi::io::RawFd) })
}

#[cfg(all(
    not(any(unix, windows)),
    not(all(target_arch = "wasm32", target_os = "wasi"))
))]
fn file_from_fd(_fd: i64) -> Option<std::fs::File> {
    None
}

#[cfg(unix)]
pub(crate) fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd as libc::c_int) };
    if duped < 0 { None } else { Some(duped as i64) }
}

#[cfg(windows)]
pub(crate) fn dup_fd(fd: i64) -> Option<i64> {
    if fd < 0 {
        return None;
    }
    let duped = unsafe { libc::dup(fd as libc::c_int) };
    if duped < 0 { None } else { Some(duped as i64) }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn dup_fd(_fd: i64) -> Option<i64> {
    None
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
pub(crate) fn windows_path_from_handle(handle: *mut std::ffi::c_void) -> Option<String> {
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
    let fd = unsafe { libc::open_osfhandle(dup as isize, flags) };
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

// Path, glob, and filesystem utilities extracted to io_path_utils.rs.
pub(crate) use super::io_path_utils::*;

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
                if let Some(res_ptr) = res_obj.as_ptr()
                    && object_type_id(res_ptr) == TYPE_ID_BYTES
                {
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

    let debug_open_fd = std::env::var("MOLT_DEBUG_OPEN_FD").as_deref() == Ok("1");

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
    let mut debug_fd_value: Option<i64> = None;
    let path_name_bits = if let Some(i) = to_i64(obj_from_bits(file_bits)) {
        if i < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative file descriptor");
        }
        fd = Some(i);
        if debug_open_fd {
            debug_fd_value = Some(i);
        }
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
            // VFS dispatch (Plan B v0.1)
            // If the path resolves through a VFS mount, serve the read
            // from the in-memory backend rather than the real filesystem.
            let path_str = path.to_string_lossy();
            if let Some(vfs) = runtime_state(_py).get_vfs()
                && let Some((mount_prefix, backend, rel_path)) = vfs.resolve(&path_str)
            {
                let is_write = mode_info.writable;
                // Capability check
                let cap_result = crate::vfs::caps::check_mount_capability(
                    &mount_prefix,
                    is_write,
                    &|cap_name| has_capability(_py, cap_name),
                );
                if let Err(vfs_err) = cap_result {
                    let msg = format!("{vfs_err}: '{path_str}'");
                    return raise_exception::<_>(_py, "PermissionError", &msg);
                }
                // VFS read / write dispatch
                // For reads: load existing file content into a bytearray.
                // For writes: start with empty (truncate) or existing
                //   (append) content, and register a writeback entry so
                //   molt_file_close flushes the final bytearray content
                //   back to the VFS backend.
                let data: Vec<u8> = if is_write && !mode_info.append {
                    // Write-truncate: start empty.
                    Vec::new()
                } else if is_write && mode_info.append {
                    // Append: seed with existing content (if any).
                    backend.open_read(&rel_path).unwrap_or_default()
                } else {
                    // Read-only: load the full file.
                    match backend.open_read(&rel_path) {
                        Ok(bytes) => bytes,
                        Err(vfs_err) => {
                            let msg = format!("{vfs_err}: '{path_str}'");
                            return match vfs_err {
                                crate::vfs::VfsError::NotFound => {
                                    raise_exception::<_>(_py, "FileNotFoundError", &msg)
                                }
                                crate::vfs::VfsError::PermissionDenied
                                | crate::vfs::VfsError::ReadOnly
                                | crate::vfs::VfsError::CapabilityDenied(_) => {
                                    raise_exception::<_>(_py, "PermissionError", &msg)
                                }
                                crate::vfs::VfsError::IsDirectory => {
                                    raise_exception::<_>(_py, "IsADirectoryError", &msg)
                                }
                                _ => raise_exception::<_>(_py, "OSError", &msg),
                            };
                        }
                    }
                };

                // Clone the Arc before dropping the VFS lock so we can
                // register it in the writeback map for writable handles.
                let vfs_backend_arc = if is_write {
                    Some((Arc::clone(&backend), rel_path.clone()))
                } else {
                    None
                };

                // Build an in-memory file handle (like BytesIO) backed
                // by the VFS data so the rest of the runtime sees a
                // normal file object.
                let initial_pos = if mode_info.append { data.len() } else { 0 };
                let bytearray_ptr = alloc_bytearray(_py, &data);
                if bytearray_ptr.is_null() {
                    return raise_exception::<_>(_py, "OSError", "open failed");
                }
                let mem_bits = MoltObject::from_ptr(bytearray_ptr).bits();
                let vfs_state = Arc::new(MoltFileState {
                    backend: Mutex::new(Some(MoltFileBackend::Memory(MoltMemoryBackend {
                        pos: initial_pos,
                    }))),
                    #[cfg(windows)]
                    crt_fd: Mutex::new(None),
                });

                // Register VFS writeback so molt_file_close can flush
                // the bytearray content back to the VFS backend.
                if let Some(entry) = vfs_backend_arc {
                    vfs_writeback_register(_py, &vfs_state, entry);
                }

                // Reuse the same encoding / errors / newline resolution
                // that the normal path uses.
                let enc = if mode_info.text {
                    let e = encoding.unwrap_or_else(|| "utf-8".to_string());
                    let (label, _kind) = match normalize_text_encoding(&e) {
                        Ok(val) => val,
                        Err(msg) => {
                            dec_ref_bits(_py, mem_bits);
                            return raise_exception::<_>(_py, "LookupError", &msg);
                        }
                    };
                    Some(label)
                } else {
                    None
                };
                let enc_original = enc.clone();
                let errs = if mode_info.text {
                    Some(errors.unwrap_or_else(|| "strict".to_string()))
                } else {
                    None
                };

                let vfs_readable = mode_info.readable || mode_info.append;
                let vfs_writable = is_write;

                let builtins = builtin_classes(_py);
                let buffered_class_bits = if vfs_readable && vfs_writable {
                    builtins.buffered_random
                } else if vfs_writable {
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
                let buf_size = if buffering == 0 {
                    0
                } else if line_buffering || buffering < 0 {
                    DEFAULT_BUFFER_SIZE
                } else {
                    buffering
                };
                let buffer_bits = if mode_info.text {
                    let buffer_ptr = alloc_file_handle_with_state(
                        _py,
                        Arc::clone(&vfs_state),
                        vfs_readable,
                        vfs_writable,
                        false, // text
                        false, // closefd
                        true,  // owns_fd
                        false, // line_buffering
                        false, // write_through
                        buf_size,
                        buffer_class_bits,
                        path_name_bits,
                        mode.clone(),
                        None,
                        None,
                        None,
                        None,
                        0,
                        mem_bits,
                    );
                    if buffer_ptr.is_null() {
                        dec_ref_bits(_py, mem_bits);
                        return MoltObject::none().bits();
                    }
                    MoltObject::from_ptr(buffer_ptr).bits()
                } else {
                    0
                };
                let ptr = alloc_file_handle_with_state(
                    _py,
                    vfs_state,
                    vfs_readable,
                    vfs_writable,
                    mode_info.text,
                    true, // closefd
                    true, // owns_fd
                    line_buffering,
                    false, // write_through
                    buf_size,
                    handle_class_bits,
                    path_name_bits,
                    mode.clone(),
                    enc,
                    enc_original,
                    errs,
                    newline,
                    buffer_bits,
                    if mode_info.text { 0 } else { mem_bits },
                );
                dec_ref_bits(_py, mem_bits);
                if buffer_bits != 0 {
                    dec_ref_bits(_py, buffer_bits);
                }
                return if ptr.is_null() {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_ptr(ptr).bits()
                };
            }
            // End VFS dispatch
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
                    let raw_code = err.raw_os_error();
                    let msg = if let Some(code) = raw_code {
                        format!("[Errno {code}] {short}: '{path_display}'")
                    } else {
                        format!("{short}: '{path_display}'")
                    };
                    if let Some(code) = raw_code {
                        return raise_os_error_errno::<_>(_py, code as i64, &msg);
                    }
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
    if crt_fd.is_none()
        && let Some(file_ref) = file.as_ref()
    {
        use std::os::windows::io::AsRawHandle;
        let handle = file_ref.as_raw_handle();
        crt_fd = windows_crt_fd_from_handle(handle, mode_info.readable, mode_info.writable);
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
        if debug_fd_value == Some(0) && debug_open_fd {
            eprintln!(
                "molt open(fd=0) buffer_handle_ptr=0x{:x}",
                buffer_ptr as usize
            );
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
    if debug_fd_value == Some(0) && debug_open_fd && !ptr.is_null() {
        eprintln!(
            "molt open(fd=0) -> file_handle_ptr=0x{:x} closefd={}",
            ptr as usize, closefd
        );
    }
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
    let trace_stdio = std::env::var("MOLT_TRACE_STDIO_BUILD").as_deref() == Ok("1");
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
        if trace_stdio && exception_pending(_py) {
            let exc_bits = molt_exception_last();
            let kind_bits = molt_exception_kind(exc_bits);
            let kind = string_obj_to_owned(obj_from_bits(kind_bits))
                .unwrap_or_else(|| "<exc>".to_string());
            eprintln!("stdio build pending after buffer alloc fd={fd}: {kind}");
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
    if trace_stdio && exception_pending(_py) {
        let exc_bits = molt_exception_last();
        let kind_bits = molt_exception_kind(exc_bits);
        let kind =
            string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_else(|| "<exc>".to_string());
        eprintln!("stdio build pending after wrapper alloc fd={fd}: {kind}");
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
    let trace_stdio = std::env::var("MOLT_TRACE_STDIO_BUILD").as_deref() == Ok("1");
    let cached_bits = slot.load(Ordering::Acquire);
    if cached_bits != 0 && !obj_from_bits(cached_bits).is_none() {
        inc_ref_bits(_py, cached_bits);
        return cached_bits;
    }

    let handle_bits = make_handle();
    if obj_from_bits(handle_bits).is_none() {
        return handle_bits;
    }
    if trace_stdio && exception_pending(_py) {
        let exc_bits = molt_exception_last();
        let kind_bits = molt_exception_kind(exc_bits);
        let kind =
            string_obj_to_owned(obj_from_bits(kind_bits)).unwrap_or_else(|| "<exc>".to_string());
        eprintln!("stdio build pending after make_handle: {kind}");
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
    crate::with_gil_entry_nopanic!(_py, {
        cached_stdio_handle(_py, &runtime_state(_py).io.sys_stdin_handle_bits, || {
            alloc_stdio_handle(_py, 0, true, false, "<stdin>", "surrogateescape", false)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stdout() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cached_stdio_handle(_py, &runtime_state(_py).io.sys_stdout_handle_bits, || {
            alloc_stdio_handle(_py, 1, false, true, "<stdout>", "surrogateescape", false)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_stderr() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        cached_stdio_handle(_py, &runtime_state(_py).io.sys_stderr_handle_bits, || {
            alloc_stdio_handle(_py, 2, false, true, "<stderr>", "backslashreplace", true)
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_open(path_bits: u64, mode_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffered_new(cls_bits: u64, raw_bits: u64, buffer_size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytesio_new(_cls_bits: u64, initial_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_stringio_new(_cls_bits: u64, initial_bits: u64, newline_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, { MoltObject::none().bits() })
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
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
                    if !multibyte && let Some(pending) = handle.pending_byte.take() {
                        buf.push(pending);
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
                    if let Some(rem) = byte_limit
                        && pending_utf8_needed > rem
                    {
                        byte_limit = Some(pending_utf8_needed);
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
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
                if !multibyte && let Some(pending) = handle.pending_byte.take() {
                    buf.push(pending);
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
                if let Some(rem) = byte_limit
                    && pending_utf8_needed > rem
                {
                    byte_limit = Some(pending_utf8_needed);
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
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
                if !multibyte && let Some(pending) = handle.pending_byte.take() {
                    buf.push(pending);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readline(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
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
            if text
                && let Some(kind_value) = encoding_kind
                && text_encoding_is_multibyte(kind_value)
            {
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
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
                    if let Some(limit) = hint
                        && total >= limit
                    {
                        break;
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
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
                    if let Some(kind_value) = encoding_kind
                        && text_encoding_is_multibyte(kind_value)
                    {
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
                        if let Some(limit) = hint
                            && total >= limit
                        {
                            break;
                        }
                        continue;
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
                if let Some(limit) = hint
                    && total >= limit
                {
                    break;
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
            ptr: std::ptr::null_mut(),
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
        let buf = std::slice::from_raw_parts_mut(export.ptr, len);
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
    crate::with_gil_entry_nopanic!(_py, {
        file_readinto_impl(_py, handle_bits, buffer_bits, "readinto")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_readinto1(handle_bits: u64, buffer_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        file_readinto_impl(_py, handle_bits, buffer_bits, "readinto1")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_peek(handle_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
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
                if let Some(buffer_ptr) = buffer_obj.as_ptr()
                    && object_type_id(buffer_ptr) == TYPE_ID_FILE_HANDLE
                {
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
            }
            if let Err(bits) = backend_flush(_py, backend) {
                return bits;
            }
            drop(guard);

            let missing = missing_bits(_py);
            let mut new_encoding = handle.encoding.clone();
            let mut new_encoding_original = handle.encoding_original.clone();
            if encoding_bits != missing
                && let Some(encoding) = reconfigure_arg_type(_py, encoding_bits, "encoding")
            {
                let (label, _kind) = match normalize_text_encoding(&encoding) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
                };
                new_encoding = Some(label.clone());
                new_encoding_original = Some(label);
            }
            let mut new_errors = handle.errors.clone();
            if errors_bits != missing
                && let Some(errors) = reconfigure_arg_type(_py, errors_bits, "errors")
            {
                new_errors = Some(errors);
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
                let pos = match text_backend_tell(_py, backend) {
                    Ok(pos) => pos,
                    Err(bits) => return bits,
                };
                return MoltObject::from_int(pos).bits();
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
    crate::with_gil_entry_nopanic!(_py, {
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
                #[cfg(unix)]
                MoltFileBackend::File(file) => {
                    use std::os::fd::AsRawFd;
                    MoltObject::from_int(file.as_raw_fd() as i64).bits()
                }
                #[cfg(windows)]
                MoltFileBackend::File(_) => {
                    let fd_guard = backend_state.crt_fd.lock().unwrap();
                    if let Some(fd) = *fd_guard {
                        MoltObject::from_int(fd).bits()
                    } else {
                        raise_exception::<_>(_py, "UnsupportedOperation", "fileno")
                    }
                }
                #[cfg(not(any(unix, windows)))]
                MoltFileBackend::File(_) => {
                    raise_exception::<_>(_py, "OSError", "fileno is unsupported on this platform")
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
    crate::with_gil_entry_nopanic!(_py, {
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
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
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
                            let msg =
                                format!("'{type_name}' object cannot be interpreted as an integer");
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
            if !handle.write_buf.is_empty()
                && let Err(bits) = flush_write_buffer(_py, handle, backend)
            {
                return bits;
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
                            let isatty = libc::isatty(fd as libc::c_int) == 1;
                            return MoltObject::from_bool(isatty).bits();
                        }
                        use std::os::windows::io::AsRawHandle;
                        let handle = file.as_raw_handle();
                        let isatty = windows_handle_isatty(handle);
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, { molt_file_exit(handle_bits, exc_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_write(handle_bits: u64, data_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            if handle.text
                && let MoltFileBackend::Text(_) = backend
            {
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
            if unread_bytes(handle) > 0
                && let Err(bits) = rewind_read_buffer(_py, handle, backend)
            {
                return bits;
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
                let written_len = crate::object::ops_string::utf8_codepoint_count_cached(
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
                if should_flush && let Err(bits) = backend_flush(_py, backend) {
                    return bits;
                }
            } else {
                handle.write_buf.extend_from_slice(&bytes);
                let need_flush =
                    should_flush || handle.write_buf.len() >= handle.buffer_size as usize;
                if need_flush && let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            MoltObject::from_int(written_len as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_writelines(handle_bits: u64, lines_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let handle_obj = obj_from_bits(handle_bits);
        let Some(ptr) = handle_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "expected file handle");
        };
        // CPython's `TextIOWrapper.flush()` first flushes its own encode buffer
        // and then flushes the underlying buffered stream. molt flattens the
        // text wrapper and its binary buffer into sibling `MoltFileHandle`s that
        // share one backend, each with its own `write_buf`. A text-mode write
        // lands in this handle's `write_buf`, but a `sys.stdout.buffer.write(...)`
        // lands in the *child* buffer handle's `write_buf`. Flushing only the
        // parent would silently drop the child's buffered bytes (e.g. a
        // sub-blocksize binary write that never fills a block, then process
        // exit), so we must cascade the flush into the `buffer_bits` child after
        // releasing the shared backend lock. The child has `buffer_bits == 0`,
        // so the cascade terminates after one hop.
        let buffer_bits = unsafe {
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
            let buffer_bits = handle.buffer_bits;
            let backend_state = Arc::clone(&handle.state);
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            // A `Text` backend (e.g. StringIO) holds its content directly and has
            // no raw write buffer to drain, so its own flush is a no-op. We still
            // fall through to cascade into any `buffer_bits` child below.
            if !matches!(backend, MoltFileBackend::Text(_)) {
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
                if let Err(bits) = backend_flush(_py, backend) {
                    return bits;
                }
            }
            buffer_bits
            // `guard` (the shared backend lock) is released here, before the
            // cascade below; the child handle locks this same mutex.
        };
        if buffer_bits != 0 && !obj_from_bits(buffer_bits).is_none() {
            let res = molt_file_flush(buffer_bits);
            if exception_pending(_py) {
                return res;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_close(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            // `close()` on an already-closed stream is a silent no-op in
            // CPython (no flush, no error). Short-circuit before the
            // flush-on-close below, which would otherwise raise
            // "I/O operation on closed file" on a second close.
            if file_handle_is_closed(handle) {
                return MoltObject::none().bits();
            }
        }
        // CPython closes a stream by flushing it first; for a `TextIOWrapper`
        // that flush cascades into the underlying buffered binary stream. Use
        // the buffer-aware `molt_file_flush` so a `sys.stdout.buffer.write(...)`
        // (or any binary write into a text wrapper's child buffer) is drained
        // before the shared backend is taken below; not just the parent
        // handle's own `write_buf`. This mirrors the exit-time flush path and
        // keeps `close()`/`flush()` symmetric for the binary buffer child.
        let res = molt_file_flush(handle_bits);
        if exception_pending(_py) {
            return res;
        }

        // VFS writeback
        // If this handle was opened for writing on a VFS mount, read
        // the final bytearray content and flush it to the VFS backend.
        unsafe {
            if let Some(handle_ptr) = file_handle_ptr(ptr).as_ref() {
                let handle = handle_ptr;
                if let Some((vfs_backend, vfs_path)) = vfs_writeback_take(_py, &handle.state) {
                    // Read the bytearray content that the runtime wrote into.
                    let mem = handle.mem_bits;
                    if mem != 0
                        && let Some(mem_ptr) = obj_from_bits(mem).as_ptr()
                        && object_type_id(mem_ptr) == TYPE_ID_BYTEARRAY
                    {
                        let vec_ptr = bytearray_vec_ptr(mem_ptr);
                        if !vec_ptr.is_null() {
                            let data = &*vec_ptr;
                            let _ = vfs_backend.open_write(&vfs_path, data);
                        }
                    }
                    // For text-mode handles, the buffer layer holds the
                    // bytearray, not the outer handle. Walk through the
                    // buffer handle's mem_bits instead.
                    if mem == 0
                        && handle.buffer_bits != 0
                        && let Some(buf_ptr) = obj_from_bits(handle.buffer_bits).as_ptr()
                        && object_type_id(buf_ptr) == TYPE_ID_FILE_HANDLE
                    {
                        let buf_handle = &*file_handle_ptr(buf_ptr);
                        let buf_mem = buf_handle.mem_bits;
                        if buf_mem != 0
                            && let Some(mem_ptr) = obj_from_bits(buf_mem).as_ptr()
                            && object_type_id(mem_ptr) == TYPE_ID_BYTEARRAY
                        {
                            let vec_ptr = bytearray_vec_ptr(mem_ptr);
                            if !vec_ptr.is_null() {
                                let data = &*vec_ptr;
                                let _ = vfs_backend.open_write(&vfs_path, data);
                            }
                        }
                    }
                }
            }
        }

        file_handle_close_ptr(ptr);
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::{file_remaining_bytes_hint, io_clear_runtime_state, molt_sys_stdout};
    use crate::{clear_exception, dec_ref_bits, obj_from_bits, runtime_state};
    use std::fs::{File, remove_file};
    use std::io::{Seek, SeekFrom, Write};
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    fn safe_temp_component(value: &str) -> String {
        value
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .collect()
    }

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let current_thread = std::thread::current();
        let thread_name = current_thread.name().unwrap_or("t");
        path.push(format!(
            "molt_io_{name}_{}_{}.bin",
            std::process::id(),
            safe_temp_component(thread_name)
        ));
        path
    }

    #[test]
    fn file_remaining_bytes_hint_tracks_stream_position() {
        let path = temp_path("reserve_hint");
        let mut writer = File::create(&path).expect("create temp file");
        writer.write_all(&[1u8; 16]).expect("write temp file");
        drop(writer);

        let mut file = File::open(&path).expect("open temp file");
        assert_eq!(file_remaining_bytes_hint(&mut file), Some(16));
        file.seek(SeekFrom::Start(5)).expect("seek temp file");
        assert_eq!(file_remaining_bytes_hint(&mut file), Some(11));

        let _ = remove_file(path);
    }

    #[test]
    fn cached_stdio_handles_are_runtime_owned_and_clearable() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            io_clear_runtime_state(_py, state);
            clear_exception(_py);

            let stdout_bits = molt_sys_stdout();
            assert!(!obj_from_bits(stdout_bits).is_none());
            assert_eq!(
                state.io.sys_stdout_handle_bits.load(Ordering::Acquire),
                stdout_bits
            );

            dec_ref_bits(_py, stdout_bits);
            io_clear_runtime_state(_py, state);
            assert_eq!(state.io.sys_stdout_handle_bits.load(Ordering::Acquire), 0);
        });
    }
}
