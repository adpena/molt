use super::*;

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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
