use super::*;

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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
