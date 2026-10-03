//! Declaring exception repr/str slots. Managed views and native exceptions use
//! the same physical fields and hold owned snapshots across every Python call.

use super::{PyErr_BadInternalCall, PyErr_SetString, release_preserving_error};
use crate::abi_types::{Py_ssize_t, PyBaseExceptionObject, PyObject, PyTypeObject};
use crate::api::{numbers, object, refcount, sequences, strings, typeobj};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::{
    ExceptionFieldStorage, ExceptionLayoutKind, ExceptionStrSlot, ExceptionTypedField,
    MAX_EXCEPTION_TYPED_FIELDS,
};
use std::ptr;

type StringSlot = unsafe extern "C" fn(*mut PyObject) -> *mut PyObject;

pub(crate) fn exception_str_slot(slot: ExceptionStrSlot) -> StringSlot {
    match slot {
        ExceptionStrSlot::Base => molt_native_exception_str,
        ExceptionStrSlot::Group => group_str,
        ExceptionStrSlot::KeyError => key_error_str,
        ExceptionStrSlot::Syntax => syntax_error_str,
        ExceptionStrSlot::Import => import_error_str,
        ExceptionStrSlot::OSError => os_error_str,
        ExceptionStrSlot::UnicodeDecode => unicode_decode_error_str,
        ExceptionStrSlot::UnicodeEncode => unicode_encode_error_str,
        ExceptionStrSlot::UnicodeTranslate => unicode_translate_error_str,
    }
}

struct RenderSnapshot {
    class: *mut PyTypeObject,
    args: *mut PyObject,
    layout: ExceptionLayoutKind,
    fields: [*mut PyObject; MAX_EXCEPTION_TYPED_FIELDS],
    start: Py_ssize_t,
    end: Py_ssize_t,
    unicode_object_present: bool,
}

impl RenderSnapshot {
    unsafe fn capture(op: *mut PyObject, required: Option<ExceptionLayoutKind>) -> Option<Self> {
        unsafe { Self::capture_fields(op, required, true, &[]) }
    }

    unsafe fn capture_fields(
        op: *mut PyObject,
        required: Option<ExceptionLayoutKind>,
        capture_args: bool,
        fields: &[ExceptionTypedField],
    ) -> Option<Self> {
        if op.is_null() {
            unsafe { PyErr_BadInternalCall() };
            return None;
        }
        if let Some(value) = GLOBAL_BRIDGE.observed_handle_for_pyobj(op) {
            // Refresh once, before taking any borrowed field. The runtime
            // projection owns args materialization and typed-field custody.
            if !GLOBAL_BRIDGE.refresh_exception_view(value.bits()) {
                return None;
            }
        } else if super::foreign_exception_layout(op).is_none() {
            unsafe { bad_receiver() };
            return None;
        }
        let class = unsafe { (*op).ob_type };
        let Some(layout) = (unsafe { crate::abi_types::exception_layout_for_type(class) }) else {
            unsafe { bad_receiver() };
            return None;
        };
        if required.is_some_and(|required| required != layout) {
            unsafe { bad_receiver() };
            return None;
        }
        let base = op.cast::<PyBaseExceptionObject>();
        let mut snapshot = Self {
            class,
            args: if capture_args {
                unsafe { (*base).args }
            } else {
                ptr::null_mut()
            },
            layout,
            fields: [ptr::null_mut(); MAX_EXCEPTION_TYPED_FIELDS],
            start: 0,
            end: 0,
            unicode_object_present: false,
        };
        unsafe {
            refcount::Py_XINCREF(class.cast());
            refcount::Py_XINCREF(snapshot.args);
            for (index, policy) in layout.field_policies().iter().enumerate() {
                if policy.storage != ExceptionFieldStorage::PySsize
                    && fields.contains(&policy.field)
                {
                    let slot =
                        crate::abi_types::exception_typed_object_slot(base, layout, policy.field)
                            .expect("schema object field must have physical storage");
                    snapshot.fields[index] = *slot;
                    refcount::Py_XINCREF(*slot);
                }
            }
            if layout == ExceptionLayoutKind::Unicode {
                let unicode = op.cast::<crate::abi_types::PyUnicodeErrorObject>();
                snapshot.start = (*unicode).start;
                snapshot.end = (*unicode).end;
                snapshot.unicode_object_present = !(*unicode).object.is_null();
            }
        }
        Some(snapshot)
    }

