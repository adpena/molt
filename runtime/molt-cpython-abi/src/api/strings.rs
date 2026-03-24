//! String API — PyUnicode_*, PyBytes_*.

use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

// ─── PyUnicode ────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromString(s: *const c_char) -> *mut PyObject {
    if s.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(s).to_bytes() };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(bytes.as_ptr(), bytes.len()) };
    if bits == 0 {
        // Fallback: return a placeholder None handle so the caller doesn't crash.
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromStringAndSize(
    s: *const c_char,
    size: Py_ssize_t,
) -> *mut PyObject {
    if s.is_null() || size < 0 {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), size as usize) };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(bytes.as_ptr(), bytes.len()) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8(op: *mut PyObject) -> *const c_char {
    if op.is_null() {
        return ptr::null();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return c"".as_ptr(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.str_data)(bits, &raw mut len) };
    if ptr.is_null() {
        c"".as_ptr()
    } else {
        ptr.cast()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8AndSize(
    op: *mut PyObject,
    size: *mut Py_ssize_t,
) -> *const c_char {
    let ptr = unsafe { PyUnicode_AsUTF8(op) };
    if !size.is_null() && !ptr.is_null() {
        let len = unsafe { CStr::from_ptr(ptr) }.to_bytes().len();
        unsafe {
            *size = len as Py_ssize_t;
        }
    }
    ptr
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_GetLength(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.str_data)(bits, &raw mut len) };
    if ptr.is_null() { -1 } else { len as Py_ssize_t }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PyUnicode_Type)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_CompareWithASCIIString(
    op: *mut PyObject,
    s: *const c_char,
) -> c_int {
    let obj_ptr = unsafe { PyUnicode_AsUTF8(op) };
    if obj_ptr.is_null() || s.is_null() {
        return -1;
    }
    unsafe {
        let a = CStr::from_ptr(obj_ptr).to_bytes();
        let b = CStr::from_ptr(s).to_bytes();
        a.cmp(b) as c_int
    }
}

// ─── PyBytes ──────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_FromStringAndSize(
    s: *const c_char,
    len: Py_ssize_t,
) -> *mut PyObject {
    if len < 0 {
        return ptr::null_mut();
    }
    let data = if s.is_null() {
        vec![0u8; len as usize]
    } else {
        unsafe { std::slice::from_raw_parts(s.cast::<u8>(), len as usize).to_vec() }
    };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_bytes)(data.as_ptr(), data.len()) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_FromString(s: *const c_char) -> *mut PyObject {
    if s.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(s).to_bytes() };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_bytes)(bytes.as_ptr(), bytes.len()) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AsStringAndSize(
    op: *mut PyObject,
    buf: *mut *mut c_char,
    length: *mut Py_ssize_t,
) -> c_int {
    if op.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if ptr.is_null() {
        if !buf.is_null() {
            unsafe {
                *buf = ptr::null_mut();
            }
        }
        if !length.is_null() {
            unsafe {
                *length = 0;
            }
        }
        return -1;
    }
    if !buf.is_null() {
        unsafe {
            *buf = ptr as *mut c_char;
        }
    }
    if !length.is_null() {
        unsafe {
            *length = len as Py_ssize_t;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PyBytes_Type)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AS_STRING(op: *mut PyObject) -> *const c_char {
    if op.is_null() {
        return ptr::null();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let data = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if data.is_null() {
        ptr::null()
    } else {
        data.cast()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyBytes_Size(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Size(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let mut len: usize = 0;
    let ptr = unsafe { (h.bytes_data)(bits, &raw mut len) };
    if ptr.is_null() { -1 } else { len as Py_ssize_t }
}

// ─── Additional PyUnicode functions ──────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Concat(
    left: *mut PyObject,
    right: *mut PyObject,
) -> *mut PyObject {
    if left.is_null() || right.is_null() {
        return ptr::null_mut();
    }
    let left_ptr = unsafe { PyUnicode_AsUTF8(left) };
    let right_ptr = unsafe { PyUnicode_AsUTF8(right) };
    if left_ptr.is_null() || right_ptr.is_null() {
        return ptr::null_mut();
    }
    let left_s = unsafe { CStr::from_ptr(left_ptr).to_bytes() };
    let right_s = unsafe { CStr::from_ptr(right_ptr).to_bytes() };
    let mut combined = Vec::with_capacity(left_s.len() + right_s.len());
    combined.extend_from_slice(left_s);
    combined.extend_from_slice(right_s);
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_str)(combined.as_ptr(), combined.len()) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Join(
    separator: *mut PyObject,
    seq: *mut PyObject,
) -> *mut PyObject {
    // Minimal: return empty string — full join requires iterating seq.
    let _ = (separator, seq);
    unsafe { PyUnicode_FromString(c"".as_ptr()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Contains(
    container: *mut PyObject,
    element: *mut PyObject,
) -> c_int {
    if container.is_null() || element.is_null() {
        return -1;
    }
    let c_ptr = unsafe { PyUnicode_AsUTF8(container) };
    let e_ptr = unsafe { PyUnicode_AsUTF8(element) };
    if c_ptr.is_null() || e_ptr.is_null() {
        return -1;
    }
    let c_bytes = unsafe { CStr::from_ptr(c_ptr).to_bytes() };
    let e_bytes = unsafe { CStr::from_ptr(e_ptr).to_bytes() };
    // Substring search.
    if e_bytes.is_empty() {
        return 1;
    }
    for window in c_bytes.windows(e_bytes.len()) {
        if window == e_bytes {
            return 1;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Decode(
    s: *const c_char,
    size: Py_ssize_t,
    _encoding: *const c_char,
    _errors: *const c_char,
) -> *mut PyObject {
    // Assume UTF-8 encoding — the common case.
    unsafe { PyUnicode_FromStringAndSize(s, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_DecodeUTF8(
    s: *const c_char,
    size: Py_ssize_t,
    _errors: *const c_char,
) -> *mut PyObject {
    unsafe { PyUnicode_FromStringAndSize(s, size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsEncodedString(
    unicode: *mut PyObject,
    _encoding: *const c_char,
    _errors: *const c_char,
) -> *mut PyObject {
    // Assume UTF-8 — return a bytes object with the UTF-8 data.
    if unicode.is_null() {
        return ptr::null_mut();
    }
    let utf8_ptr = unsafe { PyUnicode_AsUTF8(unicode) };
    if utf8_ptr.is_null() {
        return ptr::null_mut();
    }
    let utf8_len = unsafe { CStr::from_ptr(utf8_ptr) }.to_bytes().len();
    unsafe { PyBytes_FromStringAndSize(utf8_ptr, utf8_len as Py_ssize_t) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_InternInPlace(_p: *mut *mut PyObject) {
    // Interning is a no-op in the bridge — strings are already de-duped by
    // Molt's string allocator when hooks are active.
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_InternFromString(s: *const c_char) -> *mut PyObject {
    unsafe { PyUnicode_FromString(s) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Format(
    format: *mut PyObject,
    _args: *mut PyObject,
) -> *mut PyObject {
    // Minimal stub — %-formatting requires full parser.
    // Return the format string unchanged.
    if format.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_INCREF(format) };
    format
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_GET_LENGTH(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyUnicode_GetLength(op) }
}

// ─── Additional PyBytes functions ────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Concat(bytes: *mut *mut PyObject, newpart: *mut PyObject) {
    if bytes.is_null() || unsafe { *bytes }.is_null() || newpart.is_null() {
        return;
    }
    // Simplified: just keep the original bytes.
    // Full concat requires bytes_data + alloc_bytes.
    let _ = newpart;
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_FromStringAndSize(
    s: *const c_char,
    len: Py_ssize_t,
) -> *mut PyObject {
    // Bytearray is backed by bytes for now.
    unsafe { PyBytes_FromStringAndSize(s, len) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyByteArray_Check(_op: *mut PyObject) -> c_int {
    // No dedicated bytearray type yet.
    0
}
