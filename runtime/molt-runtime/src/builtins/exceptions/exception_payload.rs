use super::*;
use wtf8::Wtf8;

fn exception_dict_attr_bits(_py: &PyToken<'_>, ptr: *mut u8, name: &[u8]) -> Option<u64> {
    let dict_bits = unsafe { exception_dict_bits(ptr) };
    if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
        return None;
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let key_bits = attr_name_bits_from_bytes(_py, name)?;
        let out = dict_get_in_place(_py, dict_ptr, key_bits);
        dec_ref_bits(_py, key_bits);
        out
    }
}

fn oserror_root_name(name: &str) -> bool {
    matches!(name, "OSError" | "EnvironmentError" | "IOError")
}

fn errno_is_shutdown(errno: i64) -> bool {
    #[cfg(all(not(windows), not(target_arch = "wasm32")))]
    {
        errno == libc::ESHUTDOWN as i64
    }
    #[cfg(windows)]
    {
        errno == crate::windows_abi::WSAESHUTDOWN as i64
    }
    #[cfg(all(not(windows), target_arch = "wasm32"))]
    {
        let _ = errno;
        false
    }
}

fn oserror_subclass_for_errno(errno: i64) -> Option<&'static str> {
    if errno == libc::EAGAIN as i64
        || errno == libc::EALREADY as i64
        || errno == libc::EWOULDBLOCK as i64
        || errno == libc::EINPROGRESS as i64
    {
        return Some("BlockingIOError");
    }
    if errno == libc::ECHILD as i64 {
        return Some("ChildProcessError");
    }
    if errno == libc::EPIPE as i64 {
        return Some("BrokenPipeError");
    }
    if errno_is_shutdown(errno) {
        return Some("BrokenPipeError");
    }
    if errno == libc::ECONNABORTED as i64 {
        return Some("ConnectionAbortedError");
    }
    if errno == libc::ECONNREFUSED as i64 {
        return Some("ConnectionRefusedError");
    }
    if errno == libc::ECONNRESET as i64 {
        return Some("ConnectionResetError");
    }
    if errno == libc::EEXIST as i64 {
        return Some("FileExistsError");
    }
    if errno == libc::ENOENT as i64 {
        return Some("FileNotFoundError");
    }
    if errno == libc::EINTR as i64 {
        return Some("InterruptedError");
    }
    if errno == libc::EISDIR as i64 {
        return Some("IsADirectoryError");
    }
    if errno == libc::ENOTDIR as i64 {
        return Some("NotADirectoryError");
    }
    if errno == libc::EACCES as i64 || errno == libc::EPERM as i64 {
        return Some("PermissionError");
    }
    #[cfg(target_os = "freebsd")]
    if errno == libc::ENOTCAPABLE as i64 {
        return Some("PermissionError");
    }
    if errno == libc::ESRCH as i64 {
        return Some("ProcessLookupError");
    }
    if errno == libc::ETIMEDOUT as i64 {
        return Some("TimeoutError");
    }
    None
}

#[derive(Clone, Copy)]
pub(super) struct OSErrorFields {
    pub(super) errno_value: Option<i64>,
    pub(super) errno_bits: u64,
    pub(super) strerror_bits: u64,
    pub(super) filename_bits: u64,
    pub(super) filename2_bits: u64,
    #[cfg(windows)]
    pub(super) winerror_bits: u64,
    pub(super) characters_written_bits: Option<u64>,
}

fn oserror_integral_i64(_py: &PyToken<'_>, bits: u64) -> Option<i64> {
    let int_type = builtin_classes(_py).int;
    (int_type != 0 && isinstance_bits(_py, bits, int_type))
        .then(|| to_i64(obj_from_bits(bits)))
        .flatten()
}

