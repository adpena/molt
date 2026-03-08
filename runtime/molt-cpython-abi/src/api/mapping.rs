//! Mapping API — PyDict_*.

use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_New() -> *mut PyObject {
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_dict)() };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_SetItem(
    op: *mut PyObject,
    key: *mut PyObject,
    value: *mut PyObject,
) -> c_int {
    if op.is_null() || key.is_null() || value.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let dict_bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return -1,
    };
    let key_bits = match bridge.pyobj_to_handle(key) {
        Some(b) => b,
        None => return -1,
    };
    let val_bits = match bridge.pyobj_to_handle(value) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.dict_set)(dict_bits, key_bits, val_bits) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_SetItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
    value: *mut PyObject,
) -> c_int {
    if op.is_null() || key.is_null() || value.is_null() {
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyDict_SetItem(op, key_obj, value) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItem(op: *mut PyObject, key: *mut PyObject) -> *mut PyObject {
    if op.is_null() || key.is_null() {
        return ptr::null_mut();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let dict_bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let key_bits = match bridge.pyobj_to_handle(key) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let val_bits = unsafe { (h.dict_get)(dict_bits, key_bits) };
    if val_bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(val_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> *mut PyObject {
    if op.is_null() || key.is_null() {
        return ptr::null_mut();
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyDict_GetItem(op, key_obj) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_DelItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> c_int {
    let _ = (op, key);
    // TODO: implement dict delete when runtime exposes hook.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Size(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() {
        return 0;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return 0,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.dict_len)(bits) as Py_ssize_t }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
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