    fn field(&self, field: ExceptionTypedField) -> *mut PyObject {
        self.layout
            .field_policies()
            .iter()
            .position(|policy| policy.field == field)
            .map_or(ptr::null_mut(), |index| self.fields[index])
    }

    unsafe fn argc(&self) -> Py_ssize_t {
        if self.args.is_null() {
            0
        } else {
            unsafe { sequences::PyTuple_Size(self.args) }
        }
    }

    unsafe fn base_str(&self) -> *mut PyObject {
        match unsafe { self.argc() } {
            0 => unsafe { from_bytes(b"") },
            1 => {
                let item = unsafe { sequences::PyTuple_GetItem(self.args, 0) };
                if item.is_null() {
                    ptr::null_mut()
                } else {
                    unsafe { typeobj::PyObject_Str(item) }
                }
            }
            count if count > 1 => unsafe { typeobj::PyObject_Str(self.args) },
            _ => ptr::null_mut(),
        }
    }
}

impl Drop for RenderSnapshot {
    fn drop(&mut self) {
        // Callback rebinding can make these the last owners. Destruction must
        // neither replace a rendering failure nor introduce a cleanup error.
        super::with_preserved_error(|| unsafe {
            refcount::Py_XDECREF(self.args);
            for &field in &self.fields {
                refcount::Py_XDECREF(field);
            }
            refcount::Py_XDECREF(self.class.cast());
        });
    }
}

unsafe fn bad_receiver() {
    unsafe {
        PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast(),
            c"exception rendering slot requires a compatible exception instance".as_ptr(),
        )
    };
}

unsafe fn from_bytes(bytes: &[u8]) -> *mut PyObject {
    unsafe { strings::unicode_from_python_bytes(bytes) }
}

fn or_none(value: *mut PyObject) -> *mut PyObject {
    if value.is_null() {
        &raw mut crate::abi_types::Py_None
    } else {
        value
    }
}

/// Copy while the result is owned. Raw string bytes preserve lone surrogates;
/// encoding to UTF-8 or a Rust String here would change Python string values.
unsafe fn take_string_bytes(value: *mut PyObject) -> Option<Vec<u8>> {
    if value.is_null() {
        return None;
    }
    let bytes = unsafe { strings::unicode_bytes(value) }.map(<[u8]>::to_vec);
    if bytes.is_none() {
        unsafe { PyErr_BadInternalCall() };
    }
    unsafe { release_preserving_error(&[value]) };
    bytes
}

unsafe fn rendered_bytes(value: *mut PyObject, repr: bool) -> Option<Vec<u8>> {
    let rendered = if repr {
        unsafe { typeobj::PyObject_Repr(value) }
    } else {
        unsafe { typeobj::PyObject_Str(value) }
    };
    unsafe { take_string_bytes(rendered) }
}

/// Unicode error slots keep reason/encoding string results alive until their
/// final formatting is complete, then release them in evaluation order.
#[derive(Default)]
struct StringResults {
    objects: Vec<*mut PyObject>,
}

impl StringResults {
    unsafe fn str_bytes(&mut self, value: *mut PyObject) -> Option<Vec<u8>> {
        let result = unsafe { typeobj::PyObject_Str(value) };
        if result.is_null() {
            return None;
        }
        self.objects.push(result);
        let bytes = unsafe { strings::unicode_bytes(result) }.map(<[u8]>::to_vec);
        if bytes.is_none() {
            unsafe { PyErr_BadInternalCall() };
        }
        bytes
    }
}

impl Drop for StringResults {
    fn drop(&mut self) {
        unsafe { release_preserving_error(&self.objects) };
    }
}

pub unsafe extern "C" fn molt_native_exception_str(op: *mut PyObject) -> *mut PyObject {
    let Some(snapshot) = (unsafe { RenderSnapshot::capture(op, None) }) else {
        return ptr::null_mut();
    };
    unsafe { snapshot.base_str() }
}