pub(super) unsafe fn oserror_fields_from_args(
    _py: &PyToken<'_>,
    class_bits: u64,
    args_bits: u64,
) -> OSErrorFields {
    let none = MoltObject::none().bits();
    let mut fields = OSErrorFields {
        errno_value: None,
        errno_bits: none,
        strerror_bits: none,
        filename_bits: none,
        filename2_bits: none,
        #[cfg(windows)]
        winerror_bits: none,
        characters_written_bits: None,
    };
    let Some(args_ptr) = obj_from_bits(args_bits).as_ptr() else {
        return fields;
    };
    let type_id = unsafe { object_type_id(args_ptr) };
    if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
        return fields;
    }
    let blocking_type = exception_type_bits_from_name(_py, "BlockingIOError");
    let exact_blocking = class_bits != 0 && class_bits == blocking_type;
    unsafe {
        crate::object::seq_access::with_borrowed(args_ptr, |elems| {
            if elems.len() < 2 {
                return;
            }
            fields.errno_bits = elems[0];
            fields.errno_value = oserror_integral_i64(_py, elems[0]);
            fields.strerror_bits = elems[1];
            if let Some(&third) = elems.get(2) {
                if exact_blocking && elems.len() == 3 && oserror_integral_i64(_py, third).is_some()
                {
                    fields.characters_written_bits = Some(third);
                } else {
                    fields.filename_bits = third;
                }
            }
            #[cfg(windows)]
            if let Some(&winerror) = elems.get(3) {
                fields.winerror_bits = winerror;
                if let Some(winerror_value) = oserror_integral_i64(_py, winerror) {
                    let errno = crate::windows_abi::winerror_to_errno(winerror_value as i32) as i64;
                    fields.errno_value = Some(errno);
                    fields.errno_bits = int_bits_from_i64(_py, errno);
                }
            }
            if let Some(&filename2) = elems.get(4) {
                fields.filename2_bits = filename2;
            }
        });
    }
    fields
}

/// Return one owned tuple containing the CPython-visible `OSError.args`.
/// Filename state lives only in the typed tail, while Windows replaces arg 0
/// with the canonical errno translated from arg 3.
pub(super) fn oserror_stored_args(
    _py: &PyToken<'_>,
    fields: OSErrorFields,
    args_bits: u64,
) -> Option<u64> {
    let args_ptr = obj_from_bits(args_bits).as_ptr()?;
    if unsafe { object_type_id(args_ptr) } != TYPE_ID_TUPLE {
        return None;
    }
    let none = MoltObject::none().bits();
    let mut stored = [none; 5];
    let (len, changed) = unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(args_ptr, |args| {
            if !(2..=5).contains(&args.len()) {
                return (args.len(), false);
            }
            stored[..args.len()].copy_from_slice(args);
            #[cfg(windows)]
            {
                stored[0] = fields.errno_bits;
            }
            let truncate = fields.filename_bits != none;
            let len = if truncate { 2 } else { args.len() };
            let changed = truncate || stored[0] != args[0];
            (len, changed)
        })
        .expect("type-checked OSError args tuple")
    };
    if !changed {
        inc_ref_bits(_py, args_bits);
        return Some(args_bits);
    }
    let ptr = alloc_tuple(_py, &stored[..len]);
    (!ptr.is_null()).then(|| MoltObject::from_ptr(ptr).bits())
}

