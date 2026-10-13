use super::*;

pub(crate) extern "C" fn io_base_flush(object: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        match io_base_is_closed(py, object) {
            Ok(false) => MoltObject::none().bits(),
            Ok(true) => raise_exception::<_>(py, "ValueError", "I/O operation on closed file."),
            Err(error) => error,
        }
    })
}

pub(crate) extern "C" fn io_base_close(object: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        match io_base_is_closed(py, object) {
            Ok(true) => return MoltObject::none().bits(),
            Err(error) => return error,
            Ok(false) => {}
        }
        let _exception_scope = crate::builtins::exceptions::ExceptionStackScope::push(py);
        let result = io_call_noargs(py, object, b"flush");
        let error = crate::builtins::exceptions::take_raised(py);
        // Publish closed even after a failed flush. A user setter may itself
        // fail, in which case that error takes precedence and retains context.
        if let Some(name) = attr_name_bits_from_bytes(py, b"__IOBase_closed") {
            let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
            molt_setattr_builtin(object, name, MoltObject::from_bool(true).bits());
        }
        io_chain_close_error(py, error);
        dec_ref_bits(py, result);
        MoltObject::none().bits()
    })
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
                    Err(error) => {
                        if !matches!(
                            error,
                            crate::object::ops::EncodeError::InvalidSequence { .. }
                        ) || translated.as_slice() == raw
                        {
                            return error.raise(_py, data_obj.bits());
                        }
                        // Allocate the translated Python object only when the
                        // exception needs it. Successful writes keep the same
                        // byte buffer and do not create another Python string.
                        let object = alloc_string(_py, &translated);
                        if object.is_null() {
                            return MoltObject::none().bits();
                        }
                        let bits = MoltObject::from_ptr(object).bits();
                        let result = error.raise(_py, bits);
                        dec_ref_bits(_py, bits);
                        return result;
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
                if matches!(backend, MoltFileBackend::Memory(_)) {
                    let ptr = super::buffer::memory_backend_ptr_from_bits(
                        _py,
                        memory_backend_bits(handle),
                    );
                    match ptr.and_then(|ptr| {
                        crate::object::buffer_exports::bytearray_require_unexported(_py, ptr)
                    }) {
                        Ok(()) => {}
                        Err(bits) => return bits,
                    }
                }
                let mut written = 0usize;
                while written < bytes.len() {
                    let n = match backend_write_bytes(
                        _py,
                        memory_backend_bits(handle),
                        backend,
                        &bytes[written..],
                    ) {
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
                let Some((line_bits, done_bits)) = crate::object::seq_access::tuple_pair(pair_ptr)
                else {
                    return MoltObject::none().bits();
                };
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
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
        // releasing the shared backend lock. A buffered child may retain its
        // raw FileIO owner in the same field; that raw owner ends the cascade.
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
            let mem_bits = memory_backend_bits(handle);
            if mem_bits != 0 && !obj_from_bits(mem_bits).is_none() {
                let mem_ptr = match super::buffer::memory_backend_ptr_from_bits(_py, mem_bits) {
                    Ok(ptr) => ptr,
                    Err(bits) => return bits,
                };
                if let Err(bits) =
                    crate::object::buffer_exports::bytearray_require_unexported(_py, mem_ptr)
                {
                    return bits;
                }
            }
        }
        // Buffered/Text close observes the retained raw/buffer's public
        // closed attribute, not an override of this wrapper's own property.
        // The shared member authority also preserves its exact getter error.
        let closed = unsafe {
            super::attributes::io_member_value(
                _py,
                ptr,
                object_class_bits(ptr),
                super::attributes::IoOperation::Closed,
            )
        };
        let Some(closed) = closed else {
            return MoltObject::none().bits();
        };
        let is_closed = is_truthy(_py, obj_from_bits(closed));
        dec_ref_bits(_py, closed);
        if exception_pending(_py) || is_closed {
            return MoltObject::none().bits();
        }
        let classes = builtin_classes(_py);
        let class = unsafe { object_class_bits(ptr) };
        let raw =
            unsafe { crate::object::class_layout::is_real_subtype(_py, class, classes.file_io) };
        let memory = [classes.bytes_io, classes.string_io]
            .iter()
            .any(|&base| unsafe { crate::object::class_layout::is_real_subtype(_py, class, base) });
        // FileIO closes the base protocol and then retires its descriptor even
        // if flush failed. Buffered/text streams call their virtual flush;
        // BytesIO/StringIO close their storage directly without calling flush.
        let _exception_scope = crate::builtins::exceptions::ExceptionStackScope::push(_py);
        let result = if raw {
            io_base_close(handle_bits)
        } else if memory {
            MoltObject::none().bits()
        } else {
            io_call_noargs(_py, handle_bits, b"flush")
        };
        let flush_error = crate::builtins::exceptions::take_raised(_py);
        dec_ref_bits(_py, result);

        let result = (|| {
            // A flush override may detach or replace the child. Read and retain
            // the current owner after that callback, without holding backing
            // references across its dynamic close.
            let child = unsafe {
                let handle = &*file_handle_ptr(ptr);
                if let Err(error) = file_handle_require_attached(_py, handle) {
                    return error;
                }
                let child = handle.buffer_bits;
                if child != 0 && !obj_from_bits(child).is_none() {
                    inc_ref_bits(_py, child);
                    Some(child)
                } else {
                    None
                }
            };
            if let Some(child) = child {
                let _child = obj_from_bits(child).as_ptr().map(PtrDropGuard::new);
                let result = io_call_noargs(_py, child, b"close");
                unsafe {
                    let handle = &mut *file_handle_ptr(ptr);
                    clear_read_buffer(handle);
                    clear_write_buffer(handle);
                }
                return result;
            }

            // Detach one writeback owner before publication. The leaf of a
            // native buffer/raw chain owns both publication and physical close.
            let writeback_error = unsafe {
                let handle = &*file_handle_ptr(ptr);
                if let Some((backend, path)) = vfs_writeback_take(_py, &handle.state) {
                    let data = match super::buffer::memory_backend_vec_ref_from_bits(
                        _py,
                        memory_backend_bits(handle),
                    ) {
                        Ok(data) => data,
                        Err(error) => {
                            file_handle_close_ptr(ptr);
                            return error;
                        }
                    };
                    backend
                        .open_write(&path, data)
                        .err()
                        .map(|error| (error, path))
                } else {
                    None
                }
            };
            // Publication failure must not leave the resource open or retry
            // publication on a second close.
            file_handle_close_ptr(ptr);
            if let Some((error, path)) = writeback_error {
                return raise_vfs_error(_py, error, &path);
            }
            MoltObject::none().bits()
        })();
        io_chain_close_error(_py, flush_error);
        if exception_pending(_py) {
            dec_ref_bits(_py, result);
            MoltObject::none().bits()
        } else {
            result
        }
    })
}
