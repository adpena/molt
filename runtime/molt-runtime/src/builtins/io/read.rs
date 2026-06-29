use super::*;

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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
        if let Err(bits) = file_handle_require_attached(_py, handle) {
            return bits;
        }
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
        let mut export = BufferExport::default();
        if molt_buffer_export(buffer_bits, &mut export) != 0 || export.readonly != 0 {
            let msg = format!("{name}() argument must be a writable bytes-like object");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if export.ndim != 1 || export.itemsize != 1 || export.strides[0] != 1 {
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
            if let Err(bits) = file_handle_require_attached(_py, handle) {
                return bits;
            }
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