pub(crate) fn raise_os_error_errno<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    errno: i64,
    message: &str,
) -> T {
    let errno_bits = MoltObject::from_int(errno).bits();
    let msg_ptr = alloc_string(_py, message.as_bytes());
    if msg_ptr.is_null() {
        return T::exception_sentinel();
    }
    let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
    let args_ptr = alloc_tuple(_py, &[errno_bits, msg_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, msg_bits);
        return T::exception_sentinel();
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let class_bits = exception_type_bits_from_name(_py, "OSError");
    let ptr = alloc_exception_from_class_bits(_py, class_bits, args_bits);
    dec_ref_bits(_py, args_bits);
    if !ptr.is_null() {
        record_exception_owned(_py, ptr);
    }
    T::exception_sentinel()
}

pub(crate) fn raise_os_error<T: ExceptionSentinel>(
    _py: &PyToken<'_>,
    err: std::io::Error,
    context: &str,
) -> T {
    let errno = err
        .raw_os_error()
        .map(|val| val as i64)
        .unwrap_or(libc::EIO as i64);
    let msg = if context.is_empty() {
        err.to_string()
    } else {
        format!("{context}: {}", err)
    };
    let msg = if msg.contains("Errno") {
        msg
    } else {
        format!("[Errno {errno}] {msg}")
    };
    raise_os_error_errno(_py, errno, &msg)
}

#[derive(Clone, Copy)]
pub(super) enum UnicodeErrorKind {
    Encode,
    Decode,
    Translate,
}

#[derive(Clone, Copy)]
pub(super) struct UnicodeErrorFields {
    pub(super) encoding_bits: u64,
    pub(super) object_bits: u64,
    pub(super) start_bits: u64,
    pub(super) end_bits: u64,
    pub(super) reason_bits: u64,
    owned_object: bool,
}

impl UnicodeErrorFields {
    pub(super) fn release_owned(self, _py: &PyToken<'_>) {
        if self.owned_object {
            dec_ref_bits(_py, self.object_bits);
        }
    }
}

pub(super) fn unicode_error_fields_from_args(
    _py: &PyToken<'_>,
    kind: UnicodeErrorKind,
    args_bits: u64,
) -> Result<UnicodeErrorFields, ()> {
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        return Err(());
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            return Err(());
        }
        let elems = crate::object::seq_access::pin_tuple(_py, args_ptr)
            .expect("type-checked UnicodeError args tuple must remain live");
        let expected = match kind {
            UnicodeErrorKind::Translate => 4,
            UnicodeErrorKind::Encode | UnicodeErrorKind::Decode => 5,
        };
        if elems.len() != expected {
            let msg = format!(
                "function takes exactly {expected} arguments ({} given)",
                elems.len()
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return Err(());
        }
        let (encoding_bits, mut object_bits, start_bits, end_bits, reason_bits, object_idx) =
            match kind {
                UnicodeErrorKind::Translate => (
                    MoltObject::none().bits(),
                    elems[0],
                    elems[1],
                    elems[2],
                    elems[3],
                    1,
                ),
                UnicodeErrorKind::Encode | UnicodeErrorKind::Decode => {
                    (elems[0], elems[1], elems[2], elems[3], elems[4], 2)
                }
            };
        let builtins = builtin_classes(_py);
        if matches!(kind, UnicodeErrorKind::Encode | UnicodeErrorKind::Decode)
            && !isinstance_bits(_py, encoding_bits, builtins.str)
        {
            let msg = format!(
                "argument 1 must be str, not {}",
                type_name(_py, obj_from_bits(encoding_bits))
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return Err(());
        }
        if !isinstance_bits(_py, reason_bits, builtins.str) {
            let arg_index = match kind {
                UnicodeErrorKind::Translate => 4,
                UnicodeErrorKind::Encode | UnicodeErrorKind::Decode => 5,
            };
            let msg = format!(
                "argument {arg_index} must be str, not {}",
                type_name(_py, obj_from_bits(reason_bits))
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            return Err(());
        }
        let mut owned_object = false;
        match kind {
            UnicodeErrorKind::Decode => {
                let Some(object_ptr) = obj_from_bits(object_bits).as_ptr() else {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, obj_from_bits(object_bits))
                    );
                    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                    return Err(());
                };
                let Some(bytes) = bytes_like_slice(object_ptr) else {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, obj_from_bits(object_bits))
                    );
                    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                    return Err(());
                };
                if object_type_id(object_ptr) != crate::TYPE_ID_BYTES {
                    let bytes_ptr = alloc_bytes(_py, bytes);
                    if bytes_ptr.is_null() {
                        return Err(());
                    }
                    object_bits = MoltObject::from_ptr(bytes_ptr).bits();
                    owned_object = true;
                }
            }
            UnicodeErrorKind::Encode | UnicodeErrorKind::Translate => {
                if !isinstance_bits(_py, object_bits, builtins.str) {
                    let msg = format!(
                        "argument {object_idx} must be str, not {}",
                        type_name(_py, obj_from_bits(object_bits))
                    );
                    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                    return Err(());
                }
            }
        }
        Ok(UnicodeErrorFields {
            encoding_bits,
            object_bits,
            start_bits,
            end_bits,
            reason_bits,
            owned_object,
        })
    }
}

