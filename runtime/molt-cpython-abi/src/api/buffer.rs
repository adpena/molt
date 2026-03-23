//! Buffer protocol stubs — PyObject_GetBuffer, PyBuffer_Release, etc.
//!
//! Full buffer protocol support requires deep integration with the Molt
//! memory model. These stubs ensure extensions that check for buffer
//! support don't crash.

use crate::abi_types::{Py_buffer, PyObject};
use std::os::raw::c_int;
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetBuffer(
    obj: *mut PyObject,
    view: *mut Py_buffer,
    flags: c_int,
) -> c_int {
    let _ = (obj, flags);
    if !view.is_null() {
        unsafe {
            (*view).buf = ptr::null_mut();
            (*view).obj = ptr::null_mut();
            (*view).len = 0;
            (*view).itemsize = 1;
            (*view).readonly = 1;
            (*view).ndim = 0;
            (*view).format = ptr::null_mut();
            (*view).shape = ptr::null_mut();
            (*view).strides = ptr::null_mut();
            (*view).suboffsets = ptr::null_mut();
            (*view).internal = ptr::null_mut();
        }
    }
    // Return -1 to indicate that this object does not support the buffer protocol.
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_Release(view: *mut Py_buffer) {
    if view.is_null() {
        return;
    }
    unsafe {
        if !(*view).obj.is_null() {
            crate::api::refcount::Py_DECREF((*view).obj);
            (*view).obj = ptr::null_mut();
        }
        (*view).buf = ptr::null_mut();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CheckBuffer(obj: *mut PyObject) -> c_int {
    if obj.is_null() {
        return 0;
    }
    // Check if the type has tp_as_buffer set.
    let tp = unsafe { (*obj).ob_type };
    if tp.is_null() {
        return 0;
    }
    (!unsafe { (*tp).tp_as_buffer }.is_null()) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_IsContiguous(
    _view: *const Py_buffer,
    _order: std::os::raw::c_char,
) -> c_int {
    // Default: yes, contiguous (most common case).
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_FillInfo(
    view: *mut Py_buffer,
    obj: *mut PyObject,
    buf: *mut std::ffi::c_void,
    len: isize,
    readonly: c_int,
    _flags: c_int,
) -> c_int {
    if view.is_null() {
        return -1;
    }
    unsafe {
        (*view).buf = buf;
        (*view).obj = obj;
        (*view).len = len;
        (*view).itemsize = 1;
        (*view).readonly = readonly;
        (*view).ndim = 1;
        (*view).format = ptr::null_mut();
        (*view).shape = ptr::null_mut();
        (*view).strides = ptr::null_mut();
        (*view).suboffsets = ptr::null_mut();
        (*view).internal = ptr::null_mut();
    }
    if !obj.is_null() {
        unsafe { crate::api::refcount::Py_INCREF(obj) };
    }
    0
}