pub unsafe extern "C" fn molt_native_exception_repr(op: *mut PyObject) -> *mut PyObject {
    let Some(snapshot) = (unsafe { RenderSnapshot::capture(op, None) }) else {
        return ptr::null_mut();
    };
    let Some(mut out) = (unsafe { typeobj::type_short_name_bytes(snapshot.class) }) else {
        return ptr::null_mut();
    };
    let count = unsafe { snapshot.argc() };
    if count < 0 {
        return ptr::null_mut();
    }
    if count == 0 {
        out.extend_from_slice(b"()");
    } else {
        let value = if count == 1 {
            out.push(b'(');
            unsafe { sequences::PyTuple_GetItem(snapshot.args, 0) }
        } else {
            snapshot.args
        };
        if value.is_null() {
            return ptr::null_mut();
        }
        let Some(rendered) = (unsafe { rendered_bytes(value, true) }) else {
            return ptr::null_mut();
        };
        out.extend_from_slice(&rendered);
        if count == 1 {
            out.push(b')');
        }
    }
    unsafe { from_bytes(&out) }
}

unsafe extern "C" fn key_error_str(op: *mut PyObject) -> *mut PyObject {
    let Some(snapshot) = (unsafe { RenderSnapshot::capture(op, None) }) else {
        return ptr::null_mut();
    };
    if unsafe { snapshot.argc() } == 1 {
        let item = unsafe { sequences::PyTuple_GetItem(snapshot.args, 0) };
        if item.is_null() {
            ptr::null_mut()
        } else {
            unsafe { typeobj::PyObject_Repr(item) }
        }
    } else {
        unsafe { snapshot.base_str() }
    }
}

unsafe extern "C" fn group_str(op: *mut PyObject) -> *mut PyObject {
    let Some(snapshot) = (unsafe {
        RenderSnapshot::capture_fields(
            op,
            Some(ExceptionLayoutKind::Group),
            false,
            &[
                ExceptionTypedField::GroupMessage,
                ExceptionTypedField::GroupExceptions,
            ],
        )
    }) else {
        return ptr::null_mut();
    };
    let count =
        unsafe { sequences::PyTuple_Size(snapshot.field(ExceptionTypedField::GroupExceptions)) };
    if count < 0 {
        return ptr::null_mut();
    }
    let Some(mut out) = (unsafe {
        rendered_bytes(
            or_none(snapshot.field(ExceptionTypedField::GroupMessage)),
            false,
        )
    }) else {
        return ptr::null_mut();
    };
    out.extend_from_slice(
        format!(
            " ({count} sub-exception{})",
            if count > 1 { "s" } else { "" }
        )
        .as_bytes(),
    );
    unsafe { from_bytes(&out) }
}

unsafe extern "C" fn import_error_str(op: *mut PyObject) -> *mut PyObject {
    let Some(snapshot) = (unsafe {
        RenderSnapshot::capture_fields(
            op,
            Some(ExceptionLayoutKind::Import),
            true,
            &[ExceptionTypedField::ImportMessage],
        )
    }) else {
        return ptr::null_mut();
    };
    let message = snapshot.field(ExceptionTypedField::ImportMessage);
    if !message.is_null() && unsafe { strings::PyUnicode_CheckExact(message) } != 0 {
        unsafe { object::Py_NewRef(message) }
    } else {
        unsafe { snapshot.base_str() }
    }
}