pub(crate) fn alloc_exception_from_class_bits(
    _py: &PyToken<'_>,
    class_bits: u64,
    args_bits: u64,
) -> *mut u8 {
    let class_obj = obj_from_bits(class_bits);
    let Some(class_ptr) = class_obj.as_ptr() else {
        return std::ptr::null_mut();
    };
    unsafe {
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return std::ptr::null_mut();
        }
        let mut class_bits = class_bits;
        let args_bits = exception_normalize_args(_py, args_bits);
        if obj_from_bits(args_bits).is_none() {
            return std::ptr::null_mut();
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if base_group_bits != 0 && issubclass_bits(class_bits, base_group_bits) {
            return alloc_exception_group_from_class_bits(_py, class_bits, args_bits);
        }
        let oserror_bits = exception_type_bits_from_name(_py, "OSError");
        let mut oserror_layout = false;
        if issubclass_bits(class_bits, oserror_bits) {
            oserror_layout = true;
            let name = string_obj_to_owned(obj_from_bits(class_name_bits(class_ptr)))
                .expect("exception class must have a string name");
            let fields = oserror_fields_from_args(_py, class_bits, args_bits);
            if oserror_root_name(&name)
                && let Some(errno_val) = fields.errno_value
                && let Some(subclass) = oserror_subclass_for_errno(errno_val)
            {
                let mapped_bits = exception_type_bits_from_name(_py, subclass);
                if mapped_bits != 0 && obj_from_bits(mapped_bits).as_ptr().is_some() {
                    class_bits = mapped_bits;
                }
            }
        }
        let stored_args_bits = if oserror_layout {
            let fields = oserror_fields_from_args(_py, class_bits, args_bits);
            let Some(stored) = oserror_stored_args(_py, fields, args_bits) else {
                dec_ref_bits(_py, args_bits);
                return std::ptr::null_mut();
            };
            stored
        } else {
            inc_ref_bits(_py, args_bits);
            args_bits
        };
        let msg_bits = exception_message_for_storage(_py, class_bits, stored_args_bits);
        if !exception_message_storage_is_valid(_py, class_bits, msg_bits) {
            dec_ref_bits(_py, args_bits);
            dec_ref_bits(_py, stored_args_bits);
            return std::ptr::null_mut();
        }
        let none_bits = MoltObject::none().bits();
        let mut ptr = alloc_exception_obj(_py, class_bits, msg_bits, stored_args_bits, none_bits);
        if !ptr.is_null()
            && exception_initialize_typed_fields_from_args(_py, ptr, args_bits).is_err()
        {
            dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            ptr = std::ptr::null_mut();
        }
        dec_ref_bits(_py, args_bits);
        dec_ref_bits(_py, stored_args_bits);
        dec_ref_bits(_py, msg_bits);
        ptr
    }
}

fn exception_args_vec(ptr: *mut u8) -> Vec<u64> {
    unsafe {
        let args_bits = exception_args_bits(ptr);
        if exception_args_is_lazy_single(args_bits) {
            return vec![exception_args_payload_bits(ptr)];
        }
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr() {
            let type_id = object_type_id(args_ptr);
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                return crate::object::seq_access::with_borrowed(args_ptr, |items| items.to_vec());
            }
        }
        if args_obj.is_none() {
            Vec::new()
        } else {
            vec![args_bits]
        }
    }
}

fn exception_class_name(ptr: *mut u8) -> String {
    unsafe {
        let class_ptr = obj_from_bits(object_class_bits(ptr))
            .as_ptr()
            .expect("exception object must have a class edge");
        string_obj_to_owned(obj_from_bits(class_name_bits(class_ptr)))
            .expect("exception class must have a string name")
    }
}

pub(crate) fn format_exception(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let kind = exception_class_name(ptr);
    let args = exception_args_vec(ptr);
    if args.is_empty() {
        return format!("{kind}()");
    }
    if args.len() == 1 {
        let arg_repr = format_obj(_py, obj_from_bits(args[0]));
        return format!("{kind}({arg_repr})");
    }
    let args_repr = format_obj(_py, obj_from_bits(unsafe { exception_args_bits(ptr) }));
    format!("{kind}{args_repr}")
}

pub(crate) fn format_exception_with_traceback(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    // CPython displays chained exceptions recursively: context first,
    // then a separator, then the current exception.
    let suppress = unsafe { exception_suppress_bits(ptr) };
    let suppress_context = is_truthy(_py, obj_from_bits(suppress));
    if !suppress_context {
        let cause_bits = unsafe { exception_cause_bits(ptr) };
        let context_bits = unsafe { exception_context_bits(ptr) };
        if let Some(cause_ptr) = obj_from_bits(cause_bits).as_ptr() {
            if unsafe { object_type_id(cause_ptr) } == TYPE_ID_EXCEPTION {
                let mut chain = format_exception_with_traceback(_py, cause_ptr);
                chain.push_str(
                    "\nThe above exception was the direct cause of the following exception:\n\n",
                );
                chain.push_str(&format_single_exception(_py, ptr));
                return chain;
            }
        } else if let Some(ctx_ptr) = obj_from_bits(context_bits).as_ptr()
            && unsafe { object_type_id(ctx_ptr) } == TYPE_ID_EXCEPTION
        {
            let mut chain = format_exception_with_traceback(_py, ctx_ptr);
            chain.push_str(
                "\nDuring handling of the above exception, another exception occurred:\n\n",
            );
            chain.push_str(&format_single_exception(_py, ptr));
            return chain;
        }
    }
    format_single_exception(_py, ptr)
}

