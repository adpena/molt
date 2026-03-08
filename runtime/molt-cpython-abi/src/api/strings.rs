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
