//! Memoryview API backed by the canonical buffer export path.

use crate::abi_types::{Py_buffer, PyMemoryView_Type, PyMemoryViewObject, PyObject};
use std::ptr;

const PYBUF_FULL_RO: std::os::raw::c_int = 0x0004 | 0x0008 | 0x0010;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_Check(op: *mut PyObject) -> std::os::raw::c_int {
    if op.is_null() {
        return 0;
    }
    std::ptr::eq(unsafe { (*op).ob_type }, &raw mut PyMemoryView_Type) as std::os::raw::c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_FromObject(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        return ptr::null_mut();
    }

    let mut view_obj = Box::new(PyMemoryViewObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut PyMemoryView_Type,
        },
        view: unsafe { std::mem::zeroed() },
    });
    let view_ptr = &mut view_obj.view as *mut Py_buffer;
    if unsafe { crate::api::buffer::PyObject_GetBuffer(op, view_ptr, PYBUF_FULL_RO) } != 0 {
        return ptr::null_mut();
    }
    Box::into_raw(view_obj).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_GET_BASE(op: *mut PyObject) -> *mut PyObject {
    let view = unsafe { PyMemoryView_GET_BUFFER(op) };
    if view.is_null() {
        ptr::null_mut()
    } else {
        unsafe { (*view).obj }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMemoryView_GET_BUFFER(op: *mut PyObject) -> *mut Py_buffer {
    if unsafe { PyMemoryView_Check(op) } == 0 {
        return ptr::null_mut();
    }
    unsafe { &mut (*(op.cast::<PyMemoryViewObject>())).view }
}

pub unsafe extern "C" fn molt_memoryview_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let mut owned = unsafe { Box::from_raw(op.cast::<PyMemoryViewObject>()) };
    unsafe { crate::api::buffer::PyBuffer_Release(&mut owned.view) };
}