fn format_single_exception(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let mut out = String::new();
    if let Some(trace) = format_traceback(_py, ptr) {
        out.push_str(&trace);
    } else {
        // No traceback object attached — emit a minimal CPython-compatible
        // header from the frame stack.  Most module-level exceptions in
        // AOT-compiled code lack traceback objects because they're raised
        // by runtime intrinsics, not Python-level raise statements.
        out.push_str("Traceback (most recent call last):\n");
        if let Some((file, line, name, col, end_col)) = frame_stack_top_info(_py) {
            out.push_str(&format!("  File \"{file}\", line {line}, in {name}\n"));
            if let Some(src_line) = read_source_line(&file, line) {
                let trimmed = src_line.trim_start();
                let trim_offset = (src_line.len() - trimmed.len()) as i64;
                out.push_str(&format!("    {}\n", trimmed));
                // Only show carets when precise col_offset data is available.
                // No heuristic fallback — CPython shows no caret for frames
                // without column info in the code object.
                if col >= 0 && end_col >= 0 {
                    let c = col - trim_offset;
                    let ec = end_col - trim_offset;
                    let caret =
                        crate::object::ops_sys::traceback_format_caret_line_native(trimmed, c, ec);
                    if !caret.is_empty() {
                        out.push_str(&caret);
                    }
                }
            }
        }
    }
    let kind = exception_class_name(ptr);
    let message = format_exception_message(_py, ptr);
    if message.is_empty() {
        out.push_str(&kind);
    } else {
        out.push_str(&format!("{kind}: {message}"));
    }
    out
}

pub(crate) fn format_exception_message(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let class_bits = unsafe { object_class_bits(ptr) };
    let kind = exception_class_name(ptr);
    let exception_bits = MoltObject::from_ptr(ptr).bits();
    if exception_matches_builtin_name(_py, exception_bits, "UnicodeDecodeError")
        && let Some(msg) = format_unicode_decode_error(_py, ptr)
    {
        return msg;
    }
    if exception_matches_builtin_name(_py, exception_bits, "UnicodeEncodeError")
        && let Some(msg) = format_unicode_encode_error(_py, ptr)
    {
        return msg;
    }
    if kind == "HTTPError"
        && let (Some(code_bits), Some(msg_bits)) = (
            exception_dict_attr_bits(_py, ptr, b"code"),
            exception_dict_attr_bits(_py, ptr, b"msg"),
        )
    {
        let code = format_obj_str(_py, obj_from_bits(code_bits));
        let msg = format_obj_str(_py, obj_from_bits(msg_bits));
        return format!("HTTP Error {code}: {msg}");
    }
    if (kind == "URLError" || kind == "ContentTooShortError")
        && let Some(reason_bits) = exception_dict_attr_bits(_py, ptr, b"reason")
    {
        let reason = format_obj_str(_py, obj_from_bits(reason_bits));
        return format!("<urlopen error {reason}>");
    }
    let base_group_bits = builtin_classes(_py).base_exception_group;
    if base_group_bits != 0 && issubclass_bits(class_bits, base_group_bits) {
        let msg_bits = exception_group_message_bits(_py, ptr);
        let msg = format_obj_str(_py, obj_from_bits(msg_bits));
        let mut count = 0usize;
        if let Some(ex_bits) = exception_group_exceptions_bits(_py, ptr)
            && let Some(ex_ptr) = obj_from_bits(ex_bits).as_ptr()
        {
            unsafe {
                let type_id = object_type_id(ex_ptr);
                if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                    count = crate::object::seq_access::len(ex_ptr);
                }
            }
        }
        let suffix = if count == 1 {
            "1 sub-exception".to_string()
        } else {
            format!("{count} sub-exceptions")
        };
        if msg.is_empty() {
            return format!(" ({suffix})");
        }
        return format!("{msg} ({suffix})");
    }
    let args = exception_args_vec(ptr);
    if args.is_empty() {
        return String::new();
    }
    if exception_matches_builtin_name(_py, exception_bits, "KeyError") && args.len() == 1 {
        return format_obj(_py, obj_from_bits(args[0]));
    }
    if args.len() == 1 {
        return format_obj_str(_py, obj_from_bits(args[0]));
    }
    format_obj_str(_py, obj_from_bits(unsafe { exception_args_bits(ptr) }))
}

