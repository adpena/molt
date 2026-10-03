use super::*;

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
    let missing = exception_field_missing_bits();
    let mut fields = OSErrorFields {
        errno_value: None,
        errno_bits: missing,
        strerror_bits: missing,
        filename_bits: missing,
        filename2_bits: missing,
        #[cfg(windows)]
        winerror_bits: missing,
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
            if !(2..=5).contains(&elems.len()) {
                return;
            }
            fields.errno_bits = elems[0];
            fields.errno_value = oserror_integral_i64(_py, elems[0]);
            fields.strerror_bits = elems[1];
            if let Some(&third) = elems.get(2) {
                if exact_blocking && elems.len() == 3 && oserror_integral_i64(_py, third).is_some()
                {
                    fields.characters_written_bits = Some(third);
                } else if !obj_from_bits(third).is_none() {
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
            if !exception_field_is_missing(fields.filename_bits)
                && let Some(&filename2) = elems.get(4)
                && !obj_from_bits(filename2).is_none()
            {
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
            let truncate = !exception_field_is_missing(fields.filename_bits);
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
                    exception_field_missing_bits(),
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
            let fields = oserror_fields_from_args(_py, class_bits, args_bits);
            // CPython promotes errno only for the canonical exact OSError.
            // Aliases already share that identity; names on subclasses are mutable.
            if class_bits == oserror_bits
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

pub(crate) fn format_exception_with_traceback(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let mut rendered = String::new();
    let ok = with_saved_raised_exception(_py, || {
        let Some(chain) = crate::object::ops_sys::traceback_exception_chain(
            _py,
            MoltObject::from_ptr(ptr).bits(),
        ) else {
            return false;
        };
        for index in (0..chain.len()).rev() {
            if index + 1 < chain.len() {
                rendered.push('\n');
                rendered.push_str(chain[index + 1].separator.expect("linked exception"));
            }
            let ptr = obj_from_bits(chain[index].value.bits())
                .as_ptr()
                .expect("admitted exception");
            rendered.push_str(&format_single_exception(_py, ptr));
            if exception_pending(_py) {
                return false;
            }
        }
        true
    });
    if ok { rendered } else { String::new() }
}

fn format_single_exception(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let mut out = String::new();
    let trace = match format_traceback(_py, ptr) {
        Ok(trace) => trace,
        Err(_) => return out,
    };
    if let Some(trace) = trace {
        out.push_str(&trace);
    } else {
        // No traceback object attached — emit a minimal CPython-compatible
        // header from the frame stack.  Most module-level exceptions in
        // AOT-compiled code lack traceback objects because they're raised
        // by runtime intrinsics, not Python-level raise statements.
        out.push_str("Traceback (most recent call last):\n");
        if let Some((file, line, name, col, end_col)) = frame_stack_top_info(_py) {
            let source =
                crate::object::ops_sys::traceback_source_line_native(_py, file.as_bytes(), line);
            let frame = crate::object::ops_sys::TracebackPayloadFrame {
                filename: file.into_bytes(),
                lineno: line,
                end_lineno: line,
                colno: col,
                end_colno: end_col,
                name: name.into_bytes(),
                line: source,
            };
            let rendered = match crate::object::ops_sys::traceback_payload_format_frame(_py, &frame)
            {
                Ok(rendered) => rendered,
                Err(_) => return String::new(),
            };
            out.push_str(&String::from_utf8_lossy(&rendered));
        }
    }
    let Some(storage) = ExceptionStorage::for_exception(_py, MoltObject::from_ptr(ptr).bits())
    else {
        return String::new();
    };
    let kind = storage.class_name();
    let message = format_exception_message(_py, ptr);
    if message.is_empty() {
        out.push_str(&kind);
    } else {
        out.push_str(&format!("{kind}: {message}"));
    }
    out
}

/// Diagnostics may render while the same exception is pending. The existing
/// raised-state transaction clears that input during dispatch and restores its
/// exact owner on success; a callback failure remains the new pending error.
pub(crate) fn format_exception_message(py: &PyToken<'_>, ptr: *mut u8) -> String {
    String::from_utf8_lossy(&format_exception_message_bytes(py, ptr)).into_owned()
}

pub(crate) fn format_exception_message_bytes(py: &PyToken<'_>, ptr: *mut u8) -> Vec<u8> {
    let mut rendered = Vec::new();
    with_saved_raised_exception(py, || {
        let bits = molt_exception_message(MoltObject::from_ptr(ptr).bits());
        if exception_pending(py) {
            dec_ref_bits(py, bits);
            return false;
        }
        rendered =
            crate::object::ops_format::string_obj_bytes(obj_from_bits(bits)).unwrap_or_default();
        dec_ref_bits(py, bits);
        !exception_pending(py)
    });
    rendered
}

fn format_traceback(_py: &PyToken<'_>, ptr: *mut u8) -> Result<Option<String>, u64> {
    let trace = exception_field(
        _py,
        MoltObject::from_ptr(ptr).bits(),
        ExceptionFieldSlot::Traceback,
    )
    .ok_or_else(|| MoltObject::none().bits())?;
    let trace_bits = trace.bits();
    if obj_from_bits(trace_bits).is_none() {
        return Ok(None);
    }
    let was_lazy = traceback_payload_is_lazy(trace_bits);
    let mut payload = crate::object::ops_sys::traceback_payload_from_source(_py, trace_bits, None);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if !was_lazy {
        // The eager traceback path historically records a precise raise-site
        // span outside the traceback object. Apply it to the innermost frame
        // before using the common source/caret renderer.
        let saved_col = LAST_EXCEPTION_COL.with(|cell| {
            let mut slot = cell.borrow_mut();
            let saved = *slot;
            *slot = (-1, -1);
            saved
        });
        if saved_col.0 >= 0
            && saved_col.1 > saved_col.0
            && let Some(frame) = payload.last_mut()
        {
            frame.colno = saved_col.0;
            frame.end_colno = saved_col.1;
        }
        // The synthetic molt_main wrapper is not a Python traceback frame.
        payload.retain(|frame| !(frame.filename == b"<module>" && frame.name == b"<module>"));
    }
    if payload.is_empty() {
        return Ok(None);
    }
    let mut out = String::from("Traceback (most recent call last):\n");
    for entry in crate::object::ops_sys::traceback_payload_to_formatted_entries(_py, &payload)? {
        out.push_str(&String::from_utf8_lossy(&entry));
    }
    Ok(Some(out))
}
