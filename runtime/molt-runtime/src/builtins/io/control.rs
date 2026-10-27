use super::*;
use num_traits::ToPrimitive;

pub(crate) extern "C" fn io_base_check_closed(object: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(name) = attr_name_bits_from_bytes(py, b"closed") else {
            return MoltObject::none().bits();
        };
        let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
        let value = match crate::builtins::platform::getattr_optional_bits(py, object, name) {
            Ok(value) => value,
            Err(error) => return error,
        };
        if let Some(value) = value {
            let closed = is_truthy(py, obj_from_bits(value));
            dec_ref_bits(py, value);
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            if closed {
                return raise_exception::<_>(py, "ValueError", "I/O operation on closed file.");
            }
        }
        MoltObject::none().bits()
    })
}

pub(crate) extern "C" fn io_base_exit(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) =
            crate::builtins::native_arguments::NativeArguments::read(py, "__exit__", args, kwargs)
        else {
            return MoltObject::none().bits();
        };
        if !call.keyword_view().is_empty() {
            return raise_exception::<_>(py, "TypeError", "__exit__() takes no keyword arguments");
        }
        let Some(&object) = call.positional.first() else {
            return raise_exception::<_>(py, "TypeError", "__exit__() needs an argument");
        };
        molt_file_exit(object, MoltObject::none().bits())
    })
}

pub(crate) extern "C" fn io_base_finalize(object: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        // The generic finalizer owns once-only execution and resurrection.
        // This also preserves incoming errors on an explicit __del__ call.
        crate::builtins::exceptions::run_unraisable(py, object, None, || {
            let _exception_scope = crate::builtins::exceptions::ExceptionStackScope::push(py);
            let Some(name) = attr_name_bits_from_bytes(py, b"closed") else {
                clear_exception(py);
                return;
            };
            let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
            let value = crate::builtins::platform::getattr_optional_bits(py, object, name);
            let closed = match value {
                Ok(Some(value)) => {
                    let closed = is_truthy(py, obj_from_bits(value));
                    dec_ref_bits(py, value);
                    closed
                }
                Ok(None) | Err(_) => {
                    clear_exception(py);
                    return;
                }
            };
            if exception_pending(py) {
                clear_exception(py);
                return;
            }
            if closed {
                return;
            }
            if let Some(name) = attr_name_bits_from_bytes(py, b"_finalizing") {
                let _name = PtrDropGuard::new(obj_from_bits(name).as_ptr().unwrap());
                molt_setattr_builtin(object, name, MoltObject::from_bool(true).bits());
            }
            clear_exception(py);
            let result = io_call_noargs(py, object, b"close");
            dec_ref_bits(py, result);
            if exception_pending(py)
                && crate::object::ops_sys::env_flag_bool("PYTHONDEVMODE").unwrap_or(0) == 0
            {
                clear_exception(py);
            }
        });
        MoltObject::none().bits()
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
                let classes = builtin_classes(_py);
                let class = type_of_bits(_py, handle_bits);
                if issubclass_bits(class, classes.buffered_io_base)
                    || issubclass_bits(class, classes.text_io_base)
                {
                    return raise_exception::<_>(_py, "UnsupportedOperation", "detach");
                }
                return raise_exception::<_>(_py, "TypeError", "expected file handle");
            }
            let handle_ptr = file_handle_ptr(ptr);
            if handle_ptr.is_null() {
                return raise_exception::<_>(_py, "RuntimeError", "file handle missing");
            }
            if let Err(error) = buffered::prepare(_py, ptr, &[b"write"], true) {
                return error;
            }
            if buffered::is_reader(_py, ptr) || buffered::applies(ptr) {
                return buffered::detach(_py, handle_bits, ptr);
            }
            let handle = &mut *handle_ptr;
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let builtins = builtin_classes(_py);
            let class_bits = object_class_bits(ptr);
            if [builtins.bytes_io, builtins.string_io]
                .iter()
                .any(|&base| crate::object::class_layout::is_real_subtype(_py, class_bits, base))
            {
                return raise_exception::<_>(
                    _py,
                    "UnsupportedOperation",
                    "detach is unsupported for in-memory streams",
                );
            }
            let backend_state = Arc::clone(match file_handle_physical_state(_py, handle) {
                Ok(state) => state,
                Err(error) => return error,
            });
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
                handle.attachment = crate::object::FileAttachment::Detached;
                handle.owns_fd = false;
                return buffer_bits;
            }
            // Buffered read-ahead belongs to this wrapper. CPython detach drops
            // those unread bytes rather than rewinding/copying them into raw.
            drop(guard);
            let _ = handle;
            let Some(raw) = file_handle_raw_bits(_py, ptr) else {
                return MoltObject::none().bits();
            };
            let handle = &mut *handle_ptr;
            let stored = std::mem::replace(&mut handle.buffer_bits, MoltObject::none().bits());
            handle.attachment = crate::object::FileAttachment::Detached;
            handle.owns_fd = false;
            handle.read_buf.clear();
            handle.read_pos = 0;
            handle.pending_byte = None;
            dec_ref_bits(_py, stored);
            raw
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
            let backend_state = Arc::clone(match file_handle_physical_state(_py, handle) {
                Ok(state) => state,
                Err(error) => return error,
            });
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
                let (label, display) = match selected_text_encoding(_py, Some(&encoding)) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "LookupError", &msg),
                };
                new_encoding = Some(label);
                new_encoding_original = Some(display);
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
                handle.text_bom_written = super::handle::initial_text_bom_written(
                    match file_handle_physical_state(_py, handle) {
                        Ok(state) => state,
                        Err(error) => return error,
                    },
                    handle.buffer_bits,
                    handle.writable,
                    handle.encoding.as_deref(),
                );
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