fn format_unicode_decode_error(_py: &PyToken<'_>, ptr: *mut u8) -> Option<String> {
    let encoding_bits =
        unsafe { exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeEncoding)? };
    let object_bits =
        unsafe { exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeObject)? };
    let start = unsafe {
        exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeStart)? as isize as i64
    };
    let end = unsafe {
        exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeEnd)? as isize as i64
    };
    let reason_bits =
        unsafe { exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeReason)? };
    let encoding = string_obj_to_owned(obj_from_bits(encoding_bits))?;
    let reason = string_obj_to_owned(obj_from_bits(reason_bits))?;
    if start < 0 || end < 0 {
        return None;
    }
    let start = start as usize;
    let end = end as usize;
    if end <= start {
        return None;
    }
    if end == start + 1 {
        let obj = obj_from_bits(object_bits);
        let ptr = obj.as_ptr()?;
        let bytes = unsafe { bytes_like_slice(ptr) }?;
        if start >= bytes.len() {
            return None;
        }
        let byte = bytes[start];
        return Some(format!(
            "'{encoding}' codec can't decode byte 0x{byte:02x} in position {start}: {reason}"
        ));
    }
    let end_pos = end.saturating_sub(1);
    Some(format!(
        "'{encoding}' codec can't decode bytes in position {start}-{end_pos}: {reason}"
    ))
}

fn unicode_escape_codepoint(code: u32) -> String {
    if code <= 0xFF {
        format!("\\x{code:02x}")
    } else if code <= 0xFFFF {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

fn wtf8_from_bytes(bytes: &[u8]) -> &Wtf8 {
    // SAFETY: Molt string bytes are constructed as well-formed WTF-8.
    unsafe { &*(bytes as *const [u8] as *const Wtf8) }
}

fn wtf8_codepoint_at_index(bytes: &[u8], idx: usize) -> Option<u32> {
    wtf8_from_bytes(bytes)
        .code_points()
        .nth(idx)
        .map(|cp| cp.to_u32())
}

fn format_unicode_encode_error(_py: &PyToken<'_>, ptr: *mut u8) -> Option<String> {
    let encoding_bits =
        unsafe { exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeEncoding)? };
    let object_bits =
        unsafe { exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeObject)? };
    let start = unsafe {
        exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeStart)? as isize as i64
    };
    let end = unsafe {
        exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeEnd)? as isize as i64
    };
    let reason_bits =
        unsafe { exception_typed_field_raw_bits(ptr, ExceptionTypedField::UnicodeReason)? };
    let encoding = string_obj_to_owned(obj_from_bits(encoding_bits))?;
    let reason = string_obj_to_owned(obj_from_bits(reason_bits))?;
    if start < 0 || end < 0 {
        return None;
    }
    let start = start as usize;
    let end = end as usize;
    if end <= start {
        return None;
    }
    let obj = obj_from_bits(object_bits);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_STRING {
            return None;
        }
        let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
        if end == start + 1 {
            let code = wtf8_codepoint_at_index(bytes, start)?;
            let escaped = unicode_escape_codepoint(code);
            return Some(format!(
                "'{encoding}' codec can't encode character '{escaped}' in position {start}: {reason}"
            ));
        }
    }
    let end_pos = end.saturating_sub(1);
    Some(format!(
        "'{encoding}' codec can't encode characters in position {start}-{end_pos}: {reason}"
    ))
}