unsafe extern "C" fn syntax_error_str(op: *mut PyObject) -> *mut PyObject {
    let Some(snapshot) = (unsafe {
        RenderSnapshot::capture_fields(
            op,
            Some(ExceptionLayoutKind::Syntax),
            false,
            &[
                ExceptionTypedField::SyntaxMessage,
                ExceptionTypedField::SyntaxFilename,
                ExceptionTypedField::SyntaxLineNumber,
            ],
        )
    }) else {
        return ptr::null_mut();
    };
    let filename = snapshot.field(ExceptionTypedField::SyntaxFilename);
    let filename = if !filename.is_null() && unsafe { strings::PyUnicode_Check(filename) } != 0 {
        let Some(bytes) = (unsafe { strings::unicode_bytes(filename) }) else {
            unsafe { PyErr_BadInternalCall() };
            return ptr::null_mut();
        };
        let separator = if cfg!(windows) { b'\\' } else { b'/' };
        let offset = bytes
            .iter()
            .rposition(|byte| *byte == separator)
            .map_or(0, |index| index + 1);
        Some(bytes[offset..].to_vec())
    } else {
        None
    };
    let line = snapshot.field(ExceptionTypedField::SyntaxLineNumber);
    let line = if !line.is_null() && unsafe { numbers::PyLong_CheckExact(line) } != 0 {
        let mut overflow = 0;
        Some(unsafe { numbers::PyLong_AsLongAndOverflow(line, &raw mut overflow) })
    } else {
        None
    };
    let message = or_none(snapshot.field(ExceptionTypedField::SyntaxMessage));
    if filename.is_none() && line.is_none() {
        return unsafe { typeobj::PyObject_Str(message) };
    }
    let Some(mut out) = (unsafe { rendered_bytes(message, false) }) else {
        return ptr::null_mut();
    };
    out.extend_from_slice(b" (");
    if let Some(filename) = &filename {
        out.extend_from_slice(filename);
        if line.is_some() {
            out.extend_from_slice(b", ");
        }
    }
    if let Some(line) = line {
        out.extend_from_slice(format!("line {line}").as_bytes());
    }
    out.push(b')');
    unsafe { from_bytes(&out) }
}

unsafe extern "C" fn os_error_str(op: *mut PyObject) -> *mut PyObject {
    let Some(snapshot) = (unsafe {
        RenderSnapshot::capture_fields(
            op,
            Some(ExceptionLayoutKind::OSError),
            true,
            &[
                ExceptionTypedField::OSErrorErrno,
                ExceptionTypedField::OSErrorStrError,
                ExceptionTypedField::OSErrorFilename,
                ExceptionTypedField::OSErrorFilename2,
                ExceptionTypedField::OSErrorWinError,
            ],
        )
    }) else {
        return ptr::null_mut();
    };
    let filename = snapshot.field(ExceptionTypedField::OSErrorFilename);
    let filename2 = snapshot.field(ExceptionTypedField::OSErrorFilename2);
    let reason = snapshot.field(ExceptionTypedField::OSErrorStrError);
    let errno = snapshot.field(ExceptionTypedField::OSErrorErrno);
    #[cfg(windows)]
    let winerror = snapshot.field(ExceptionTypedField::OSErrorWinError);
    #[cfg(windows)]
    let (label, code) = if !winerror.is_null() && (!filename.is_null() || !reason.is_null()) {
        ("WinError", winerror)
    } else {
        ("Errno", errno)
    };
    #[cfg(not(windows))]
    let (label, code) = ("Errno", errno);
    if filename.is_null() && (code.is_null() || reason.is_null()) {
        return unsafe { snapshot.base_str() };
    }
    let Some(code) = (unsafe { rendered_bytes(or_none(code), false) }) else {
        return ptr::null_mut();
    };
    let Some(reason) = (unsafe { rendered_bytes(or_none(reason), false) }) else {
        return ptr::null_mut();
    };
    let mut out = format!("[{label} ").into_bytes();
    out.extend_from_slice(&code);
    out.extend_from_slice(b"] ");
    out.extend_from_slice(&reason);
    if !filename.is_null() {
        let Some(filename) = (unsafe { rendered_bytes(filename, true) }) else {
            return ptr::null_mut();
        };
        out.extend_from_slice(b": ");
        out.extend_from_slice(&filename);
        if !filename2.is_null() {
            let Some(filename2) = (unsafe { rendered_bytes(filename2, true) }) else {
                return ptr::null_mut();
            };
            out.extend_from_slice(b" -> ");
            out.extend_from_slice(&filename2);
        }
    }
    unsafe { from_bytes(&out) }
}

