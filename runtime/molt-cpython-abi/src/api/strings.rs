//! String API — PyUnicode_*, PyBytes_*.

use crate::abi_types::{PyObject, Py_ssize_t};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::ptr;

// ─── PyUnicode ────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromString(s: *const c_char) -> *mut PyObject {
    if s.is_null() { return ptr::null_mut(); }
    let bytes = unsafe { CStr::from_ptr(s).to_bytes() };
    // TODO: allocate a Molt str object via runtime hook; for now use a placeholder handle.
    let bits = MoltObject::none().bits();
    let _ = bytes; // consumed by TODO above
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_FromStringAndSize(
    s: *const c_char,
    size: Py_ssize_t,
) -> *mut PyObject {
    if s.is_null() || size < 0 { return ptr::null_mut(); }
    let _bytes = unsafe { std::slice::from_raw_parts(s.cast::<u8>(), size as usize) };
    // TODO: allocate Molt str via runtime hook.
    let bits = MoltObject::none().bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8(op: *mut PyObject) -> *const c_char {
    if op.is_null() { return ptr::null(); }
    // Returns a pointer into the Molt string's internal storage.
    // TODO: wire to runtime string storage accessor.
    b"\0".as_ptr().cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_AsUTF8AndSize(
    op: *mut PyObject,
    size: *mut Py_ssize_t,
) -> *const c_char {
    let ptr = unsafe { PyUnicode_AsUTF8(op) };
    if !size.is_null() && !ptr.is_null() {
        unsafe { *size = unsafe { CStr::from_ptr(ptr) }.to_bytes().len() as Py_ssize_t; }
    }
    ptr
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_GetLength(op: *mut PyObject) -> Py_ssize_t {
    let s = unsafe { PyUnicode_AsUTF8(op) };
    if s.is_null() { return -1; }
    unsafe { CStr::from_ptr(s) }.to_bytes().len() as Py_ssize_t
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_Check(op: *mut PyObject) -> c_int {
    if op.is_null() { return 0; }
    // Type check via ob_type pointer — matches CPython extension convention.
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, unsafe { &raw const crate::abi_types::PyUnicode_Type })) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnicode_CompareWithASCIIString(
    op: *mut PyObject,
    s: *const c_char,
) -> c_int {
    let obj_ptr = unsafe { PyUnicode_AsUTF8(op) };
    if obj_ptr.is_null() || s.is_null() { return -1; }
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
    if len < 0 { return ptr::null_mut(); }
    let _data = if s.is_null() {
        vec![0u8; len as usize]
    } else {
        unsafe { std::slice::from_raw_parts(s.cast::<u8>(), len as usize).to_vec() }
    };
    // TODO: allocate Molt bytes object via runtime hook.
    let bits = MoltObject::none().bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_FromString(s: *const c_char) -> *mut PyObject {
    if s.is_null() { return ptr::null_mut(); }
    let _bytes = unsafe { CStr::from_ptr(s).to_bytes() };
    // TODO: allocate Molt bytes object via runtime hook.
    let bits = MoltObject::none().bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_AsStringAndSize(
    op: *mut PyObject,
    buf: *mut *mut c_char,
    length: *mut Py_ssize_t,
) -> c_int {
    // TODO: wire to Molt bytes storage
    if !buf.is_null() { unsafe { *buf = ptr::null_mut(); } }
    if !length.is_null() { unsafe { *length = 0; } }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBytes_Check(op: *mut PyObject) -> c_int {
    if op.is_null() { return 0; }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, unsafe { &raw const crate::abi_types::PyBytes_Type })) as c_int
}
