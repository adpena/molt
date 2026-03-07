//! Sequence API — PyList_*, PyTuple_*.

use crate::abi_types::{PyObject, Py_ssize_t};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;
#[allow(unused_imports)]
use crate::abi_types::{PyList_Type, PyTuple_Type};

// ─── PyList ───────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_New(size: Py_ssize_t) -> *mut PyObject {
    let _ = size;
    // Allocate a Molt list of given initial size.
    // TODO: wire to molt runtime list allocator via c_api.
    let bits = MoltObject::none().bits(); // placeholder
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Append(list: *mut PyObject, item: *mut PyObject) -> c_int {
    if list.is_null() || item.is_null() { return -1; }
    // TODO: wire to molt runtime list append.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GET_ITEM(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    if op.is_null() { return ptr::null_mut(); }
    // TODO: return borrowed ref to list item.
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    unsafe { PyList_GET_ITEM(op, i) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_SET_ITEM(op: *mut PyObject, i: Py_ssize_t, v: *mut PyObject) {
    let _ = (op, i, v);
    // TODO: wire to molt runtime list set.
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_SetItem(
    op: *mut PyObject,
    i: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    unsafe { PyList_SET_ITEM(op, i, v) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() { return 0; }
    // TODO: return list length.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Size(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyList_GET_SIZE(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Check(op: *mut PyObject) -> c_int {
    if op.is_null() { return 0; }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, unsafe { &raw const crate::abi_types::PyList_Type })) as c_int
}

// ─── PyTuple ──────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_New(size: Py_ssize_t) -> *mut PyObject {
    let _ = size;
    let bits = MoltObject::none().bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GET_ITEM(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    let _ = (op, i);
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    unsafe { PyTuple_GET_ITEM(op, i) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
    let _ = op;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Size(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyTuple_GET_SIZE(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_SetItem(
    op: *mut PyObject,
    i: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    let _ = (op, i, v);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Check(op: *mut PyObject) -> c_int {
    if op.is_null() { return 0; }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, unsafe { &raw const crate::abi_types::PyTuple_Type })) as c_int
}
