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
pub(crate) use super::io_path_utils::*;
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

#[path = "io/buffer.rs"]
mod buffer;
pub(crate) use buffer::collect_bytes_like;
#[cfg(test)]
use buffer::file_remaining_bytes_hint;
use buffer::{
    backend_flush, backend_read_bytes, backend_seek, backend_tell, backend_truncate,
    backend_write_bytes, buffered_read_bytes, buffered_read_into, clear_read_buffer,
    clear_write_buffer, file_read1_bytes, flush_write_buffer, handle_read_byte,
    memory_backend_vec_ref, prepend_read_buffer, rewind_read_buffer, unread_bytes,
};
#[path = "io/handle.rs"]
mod handle;
use handle::{
    VfsWritebackEntry, alloc_file_handle_with_state, resolve_file_handle_ptr,
    vfs_writeback_register, vfs_writeback_take,
};
pub(crate) use handle::{
    close_payload, file_handle_close_ptr, file_handle_detached_message, file_handle_enter,
    file_handle_exit, file_handle_is_closed, file_handle_require_attached,
};
#[path = "io/text.rs"]
mod text;
use text::*;
#[path = "io/open.rs"]
mod open;
pub(crate) use open::dup_fd;
#[cfg(windows)]
use open::windows_handle_isatty;
#[cfg(windows)]
pub(crate) use open::windows_path_from_handle;
pub use open::{
    molt_file_io_init, molt_file_io_new, molt_file_open, molt_file_open_ex, molt_open_builtin,
    molt_sys_stderr, molt_sys_stdin, molt_sys_stdout,
};
use open::{open_arg_newline, reconfigure_arg_newline, reconfigure_arg_type};
#[path = "io/construct.rs"]
mod construct;
pub use construct::{
    molt_buffered_init, molt_buffered_new, molt_bytesio_init, molt_bytesio_new, molt_io_class,
    molt_stringio_init, molt_stringio_new, molt_text_io_wrapper_init, molt_text_io_wrapper_new,
};

const DEFAULT_BUFFER_SIZE: i64 = 8192;

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
