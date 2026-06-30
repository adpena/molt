//! Slice object ABI - PySlice_New, checks, and index normalization.

use crate::abi_types::{Py_None, Py_ssize_t, PyObject, PySliceObject};
use std::os::raw::c_int;

pub unsafe extern "C" fn molt_slice_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let slice = op.cast::<PySliceObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*slice).start);
        crate::api::refcount::Py_XDECREF((*slice).stop);
        crate::api::refcount::Py_XDECREF((*slice).step);
        drop(Box::from_raw(slice));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_New(
    start: *mut PyObject,
    stop: *mut PyObject,
    step: *mut PyObject,
) -> *mut PyObject {
    let start = if start.is_null() {
        &raw mut Py_None
    } else {
        start
    };
    let stop = if stop.is_null() {
        &raw mut Py_None
    } else {
        stop
    };
    let step = if step.is_null() {
        &raw mut Py_None
    } else {
        step
    };
    unsafe {
        crate::api::refcount::Py_INCREF(start);
        crate::api::refcount::Py_INCREF(stop);
        crate::api::refcount::Py_INCREF(step);
    }
    let slice = Box::new(PySliceObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PySlice_Type,
        },
        start,
        stop,
        step,
    });
    Box::into_raw(slice).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    std::ptr::eq(ob_type, &raw mut crate::abi_types::PySlice_Type) as c_int
}

unsafe fn slice_index_value(op: *mut PyObject) -> Py_ssize_t {
    unsafe { crate::api::numbers::PyLong_AsSsize_t(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_Unpack(
    slice: *mut PyObject,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: *mut Py_ssize_t,
) -> c_int {
    if slice.is_null() || start.is_null() || stop.is_null() || step.is_null() {
        return -1;
    }
    if unsafe { PySlice_Check(slice) } == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"PySlice_Unpack requires a slice object".as_ptr(),
            );
        }
        return -1;
    }
    let layout = slice.cast::<PySliceObject>();
    let raw_step = unsafe { (*layout).step };
    let parsed_step = if std::ptr::eq(raw_step, &raw mut Py_None) {
        1
    } else {
        unsafe { slice_index_value(raw_step) }
    };
    if parsed_step == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_ValueError,
                c"slice step cannot be zero".as_ptr(),
            );
        }
        return -1;
    }
    let normalized_step = parsed_step.max(-Py_ssize_t::MAX);
    let raw_start = unsafe { (*layout).start };
    let raw_stop = unsafe { (*layout).stop };
    unsafe {
        *step = normalized_step;
        *start = if std::ptr::eq(raw_start, &raw mut Py_None) {
            if normalized_step < 0 {
                Py_ssize_t::MAX
            } else {
                0
            }
        } else {
            slice_index_value(raw_start)
        };
        *stop = if std::ptr::eq(raw_stop, &raw mut Py_None) {
            if normalized_step < 0 {
                Py_ssize_t::MIN
            } else {
                Py_ssize_t::MAX
            }
        } else {
            slice_index_value(raw_stop)
        };
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_AdjustIndices(
    length: Py_ssize_t,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: Py_ssize_t,
) -> Py_ssize_t {
    if start.is_null() || stop.is_null() || step == 0 {
        return 0;
    }
    let length = length.max(0);
    unsafe {
        if *start < 0 {
            *start += length;
            if *start < 0 {
                *start = if step < 0 { -1 } else { 0 };
            }
        } else if *start >= length {
            *start = if step < 0 { length - 1 } else { length };
        }

        if *stop < 0 {
            *stop += length;
            if *stop < 0 {
                *stop = if step < 0 { -1 } else { 0 };
            }
        } else if *stop >= length {
            *stop = if step < 0 { length - 1 } else { length };
        }

        if step < 0 {
            if *stop < *start {
                (*start - *stop - 1) / (-step) + 1
            } else {
                0
            }
        } else if *start < *stop {
            (*stop - *start - 1) / step + 1
        } else {
            0
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_GetIndicesEx(
    slice: *mut PyObject,
    length: Py_ssize_t,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: *mut Py_ssize_t,
    slicelength: *mut Py_ssize_t,
) -> c_int {
    if slicelength.is_null() {
        return -1;
    }
    if unsafe { PySlice_Unpack(slice, start, stop, step) } < 0 {
        unsafe {
            *slicelength = 0;
        }
        return -1;
    }
    unsafe {
        *slicelength = PySlice_AdjustIndices(length, start, stop, *step);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySlice_GetIndices(
    slice: *mut PyObject,
    length: Py_ssize_t,
    start: *mut Py_ssize_t,
    stop: *mut Py_ssize_t,
    step: *mut Py_ssize_t,
) -> c_int {
    let mut slicelength = 0;
    unsafe { PySlice_GetIndicesEx(slice, length, start, stop, step, &raw mut slicelength) }
}