unsafe fn unicode_error_str(op: *mut PyObject, slot: ExceptionStrSlot) -> *mut PyObject {
    let mut strings = StringResults::default();
    let reason = {
        let Some(snapshot) = (unsafe {
            RenderSnapshot::capture_fields(
                op,
                Some(ExceptionLayoutKind::Unicode),
                false,
                &[ExceptionTypedField::UnicodeReason],
            )
        }) else {
            return ptr::null_mut();
        };
        if !snapshot.unicode_object_present {
            return unsafe { from_bytes(b"") };
        }
        // CPython invokes reason before encoding, including modified field objects.
        let Some(reason) =
            (unsafe { strings.str_bytes(snapshot.field(ExceptionTypedField::UnicodeReason)) })
        else {
            return ptr::null_mut();
        };
        reason
    };
    let mut out = Vec::new();
    if slot != ExceptionStrSlot::UnicodeTranslate {
        let Some(encoding_snapshot) = (unsafe {
            RenderSnapshot::capture_fields(
                op,
                Some(ExceptionLayoutKind::Unicode),
                false,
                &[ExceptionTypedField::UnicodeEncoding],
            )
        }) else {
            return ptr::null_mut();
        };
        let Some(encoding) = (unsafe {
            strings.str_bytes(encoding_snapshot.field(ExceptionTypedField::UnicodeEncoding))
        }) else {
            return ptr::null_mut();
        };
        out.push(b'\'');
        out.extend_from_slice(&encoding);
        out.extend_from_slice(b"' codec ");
    }
    // These reads occur after both callbacks. A reason/encoding callback may
    // replace the input object or positions; pin only the values now observed.
    let Some(snapshot) = (unsafe {
        RenderSnapshot::capture_fields(
            op,
            Some(ExceptionLayoutKind::Unicode),
            false,
            &[ExceptionTypedField::UnicodeObject],
        )
    }) else {
        return ptr::null_mut();
    };
    let value = snapshot.field(ExceptionTypedField::UnicodeObject);
    let start = snapshot.start;
    let end = snapshot.end;
    let single = end.checked_sub(start) == Some(1);
    let unit = if slot == ExceptionStrSlot::UnicodeDecode {
        let mut data = ptr::null_mut();
        let mut length = 0;
        if unsafe { strings::PyBytes_AsStringAndSize(value, &raw mut data, &raw mut length) } != 0 {
            return ptr::null_mut();
        }
        if single && start >= 0 && start < length {
            Some(unsafe { *data.cast::<u8>().offset(start) } as u32)
        } else {
            None
        }
    } else {
        let Some(bytes) = (unsafe { strings::unicode_bytes(value) }) else {
            unsafe { PyErr_BadInternalCall() };
            return ptr::null_mut();
        };
        let Some(text) = strings::PythonStringBytes::from_bytes(bytes) else {
            unsafe { PyErr_BadInternalCall() };
            return ptr::null_mut();
        };
        if single && start >= 0 {
            text.code_points().nth(start as usize)
        } else {
            None
        }
    };
    let operation = match slot {
        ExceptionStrSlot::UnicodeDecode => "decode",
        ExceptionStrSlot::UnicodeEncode => "encode",
        ExceptionStrSlot::UnicodeTranslate => "translate",
        _ => unreachable!("Unicode slot identity"),
    };
    let detail = match unit {
        Some(byte) if slot == ExceptionStrSlot::UnicodeDecode => {
            format!("can't decode byte 0x{byte:02x} in position {start}: ")
        }
        Some(code) => {
            let escaped = if code <= 0xff {
                format!("\\x{code:02x}")
            } else if code <= 0xffff {
                format!("\\u{code:04x}")
            } else {
                format!("\\U{code:08x}")
            };
            format!("can't {operation} character '{escaped}' in position {start}: ")
        }
        None => {
            let units = if slot == ExceptionStrSlot::UnicodeDecode {
                "bytes"
            } else {
                "characters"
            };
            format!(
                "can't {operation} {units} in position {start}-{}: ",
                end.wrapping_sub(1)
            )
        }
    };
    out.extend_from_slice(detail.as_bytes());
    out.extend_from_slice(&reason);
    unsafe { from_bytes(&out) }
}

unsafe extern "C" fn unicode_decode_error_str(op: *mut PyObject) -> *mut PyObject {
    unsafe { unicode_error_str(op, ExceptionStrSlot::UnicodeDecode) }
}

unsafe extern "C" fn unicode_encode_error_str(op: *mut PyObject) -> *mut PyObject {
    unsafe { unicode_error_str(op, ExceptionStrSlot::UnicodeEncode) }
}

unsafe extern "C" fn unicode_translate_error_str(op: *mut PyObject) -> *mut PyObject {
    unsafe { unicode_error_str(op, ExceptionStrSlot::UnicodeTranslate) }
}
