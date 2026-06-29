//! Buffer protocol entrypoints backed by the runtime-owned typed strided export.

use crate::abi_types::{Py_buffer, PyExc_BufferError, PyExc_TypeError, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::{MoltBufferView, hooks_or_stubs};
use std::os::raw::{c_char, c_int};
use std::ptr;

const PYBUF_WRITABLE: c_int = 0x0001;
const PYBUF_FORMAT: c_int = 0x0004;
const PYBUF_ND: c_int = 0x0008;
const PYBUF_STRIDES: c_int = 0x0010 | PYBUF_ND;
const PYBUF_C_CONTIGUOUS: c_int = 0x0020;
const PYBUF_F_CONTIGUOUS: c_int = 0x0040;
const PYBUF_ANY_CONTIGUOUS: c_int = 0x0080;

unsafe fn set_buffer_error(message: &'static [u8]) {
    unsafe {
        crate::api::errors::PyErr_SetString(&raw mut PyExc_BufferError, message.as_ptr().cast());
    }
}

unsafe fn set_type_error(message: &'static [u8]) {
    unsafe {
        crate::api::errors::PyErr_SetString(&raw mut PyExc_TypeError, message.as_ptr().cast());
    }
}

unsafe fn pybuffer_is_c_contiguous(view: *const Py_buffer) -> bool {
    if view.is_null() || unsafe { (*view).ndim } <= 1 {
        return true;
    }
    if unsafe { (*view).shape.is_null() || (*view).strides.is_null() } {
        return true;
    }
    let ndim = unsafe { (*view).ndim as usize };
    let mut expected = unsafe { (*view).itemsize.max(1) };
    for i in (0..ndim).rev() {
        let dim = unsafe { *(*view).shape.add(i) };
        let stride = unsafe { *(*view).strides.add(i) };
        if dim > 1 && stride != expected {
            return false;
        }
        expected = expected.saturating_mul(dim.max(1));
    }
    true
}

unsafe fn pybuffer_is_f_contiguous(view: *const Py_buffer) -> bool {
    if view.is_null() || unsafe { (*view).ndim } <= 1 {
        return true;
    }
    if unsafe { (*view).shape.is_null() || (*view).strides.is_null() } {
        return true;
    }
    let ndim = unsafe { (*view).ndim as usize };
    let mut expected = unsafe { (*view).itemsize.max(1) };
    for i in 0..ndim {
        let dim = unsafe { *(*view).shape.add(i) };
        let stride = unsafe { *(*view).strides.add(i) };
        if dim > 1 && stride != expected {
            return false;
        }
        expected = expected.saturating_mul(dim.max(1));
    }
    true
}

unsafe fn pybuffer_satisfies_flags(view: *const Py_buffer, flags: c_int) -> bool {
    if (flags & PYBUF_C_CONTIGUOUS) != 0 && !unsafe { pybuffer_is_c_contiguous(view) } {
        return false;
    }
    if (flags & PYBUF_F_CONTIGUOUS) != 0 && !unsafe { pybuffer_is_f_contiguous(view) } {
        return false;
    }
    if (flags & PYBUF_ANY_CONTIGUOUS) != 0
        && !unsafe { pybuffer_is_c_contiguous(view) }
        && !unsafe { pybuffer_is_f_contiguous(view) }
    {
        return false;
    }
    true
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_GetBuffer(
    obj: *mut PyObject,
    view: *mut Py_buffer,
    flags: c_int,
) -> c_int {
    if view.is_null() {
        unsafe { set_type_error(b"buffer view must not be NULL\0") };
        return -1;
    }
    unsafe {
        ptr::write_bytes(view, 0, 1);
        (*view).itemsize = 1;
        (*view).readonly = 1;
    }
    if obj.is_null() {
        unsafe { set_type_error(b"buffer exporter must not be NULL\0") };
        return -1;
    }
    let bits = match GLOBAL_BRIDGE.lock().pyobj_to_handle(obj) {
        Some(bits) => bits,
        None => {
            unsafe { set_buffer_error(b"object has no Molt buffer handle\0") };
            return -1;
        }
    };
    let hooks = hooks_or_stubs();
    let mut owned = Box::new(MoltBufferView::default());
    if unsafe { (hooks.buffer_acquire)(bits, owned.as_mut() as *mut MoltBufferView) } != 0 {
        unsafe { set_buffer_error(b"object does not export a buffer\0") };
        return -1;
    }
    if (flags & PYBUF_WRITABLE) != 0 && owned.readonly != 0 {
        unsafe {
            let _ = (hooks.buffer_release)(owned.as_mut() as *mut MoltBufferView);
            set_buffer_error(b"writable buffer requested for readonly object\0");
        }
        return -1;
    }
    unsafe {
        (*view).buf = owned.data.cast();
        (*view).obj = obj;
        (*view).len = owned.len as isize;
        (*view).itemsize = owned.itemsize as isize;
        (*view).readonly = owned.readonly as c_int;
        (*view).ndim = owned.ndim as c_int;
        (*view).format = if (flags & PYBUF_FORMAT) != 0 {
            owned.format.as_mut_ptr().cast::<c_char>()
        } else {
            ptr::null_mut()
        };
        (*view).shape = if (flags & (PYBUF_ND | PYBUF_STRIDES)) != 0 {
            owned.shape.as_mut_ptr()
        } else {
            ptr::null_mut()
        };
        (*view).strides = if (flags & PYBUF_STRIDES) != 0 {
            owned.strides.as_mut_ptr()
        } else {
            ptr::null_mut()
        };
        (*view).suboffsets = ptr::null_mut();
        (*view).internal = Box::into_raw(owned).cast();
        crate::api::refcount::Py_INCREF(obj);
        if !pybuffer_satisfies_flags(view, flags) {
            PyBuffer_Release(view);
            set_buffer_error(b"requested contiguous buffer is not available\0");
            return -1;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_Release(view: *mut Py_buffer) {
    if view.is_null() {
        return;
    }
    unsafe {
        if !(*view).internal.is_null() {
            let mut owned = Box::from_raw((*view).internal.cast::<MoltBufferView>());
            let _ = (hooks_or_stubs().buffer_release)(owned.as_mut() as *mut MoltBufferView);
            (*view).internal = ptr::null_mut();
        }
        if !(*view).obj.is_null() {
            crate::api::refcount::Py_DECREF((*view).obj);
            (*view).obj = ptr::null_mut();
        }
        (*view).buf = ptr::null_mut();
        (*view).len = 0;
        (*view).itemsize = 1;
        (*view).readonly = 1;
        (*view).ndim = 0;
        (*view).format = ptr::null_mut();
        (*view).shape = ptr::null_mut();
        (*view).strides = ptr::null_mut();
        (*view).suboffsets = ptr::null_mut();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CheckBuffer(obj: *mut PyObject) -> c_int {
    if obj.is_null() {
        return 0;
    }
    let bits = match GLOBAL_BRIDGE.lock().pyobj_to_handle(obj) {
        Some(bits) => bits,
        None => return 0,
    };
    let hooks = hooks_or_stubs();
    let mut view = MoltBufferView::default();
    if unsafe { (hooks.buffer_acquire)(bits, &mut view as *mut MoltBufferView) } == 0 {
        unsafe {
            let _ = (hooks.buffer_release)(&mut view as *mut MoltBufferView);
        }
        1
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBuffer_IsContiguous(
    view: *const Py_buffer,
    order: std::os::raw::c_char,
) -> c_int {
    if view.is_null() {
        return 0;
    }
    match order as u8 {
        b'C' | b'c' => unsafe { pybuffer_is_c_contiguous(view) as c_int },
        b'F' | b'f' => unsafe { pybuffer_is_f_contiguous(view) as c_int },
        _ => unsafe { (pybuffer_is_c_contiguous(view) || pybuffer_is_f_contiguous(view)) as c_int },
    }
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