// FileIO append and text positioning use the platform's actual offset error
// domain. A pipe is unsupported text positioning; EBADF/EIO remain OSError.
fn require_seekable_text_backend(
    py: &PyToken<'_>,
    state: &MoltFileState,
    backend: &mut MoltFileBackend,
) -> Result<(), u64> {
    if let MoltFileBackend::File(file) = backend {
        let error = host_file_seek(state, file, std::io::SeekFrom::Current(0)).err();
        if let Some(error) = error {
            #[cfg(windows)]
            let nonseekable = error.raw_os_error() == Some(libc::ESPIPE);
            #[cfg(not(windows))]
            let nonseekable = error.kind() == ErrorKind::NotSeekable;
            if nonseekable {
                return Err(raise_exception::<_>(
                    py,
                    "UnsupportedOperation",
                    "underlying stream is not seekable",
                ));
            }
            #[cfg(windows)]
            return Err(match error.raw_os_error() {
                Some(errno) => raise_os_error_errno::<u64>(py, i64::from(errno), "tell"),
                None => raise_os_error::<u64>(py, error, "tell"),
            });
            #[cfg(not(windows))]
            return Err(raise_os_error::<u64>(py, error, "tell"));
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_seek(handle_bits: u64, offset_bits: u64, whence_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        file_seek(_py, handle_bits, offset_bits, whence_bits)
    })
}

fn file_seek(_py: &PyToken<'_>, handle_bits: u64, offset_bits: u64, whence_bits: u64) -> u64 {
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
        // seek's Clinic whence conversion precedes readiness, including an
        // uninitialized writer whose __index__ callback initializes it.
        if buffered::is_reader(_py, ptr) {
            return buffered_reader::seek(_py, handle_bits, ptr, offset_bits, whence_bits);
        }
        if buffered::is_writer(_py, ptr)
            && (*handle_ptr).attachment != crate::object::FileAttachment::Attached
        {
            return buffered_writer::seek(_py, handle_bits, ptr, offset_bits, whence_bits);
        }
        if (crate::builtins::numbers::index_i64_integral_bits(offset_bits).is_none()
            || crate::builtins::numbers::index_i64_integral_bits(whence_bits).is_none())
            && let Err(error) = buffered::prepare_conversion(_py, ptr)
        {
            return error;
        }
        if let Err(error) = buffered::prepare(_py, ptr, &[b"seekable", b"write", b"seek"], false) {
            return error;
        }
        if buffered::applies(ptr) {
            return buffered_writer::seek(_py, handle_bits, ptr, offset_bits, whence_bits);
        }
        let handle = &mut *handle_ptr;
        if let Err(bits) = file_handle_require_attached(_py, handle) {
            return bits;
        }
        if file_handle_is_closed(handle) {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        }
        // Index callbacks may close/detach the stream; no backing borrow or
        // mutex is retained across argument conversion.
        let Some(whence) = crate::builtins::numbers::index_c_int_from_obj(_py, whence_bits) else {
            return MoltObject::none().bits();
        };
        let whence = i64::from(whence);
        let type_name = class_name_for_error(type_of_bits(_py, offset_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, offset_bits, &msg) else {
            return MoltObject::none().bits();
        };
        let handle = &mut *handle_ptr;
        if let Err(bits) = file_handle_require_attached(_py, handle) {
            return bits;
        }
        if file_handle_is_closed(handle) {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        }
        let flush_text = if handle.text {
            let state = Arc::clone(match file_handle_physical_state(_py, handle) {
                Ok(state) => state,
                Err(error) => return error,
            });
            let mut guard = state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            if matches!(backend, MoltFileBackend::Text(_)) {
                false
            } else {
                if let Err(bits) = require_seekable_text_backend(_py, &state, backend) {
                    return bits;
                }
                match whence {
                    0 if value.sign() == num_bigint::Sign::Minus => {
                        return raise_exception::<_>(_py, "ValueError", "negative seek position");
                    }
                    1 | 2 if value != num_bigint::BigInt::from(0) => {
                        return raise_exception::<_>(
                            _py,
                            "UnsupportedOperation",
                            if whence == 1 {
                                "can't do nonzero cur-relative seeks"
                            } else {
                                "can't do nonzero end-relative seeks"
                            },
                        );
                    }
                    0..=2 => {}
                    _ => return raise_exception::<_>(_py, "ValueError", "invalid whence"),
                }
                true
            }
        } else {
            false
        };
        if handle.text && whence == 1 && value == num_bigint::BigInt::from(0) {
            let cookie = if flush_text {
                io_call_noargs(_py, handle_bits, b"tell")
            } else {
                file_tell(_py, handle_bits)
            };
            if exception_pending(_py) {
                dec_ref_bits(_py, cookie);
                return MoltObject::none().bits();
            }
            let result = file_seek(_py, handle_bits, cookie, MoltObject::from_int(0).bits());
            dec_ref_bits(_py, cookie);
            return result;
        }
        if flush_text {
            // SEEK_SET/END honor the same overridable flush protocol as tell.
            // The temporary admission borrow/lock has ended before the call.
            let result = io_call_noargs(_py, handle_bits, b"flush");
            dec_ref_bits(_py, result);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        let handle = &mut *handle_ptr;
        if let Err(bits) = file_handle_require_attached(_py, handle) {
            return bits;
        }
        if file_handle_is_closed(handle) {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        }
        let backend_state = Arc::clone(match file_handle_physical_state(_py, handle) {
            Ok(state) => state,
            Err(error) => return error,
        });
        let mut guard = backend_state.backend.lock().unwrap();
        let Some(backend) = guard.as_mut() else {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        };
        // BytesIO/StringIO admit target Py_ssize_t before any flush/position
        // mutation. Text wrappers over memory still accept their opaque cookie.
        let memory_offset = if matches!(backend, MoltFileBackend::Text(_))
            || (!handle.text && matches!(backend, MoltFileBackend::Memory(_)))
        {
            match value.to_isize() {
                Some(offset) => Some(offset as i64),
                None => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "Python int too large to convert to C ssize_t",
                    );
                }
            }
        } else {
            None
        };
        if !handle.write_buf.is_empty()
            && let Err(bits) = flush_write_buffer(_py, handle, backend)
        {
            return bits;
        }
        if handle.text {
            if let MoltFileBackend::Text(_) = backend {
                let offset = memory_offset.expect("StringIO offset was admitted as Py_ssize_t");
                let pos = match text_backend_seek(_py, backend, offset, whence) {
                    Ok(pos) => pos,
                    Err(bits) => return bits,
                };
                handle.pending_byte = None;
                handle.text_pending_bytes.clear();
                handle.text_pending_text.clear();
                clear_read_buffer(handle);
                clear_write_buffer(handle);
                return int_bits_from_i128(_py, i128::from(pos));
            }
            if whence == 0 {
                let cookie = match text_cookie_decode_value(&value) {
                    Ok(val) => val,
                    Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
                };
                match backend_seek(_py, handle, backend, std::io::SeekFrom::Start(cookie.pos)) {
                    Ok(_) => {}
                    Err(bits) => return bits,
                };
                text_cookie_restore_decoder(handle, &cookie);
                handle.pending_byte = cookie.pending_byte;
                handle.text_pending_bytes = cookie.pending_bytes;
                handle.text_pending_text = cookie.pending_text;
                clear_read_buffer(handle);
                clear_write_buffer(handle);
                return int_bits_from_bigint(_py, value);
            }
        }
        let Some(offset) = memory_offset.or_else(|| value.to_i64()) else {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "Python int too large to convert to C long",
            );
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
            reset_text_decoder(handle);
            handle.text_bom_written = pos != 0;
        }
        int_bits_from_i128(_py, i128::from(pos))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_tell(handle_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { file_tell(_py, handle_bits) })
}

