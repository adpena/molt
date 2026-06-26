use super::*;

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
pub extern "C" fn molt_buffered_new(cls_bits: u64, raw_bits: u64, buffer_size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let raw_handle_ptr = match resolve_file_handle_ptr(_py, raw_bits) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        unsafe {
            let raw_handle = &mut *raw_handle_ptr;
            if raw_handle.detached {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    file_handle_detached_message(raw_handle),
                );
            }
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
            if buffer_handle.detached {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    file_handle_detached_message(buffer_handle),
                );
            }
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
