//! Buffer protocol entrypoints backed by the runtime-owned typed strided export.

use crate::abi_types::{
    Py_buffer, PyBUF_ANY_CONTIGUOUS, PyBUF_C_CONTIGUOUS, PyBUF_F_CONTIGUOUS, PyBUF_FORMAT,
    PyBUF_ND, PyBUF_SIMPLE, PyBUF_STRIDES, PyBUF_WRITABLE, PyExc_BufferError, PyExc_TypeError,
    PyObject,
};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::{MoltBufferView, hooks_or_stubs};
use std::os::raw::{c_char, c_int};
use std::ptr;

const PYBUF_C_CONTIGUOUS_BIT: c_int = PyBUF_C_CONTIGUOUS & !PyBUF_STRIDES;
const PYBUF_F_CONTIGUOUS_BIT: c_int = PyBUF_F_CONTIGUOUS & !PyBUF_STRIDES;
const PYBUF_ANY_CONTIGUOUS_BIT: c_int = PyBUF_ANY_CONTIGUOUS & !PyBUF_STRIDES;

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum BufferReleaseKind {
    Runtime,
    Raw,
}

struct BufferInternal {
    release_kind: BufferReleaseKind,
    descriptor: MoltBufferView,
}

impl BufferInternal {
    fn runtime(descriptor: MoltBufferView) -> Self {
        Self {
            release_kind: BufferReleaseKind::Runtime,
            descriptor,
        }
    }

    fn raw_1d(buf: *mut std::ffi::c_void, len: isize, readonly: c_int, base: u64) -> Self {
        let mut descriptor = MoltBufferView::default();
        descriptor.data = buf.cast();
        descriptor.len = len as u64;
        descriptor.readonly = u32::from(readonly != 0);
        descriptor.ndim = 1;
        descriptor.itemsize = 1;
        descriptor.base = base;
        descriptor.shape[0] = len;
        descriptor.strides[0] = 1;
        descriptor.format[0] = b'B';
        descriptor.format[1] = 0;
        Self {
            release_kind: BufferReleaseKind::Raw,
            descriptor,
        }
    }
}

unsafe fn reset_pybuffer(view: *mut Py_buffer) {
    unsafe {
        ptr::write_bytes(view, 0, 1);
        (*view).itemsize = 1;
        (*view).readonly = 1;
    }
}

unsafe fn apply_molt_view(
    view: *mut Py_buffer,
    obj: *mut PyObject,
    descriptor: &mut MoltBufferView,
    flags: c_int,
) {
    unsafe {
        (*view).buf = descriptor.data.cast();
        (*view).obj = obj;
        (*view).len = descriptor.len as isize;
        (*view).itemsize = descriptor.itemsize as isize;
        (*view).readonly = descriptor.readonly as c_int;
        (*view).ndim = descriptor.ndim as c_int;
        (*view).format = if (flags & PyBUF_FORMAT) != 0 {
            descriptor.format.as_mut_ptr().cast::<c_char>()
        } else {
            ptr::null_mut()
        };
        (*view).shape = if (flags & (PyBUF_ND | PyBUF_STRIDES)) != 0 {
            descriptor.shape.as_mut_ptr()
        } else {
            ptr::null_mut()
        };
        (*view).strides = if (flags & PyBUF_STRIDES) != 0 {
            descriptor.strides.as_mut_ptr()
        } else {
            ptr::null_mut()
        };
        (*view).suboffsets = ptr::null_mut();
    }
}

unsafe fn install_buffer_internal(
    view: *mut Py_buffer,
    obj: *mut PyObject,
    mut internal: Box<BufferInternal>,
    flags: c_int,
) -> c_int {
    unsafe {
        apply_molt_view(view, obj, &mut internal.descriptor, flags);
        (*view).internal = Box::into_raw(internal).cast();
        if !obj.is_null() {
            crate::api::refcount::Py_INCREF(obj);
        }
        if !pybuffer_satisfies_flags(view, flags) {
            PyBuffer_Release(view);
            set_buffer_error(b"requested contiguous buffer is not available\0");
            return -1;
        }
    }
    0
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
    if (flags & PYBUF_C_CONTIGUOUS_BIT) != 0 && !unsafe { pybuffer_is_c_contiguous(view) } {
        return false;
    }
    if (flags & PYBUF_F_CONTIGUOUS_BIT) != 0 && !unsafe { pybuffer_is_f_contiguous(view) } {
        return false;
    }
    if (flags & PYBUF_ANY_CONTIGUOUS_BIT) != 0
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
    unsafe { reset_pybuffer(view) };
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
    if (flags & PyBUF_WRITABLE) != 0 && owned.readonly != 0 {
        unsafe {
            let _ = (hooks.buffer_release)(owned.as_mut() as *mut MoltBufferView);
            set_buffer_error(b"writable buffer requested for readonly object\0");
        }
        return -1;
    }
    if unsafe {
        install_buffer_internal(view, obj, Box::new(BufferInternal::runtime(*owned)), flags)
    } != 0
    {
        return -1;
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
            let mut internal = Box::from_raw((*view).internal.cast::<BufferInternal>());
            if internal.release_kind == BufferReleaseKind::Runtime {
                let _ = (hooks_or_stubs().buffer_release)(
                    &mut internal.descriptor as *mut MoltBufferView,
                );
            }
            (*view).internal = ptr::null_mut();
        }
        if !(*view).obj.is_null() {
            crate::api::refcount::Py_DECREF((*view).obj);
            (*view).obj = ptr::null_mut();
        }
        reset_pybuffer(view);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_CheckBuffer(obj: *mut PyObject) -> c_int {
    if obj.is_null() {
        return 0;
    }
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    if unsafe { PyObject_GetBuffer(obj, &mut view, PyBUF_SIMPLE) } == 0 {
        unsafe { PyBuffer_Release(&mut view) };
        return 1;
    }
    unsafe { crate::api::errors::PyErr_Clear() };
    0
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
    flags: c_int,
) -> c_int {
    if view.is_null() {
        return -1;
    }
    if len < 0 {
        unsafe { set_buffer_error(b"buffer length must not be negative\0") };
        return -1;
    }
    if (flags & PyBUF_WRITABLE) != 0 && readonly != 0 {
        unsafe { set_buffer_error(b"writable buffer requested for readonly object\0") };
        return -1;
    }
    let base = if obj.is_null() {
        0
    } else {
        match GLOBAL_BRIDGE.lock().pyobj_to_handle(obj) {
            Some(bits) => bits,
            None => 0,
        }
    };
    unsafe {
        reset_pybuffer(view);
        install_buffer_internal(
            view,
            obj,
            Box::new(BufferInternal::raw_1d(buf, len, readonly, base)),
            flags,
        )
    }
}