fn format_traceback(_py: &PyToken<'_>, ptr: *mut u8) -> Option<String> {
    let trace_bits = unsafe { exception_trace_bits(ptr) };
    if obj_from_bits(trace_bits).is_none() {
        return None;
    }
    if traceback_payload_is_lazy(trace_bits) {
        let payload = crate::object::ops_sys::traceback_payload_from_source(_py, trace_bits, None);
        if payload.is_empty() {
            return None;
        }
        let mut out = String::from("Traceback (most recent call last):\n");
        out.extend(crate::object::ops_sys::traceback_payload_to_formatted_lines(_py, &payload));
        return Some(out);
    }
    let mut out = String::from("Traceback (most recent call last):\n");
    let tb_frame_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.tb_lineno_name,
        b"tb_lineno",
    );
    let tb_next_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
    let f_code_bits = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_bits =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let mut current_bits = trace_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if depth > 512 {
            out.push_str("  <traceback truncated>\n");
            break;
        }
        let tb_obj = obj_from_bits(current_bits);
        let Some(tb_ptr) = tb_obj.as_ptr() else {
            break;
        };
        let (frame_bits, line, next_bits) = unsafe {
            let dict_bits = instance_dict_bits(tb_ptr);
            let mut frame_bits = MoltObject::none().bits();
            let mut line = 0i64;
            let mut next_bits = MoltObject::none().bits();
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_frame_bits) {
                    frame_bits = bits;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_lineno_bits)
                    && let Some(val) = to_i64(obj_from_bits(bits))
                {
                    line = val;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_next_bits) {
                    next_bits = bits;
                }
            }
            (frame_bits, line, next_bits)
        };
        let (filename, func_name, frame_line) = unsafe {
            let mut filename = "<unknown>".to_string();
            let mut func_name = "<module>".to_string();
            let mut frame_line = line;
            if let Some(frame_ptr) = obj_from_bits(frame_bits).as_ptr() {
                let dict_bits = instance_dict_bits(frame_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_lineno_bits)
                        && let Some(val) = to_i64(obj_from_bits(bits))
                    {
                        frame_line = val;
                    }
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_code_bits)
                        && let Some(code_ptr) = obj_from_bits(bits).as_ptr()
                        && object_type_id(code_ptr) == TYPE_ID_CODE
                    {
                        let filename_bits = code_filename_bits(code_ptr);
                        if let Some(name) = string_obj_to_owned(obj_from_bits(filename_bits)) {
                            filename = name;
                        }
                        let name_bits = code_name_bits(code_ptr);
                        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits))
                            && !name.is_empty()
                        {
                            func_name = name;
                        }
                    }
                }
            }
            (filename, func_name, frame_line)
        };
        let final_line = if line > 0 { line } else { frame_line };
        // Skip the synthetic <module> wrapper frame that molt_main pushes.
        // CPython doesn't have this frame — it goes directly to the module
        // chunk which has the real filename and line number.
        if filename == "<module>" && func_name == "<module>" {
            current_bits = next_bits;
            depth += 1;
            continue;
        }
        out.push_str(&format!(
            "  File \"{filename}\", line {final_line}, in {func_name}\n"
        ));
        if let Some(src_line) = read_source_line(&filename, final_line) {
            let trimmed = src_line.trim_start();
            let trim_offset = (src_line.len() - trimmed.len()) as i64;
            out.push_str(&format!("    {}\n", trimmed));
            // Use col_offset stashed at exception-raise time.
            // Use precise col_offset stashed at exception-raise time.
            // No heuristic fallback — CPython shows no caret for frames
            // without column info in the code object.
            let saved_col = LAST_EXCEPTION_COL.with(|cell| {
                let val = *cell.borrow();
                *cell.borrow_mut() = (-1, -1);
                val
            });
            if saved_col.0 >= 0 && saved_col.1 >= 0 {
                let c = saved_col.0 - trim_offset;
                let ec = saved_col.1 - trim_offset;
                let caret =
                    crate::object::ops_sys::traceback_format_caret_line_native(trimmed, c, ec);
                if !caret.is_empty() {
                    out.push_str(&caret);
                }
            }
        }
        current_bits = next_bits;
        depth += 1;
    }
    Some(out)
}

/// Read a single source line from a file for traceback display.
/// Returns None if the file can't be read or the line doesn't exist.
/// Matches CPython's `linecache.getline` behaviour for AOT tracebacks.
fn read_source_line(filename: &str, lineno: i64) -> Option<String> {
    if lineno <= 0 || filename.is_empty() || filename == "<unknown>" || filename == "<module>" {
        return None;
    }
    let content = std::fs::read_to_string(filename).ok()?;
    content.lines().nth((lineno - 1) as usize).map(String::from)
}
