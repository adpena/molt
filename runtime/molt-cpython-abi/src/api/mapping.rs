//! Mapping API — PyDict_*.

use crate::abi_types::{PyObject, Py_ssize_t};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_New() -> *mut PyObject {
    let bits = MoltObject::none().bits(); // TODO: allocate Molt dict
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_SetItem(
    op: *mut PyObject,
    key: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    if op.is_null() || key.is_null() || value.is_null() { return -1; }
    // TODO: wire to molt runtime dict set
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_SetItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
    value: *mut PyObject,
) -> c_int {
    if op.is_null() || key.is_null() || value.is_null() { return -1; }
    // TODO: set string-keyed item
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItem(
    op: *mut PyObject,
    key: *mut PyObject,
) -> *mut PyObject {
    if op.is_null() || key.is_null() { return ptr::null_mut(); }
    ptr::null_mut() // TODO: lookup
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> *mut PyObject {
    let _ = (op, key);
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_DelItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> c_int {
    let _ = (op, key);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Size(op: *mut PyObject) -> Py_ssize_t {
    let _ = op;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Check(op: *mut PyObject) -> c_int {
    if op.is_null() { return 0; }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, unsafe { &raw const crate::abi_types::PyDict_Type })) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Copy(op: *mut PyObject) -> *mut PyObject {
    let _ = op;
    unsafe { PyDict_New() }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Keys(op: *mut PyObject) -> *mut PyObject {
    let _ = op;
    unsafe { crate::api::sequences::PyList_New(0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Values(op: *mut PyObject) -> *mut PyObject {
    let _ = op;
    unsafe { crate::api::sequences::PyList_New(0) }
}
