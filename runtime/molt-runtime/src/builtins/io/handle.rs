use super::*;

pub(super) type VfsWritebackEntry = (Arc<dyn crate::vfs::VfsBackend>, String);

pub(super) fn vfs_writeback_register(
    _py: &PyToken<'_>,
    state: &Arc<MoltFileState>,
    entry: VfsWritebackEntry,
) {
    let key = Arc::as_ptr(state) as usize;
    runtime_state(_py)
        .io
        .vfs_writebacks
        .lock()
        .unwrap()
        .insert(key, entry);
}

pub(super) fn vfs_writeback_take(
    _py: &PyToken<'_>,
    state: &Arc<MoltFileState>,
) -> Option<VfsWritebackEntry> {
    let key = Arc::as_ptr(state) as usize;
    runtime_state(_py)
        .io
        .vfs_writebacks
        .lock()
        .unwrap()
        .remove(&key)
}

pub(super) fn resolve_file_handle_ptr(
    _py: &PyToken<'_>,
    obj_bits: u64,
) -> Result<*mut MoltFileHandle, u64> {
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
pub(super) fn alloc_file_handle_with_state(
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

pub(crate) unsafe fn file_handle_enter(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    unsafe {
        let bits = MoltObject::from_ptr(ptr).bits();
        let handle_ptr = file_handle_ptr(ptr);
        if !handle_ptr.is_null() {
            let handle = &mut *handle_ptr;
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
                if file_handle_require_attached(_py, handle).is_err() {
                    return;
                }
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

pub(crate) fn file_handle_detached_message(handle: &MoltFileHandle) -> &'static str {
    if handle.text {
        "underlying buffer has been detached"
    } else {
        "raw stream has been detached"
    }
}

pub(crate) fn file_handle_require_attached(
    _py: &PyToken<'_>,
    handle: &MoltFileHandle,
) -> Result<(), u64> {
    if handle.detached {
        Err(raise_exception::<u64>(
            _py,
            "ValueError",
            file_handle_detached_message(handle),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn file_handle_is_closed(handle: &MoltFileHandle) -> bool {
    if handle.closed {
        return true;
    }
    Arc::clone(&handle.state).backend.lock().unwrap().is_none()
}
