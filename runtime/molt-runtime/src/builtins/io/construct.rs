use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_io_default_buffer_size() -> u64 {
    crate::with_gil_entry_nopanic!(py, { MoltObject::from_int(default_buffer_size(py)).bits() })
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
            "_IOBase" => builtins.io_base,
            "_RawIOBase" => builtins.raw_io_base,
            "_BufferedIOBase" => builtins.buffered_io_base,
            "_TextIOBase" => builtins.text_io_base,
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

pub(crate) extern "C" fn buffered_writer_new_method(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) =
            crate::builtins::native_arguments::NativeArguments::read(py, "__new__", args, kwargs)
        else {
            return MoltObject::none().bits();
        };
        let Some((class, _)) = crate::builtins::type_ops::native_constructor_receiver(
            py,
            builtin_classes(py).buffered_writer,
            call.positional.first().copied(),
            "BufferedWriter",
        ) else {
            return MoltObject::none().bits();
        };
        buffered_writer::new(py, class)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffered_new(cls_bits: u64, raw_bits: u64, buffer_size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let builtins = builtin_classes(_py);
        let Some((_, class_ptr)) = crate::builtins::type_ops::native_constructor_receiver(
            _py,
            builtins.buffered_io_base,
            Some(cls_bits),
            "_BufferedIOBase",
        ) else {
            return MoltObject::none().bits();
        };
        // The first concrete buffered owner in the real MRO determines the
        // capability contract; subclasses retain their requested result class.
        let capabilities = unsafe { crate::class_mro_view(_py, class_ptr) }
            .iter()
            .find_map(|&base| {
                if base == builtins.buffered_reader {
                    Some((true, false))
                } else if base == builtins.buffered_writer {
                    Some((false, true))
                } else if base == builtins.buffered_random {
                    Some((true, true))
                } else {
                    None
                }
            });
        let Some((want_read, want_write)) = capabilities else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "expected a concrete buffered stream type",
            );
        };
        if want_write && !want_read {
            return buffered_writer::new(_py, cls_bits);
        }
        let raw_handle_ptr = match resolve_file_handle_ptr(_py, raw_bits) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        unsafe {
            let raw_handle = &mut *raw_handle_ptr;
            if let Err(bits) = file_handle_require_attached(_py, raw_handle) {
                return bits;
            }
            if file_handle_is_closed(raw_handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if raw_handle.text {
                return raise_exception::<_>(_py, "ValueError", "raw stream must be binary");
            }
            let size_obj = obj_from_bits(buffer_size_bits);
            let mut buffer_size = if size_obj.is_none() {
                default_buffer_size(_py)
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
                buffer_size = default_buffer_size(_py);
            }
            if buffer_size == 0 {
                return raise_exception::<_>(_py, "ValueError", "buffer size must be > 0");
            }
            if want_read && !raw_handle.readable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not readable");
            }
            if want_write && !raw_handle.writable {
                return raise_exception::<_>(_py, "UnsupportedOperation", "not writable");
            }
            let owns_fd = raw_handle.owns_fd;
            let ptr = alloc_file_handle_with_state(
                _py,
                Some(Arc::clone(
                    match file_handle_physical_state(_py, raw_handle) {
                        Ok(state) => state,
                        Err(error) => return error,
                    },
                )),
                want_read,
                want_write,
                false,
                raw_handle.closefd,
                false,
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
                raw_bits,
                raw_handle.mem_bits,
            );
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                (*file_handle_ptr(ptr)).owns_fd = owns_fd;
                MoltObject::from_ptr(ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_buffered_init(self_bits: u64, raw_bits: u64, buffer_size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(pointer) = obj_from_bits(self_bits).as_ptr() else {
            return raise_exception(py, "TypeError", "expected buffered stream");
        };
        unsafe {
            if object_type_id(pointer) != TYPE_ID_FILE_HANDLE || file_handle_ptr(pointer).is_null()
            {
                return raise_exception(py, "TypeError", "expected buffered stream");
            }
            let classes = builtin_classes(py);
            let class = obj_from_bits(object_class_bits(pointer)).as_ptr().unwrap();
            let owner = crate::class_mro_view(py, class)
                .iter()
                .copied()
                .find(|base| {
                    [
                        classes.buffered_reader,
                        classes.buffered_writer,
                        classes.buffered_random,
                    ]
                    .contains(base)
                });
            if owner == Some(classes.buffered_writer) {
                return buffered_writer::initialize(
                    py,
                    self_bits,
                    pointer,
                    raw_bits,
                    buffer_size_bits,
                );
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_text_io_wrapper_new(
    cls_bits: u64,
    buffer_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    newline_bits: u64,
    line_buffering_bits: u64,
    write_through_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if crate::builtins::type_ops::native_constructor_receiver(
            _py,
            builtin_classes(_py).text_io_wrapper,
            Some(cls_bits),
            "TextIOWrapper",
        )
        .is_none()
        {
            return MoltObject::none().bits();
        }
        // Warning callbacks may close the supplied buffer. Complete them
        // before resolving/borrowing its native handle.
        if obj_from_bits(encoding_bits).is_none() && !warn_default_text_encoding(_py, None) {
            return MoltObject::none().bits();
        }
        let buffer_handle_ptr = match resolve_file_handle_ptr(_py, buffer_bits) {
            Ok(ptr) => ptr,
            Err(bits) => return bits,
        };
        unsafe {
            let buffer_handle = &mut *buffer_handle_ptr;
            if let Err(bits) = file_handle_require_attached(_py, buffer_handle) {
                return bits;
            }
            if file_handle_is_closed(buffer_handle) {
                return raise_exception::<_>(_py, "ValueError", "I/O operation on closed file");
            }
            if buffer_handle.text {
                return raise_exception::<_>(_py, "ValueError", "buffer must be binary");
            }
            let encoding = if obj_from_bits(encoding_bits).is_none() {
                None
            } else {
                let encoding = reconfigure_arg_type(_py, encoding_bits, "encoding");
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                encoding
            };
            let (encoding, encoding_original) =
                match selected_text_encoding(_py, encoding.as_deref()) {
                    Ok((label, display)) => (Some(label), Some(display)),
                    Err(message) => return raise_exception::<u64>(_py, "LookupError", &message),
                };
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
            let owns_fd = buffer_handle.owns_fd;
            let ptr = alloc_file_handle_with_state(
                _py,
                Some(Arc::clone(
                    match file_handle_physical_state(_py, buffer_handle) {
                        Ok(state) => state,
                        Err(error) => return error,
                    },
                )),
                buffer_handle.readable,
                buffer_handle.writable,
                true,
                buffer_handle.closefd,
                false,
                line_buffering,
                write_through,
                buffer_handle.buffer_size,
                cls_bits,
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
                (*file_handle_ptr(ptr)).owns_fd = owns_fd;
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
pub extern "C" fn molt_bytesio_new(cls_bits: u64, initial_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if crate::builtins::type_ops::native_constructor_receiver(
            _py,
            builtin_classes(_py).bytes_io,
            Some(cls_bits),
            "BytesIO",
        )
        .is_none()
        {
            return MoltObject::none().bits();
        }
        // BytesIO requests the contiguous read-only role, including dimensions.
        // Keep the original exporter live until the owned backing copy exists.
        let input = if obj_from_bits(initial_bits).is_none() {
            None
        } else {
            match crate::object::buffer_exports::ScopedBuffer::with_flags(
                _py,
                initial_bits,
                molt_cpython_abi::abi_types::PyBUF_CONTIG_RO,
            ) {
                Ok(input) => Some(input),
                Err(error) => {
                    return error.raise(_py, "a bytes-like object is required");
                }
            }
        };
        let payload = match input.as_ref() {
            None => &[][..],
            Some(input) => {
                let len = match input.contiguous_len() {
                    Ok(len) => len,
                    Err(error) => return error.raise(_py, "a bytes-like object is required"),
                };
                unsafe {
                    std::slice::from_raw_parts(
                        if len == 0 {
                            std::ptr::NonNull::<u8>::dangling().as_ptr()
                        } else {
                            input.view().data
                        },
                        len,
                    )
                }
            }
        };
        let bytearray_ptr = alloc_bytearray(_py, payload);
        drop(input);
        if bytearray_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mem_bits = MoltObject::from_ptr(bytearray_ptr).bits();
        let state = Arc::new(MoltFileState {
            backend: Mutex::new(Some(MoltFileBackend::Memory(MoltMemoryBackend { pos: 0 }))),
            #[cfg(windows)]
            crt_fd: Mutex::new(None),
            host_descriptor: false,
        });
        let ptr = alloc_file_handle_with_state(
            _py,
            Some(state),
            true,
            true,
            false,
            true,
            true,
            false,
            false,
            0,
            cls_bits,
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
pub extern "C" fn molt_stringio_new(cls_bits: u64, initial_bits: u64, newline_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if crate::builtins::type_ops::native_constructor_receiver(
            _py,
            builtin_classes(_py).string_io,
            Some(cls_bits),
            "StringIO",
        )
        .is_none()
        {
            return MoltObject::none().bits();
        }
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
            host_descriptor: false,
        });
        let ptr = alloc_file_handle_with_state(
            _py,
            Some(state),
            true,
            true,
            true,
            true,
            true,
            false,
            false,
            0,
            cls_bits,
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