fn file_tell(_py: &PyToken<'_>, handle_bits: u64) -> u64 {
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
        if let Err(error) = buffered::prepare(_py, ptr, &[b"tell"], false) {
            return error;
        }
        if buffered::is_reader(_py, ptr) {
            return buffered_reader::tell(_py, handle_bits, ptr);
        }
        if buffered::applies(ptr) {
            return buffered_writer::tell(_py, handle_bits, ptr);
        }
        let flush_text = {
            let handle = &mut *handle_ptr;
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let state = Arc::clone(match file_handle_physical_state(_py, handle) {
                Ok(state) => state,
                Err(error) => return error,
            });
            let mut guard = state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let flush_text = handle.text && !matches!(backend, MoltFileBackend::Text(_));
            if flush_text {
                // Refuse nonseekable text streams before flushing, as CPython
                // does. Binary buffered tell retains its nonflushing behavior.
                if let Err(bits) = require_seekable_text_backend(_py, &state, backend) {
                    return bits;
                }
                if let Err(bits) = flush_write_buffer(_py, handle, backend) {
                    return bits;
                }
            }
            flush_text
        };
        if flush_text {
            // No backing borrow or lock may span the overridable flush call.
            let result = io_call_noargs(_py, handle_bits, b"flush");
            dec_ref_bits(_py, result);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        // A flush callback may close/detach the stream: revalidate its owner.
        let handle = &mut *handle_ptr;
        if let Err(bits) = file_handle_require_attached(_py, handle) {
            return bits;
        }
        if file_handle_is_closed(handle) {
            return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
        }
        let backend_state = Arc::clone(match file_handle_physical_state(_py, handle) {
            Ok(state) => state,
            Err(error) => return error,
        });
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
            return int_bits_from_i128(_py, i128::from(pos));
        }
        let pos = match backend_tell(_py, &backend_state, backend) {
            Ok(pos) => pos,
            Err(bits) => return bits,
        };
        let unread = unread_bytes(handle) as i128;
        let buffered_write = (handle.write_buf.len() - handle.write_start) as i128;
        let logical = i128::from(pos) - unread + buffered_write;
        if handle.text {
            if !(0..=i128::from(u64::MAX)).contains(&logical) {
                return raise_exception::<_>(_py, "OSError", "tell failed");
            }
            match text_cookie_encode_bits(
                _py,
                logical as u64,
                handle.pending_byte,
                &handle.text_pending_bytes,
                &handle.text_pending_text,
                text_cookie_decoder(handle),
                handle.text_bom_seen,
            ) {
                Ok(bits) => bits,
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        } else {
            int_bits_from_i128(_py, logical)
        }
    }
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
            if let Err(error) = buffered::prepare(_py, ptr, &[b"fileno"], false) {
                return error;
            }
            if buffered::is_writer(_py, ptr)
                || (buffered::is_reader(_py, ptr) && buffered::applies(ptr))
            {
                return buffered::query(_py, handle_bits, ptr, b"fileno");
            }
            let handle = &mut *handle_ptr;
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(match file_handle_physical_state(_py, handle) {
                Ok(state) => state,
                Err(error) => return error,
            });
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
            if buffered::is_reader(_py, ptr) {
                return raise_exception(_py, "UnsupportedOperation", "truncate");
            }
            if !obj_from_bits(size_bits).is_none()
                && crate::builtins::numbers::index_i64_integral_bits(size_bits).is_none()
                && let Err(error) = buffered::prepare_conversion(_py, ptr)
            {
                return error;
            }
            if let Err(error) =
                buffered::prepare(_py, ptr, &[b"write", b"truncate", b"tell"], false)
            {
                return error;
            }
            if buffered::applies(ptr) {
                return buffered_writer::truncate(_py, handle_bits, ptr, size_bits);
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
            let backend_state = Arc::clone(match file_handle_physical_state(_py, handle) {
                Ok(state) => state,
                Err(error) => return error,
            });
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
                let pos = match backend_tell(_py, &backend_state, backend) {
                    Ok(pos) => pos as i64,
                    Err(bits) => return bits,
                };
                let unread = unread_bytes(handle) as i64;
                let buffered_write = (handle.write_buf.len() - handle.write_start) as i64;
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
            if buffered::is_reader(_py, ptr) {
                return buffered::query(_py, handle_bits, ptr, b"readable");
            }
            if buffered::is_writer(_py, ptr) {
                // BufferedWriter inherits IOBase.readable; it does not expose
                // _Buffered.readable's forwarding descriptor.
                return MoltObject::from_bool(false).bits();
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
            if buffered::is_reader(_py, ptr) {
                return MoltObject::from_bool(false).bits();
            }
            if let Err(error) = buffered::prepare(_py, ptr, &[b"writable"], false) {
                return error;
            }
            if buffered::is_writer(_py, ptr) {
                return buffered::query(_py, handle_bits, ptr, b"writable");
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
            if let Err(error) = buffered::prepare(_py, ptr, &[b"seekable"], false) {
                return error;
            }
            if buffered::is_writer(_py, ptr)
                || (buffered::is_reader(_py, ptr) && buffered::applies(ptr))
            {
                return buffered::query(_py, handle_bits, ptr, b"seekable");
            }
            let handle = &mut *handle_ptr;
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(match file_handle_physical_state(_py, handle) {
                Ok(state) => state,
                Err(error) => return error,
            });
            let mut guard = backend_state.backend.lock().unwrap();
            let Some(backend) = guard.as_mut() else {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            };
            let seekable = match backend {
                MoltFileBackend::File(file) => {
                    host_file_seek(&backend_state, file, std::io::SeekFrom::Current(0)).is_ok()
                }

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
            if let Err(error) = buffered::prepare(_py, ptr, &[b"isatty"], false) {
                return error;
            }
            if buffered::is_writer(_py, ptr)
                || (buffered::is_reader(_py, ptr) && buffered::applies(ptr))
            {
                return buffered::query(_py, handle_bits, ptr, b"isatty");
            }
            let handle = &mut *handle_ptr;
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
            if file_handle_is_closed(handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            let backend_state = Arc::clone(match file_handle_physical_state(_py, handle) {
                Ok(state) => state,
                Err(error) => return error,
            });
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
        let reader = obj_from_bits(handle_bits)
            .as_ptr()
            .is_some_and(|pointer| unsafe {
                object_type_id(pointer) == TYPE_ID_FILE_HANDLE && buffered::is_reader(_py, pointer)
            });
        let virtual_reader = reader
            && unsafe {
                object_class_bits(obj_from_bits(handle_bits).as_ptr().unwrap())
                    != builtin_classes(_py).buffered_reader
            };
        let line_bits = if virtual_reader {
            io_call_noargs(_py, handle_bits, b"readline")
        } else {
            molt_file_readline(handle_bits, MoltObject::from_int(-1).bits())
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let line_obj = obj_from_bits(line_bits);
        if reader
            && !line_obj
                .as_ptr()
                .is_some_and(|pointer| unsafe { object_type_id(pointer) == TYPE_ID_BYTES })
        {
            let message = format!(
                "readline() should have returned a bytes object, not '{}'",
                type_name(_py, line_obj)
            );
            dec_ref_bits(_py, line_bits);
            return raise_exception(_py, "OSError", &message);
        }
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
pub extern "C" fn molt_file_enter(object: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        io_base_check_closed(object);
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        inc_ref_bits(py, object);
        object
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_file_exit(handle_bits: u64, _exc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { io_call_noargs(_py, handle_bits, b"close") })
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
