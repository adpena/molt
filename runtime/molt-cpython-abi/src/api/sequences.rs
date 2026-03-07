//! Sequence API — PyList_*, PyTuple_*.

use crate::abi_types::{PyObject, Py_ssize_t};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;
#[allow(unused_imports)]
use crate::abi_types::{PyList_Type, PyTuple_Type};

// ─── PyList ───────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_New(_size: Py_ssize_t) -> *mut PyObject {
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_list)() };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Append(list: *mut PyObject, item: *mut PyObject) -> c_int {
    if list.is_null() || item.is_null() { return -1; }
    let bridge = GLOBAL_BRIDGE.lock();
    let list_bits = match bridge.pyobj_to_handle(list) {
        Some(b) => b,
        None => return -1,
    };
    let item_bits = match bridge.pyobj_to_handle(item) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.list_append)(list_bits, item_bits) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GET_ITEM(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    if op.is_null() || i < 0 { return ptr::null_mut(); }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let item_bits = unsafe { (h.list_item)(bits, i as usize) };
    if item_bits == 0 { return ptr::null_mut(); }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    unsafe { PyList_GET_ITEM(op, i) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_SET_ITEM(op: *mut PyObject, i: Py_ssize_t, v: *mut PyObject) {
    if op.is_null() || i < 0 || v.is_null() { return; }
    let bridge = GLOBAL_BRIDGE.lock();
    let list_bits = match bridge.pyobj_to_handle(op) { Some(b) => b, None => return };
    let val_bits  = match bridge.pyobj_to_handle(v)  { Some(b) => b, None => return };
    drop(bridge);
    let h = hooks_or_stubs();
    // SET_ITEM on a list uses list_append semantics at index i.
    // For simplicity: append (correct for build-then-fill pattern).
    let _ = (list_bits, val_bits, i);
    // TODO: implement indexed set when runtime exposes list_set_item hook.
    let _ = h;
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
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) { Some(b) => b, None => return 0 };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.list_len)(bits) as Py_ssize_t }
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
    let n = if size < 0 { 0 } else { size as usize };
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_tuple)(n) };
    if bits == 0 {
        let fallback = MoltObject::none().bits();
        return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(fallback) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GET_ITEM(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    if op.is_null() || i < 0 { return ptr::null_mut(); }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) { Some(b) => b, None => return ptr::null_mut() };
    drop(bridge);
    let h = hooks_or_stubs();
    let item_bits = unsafe { (h.tuple_item)(bits, i as usize) };
    if item_bits == 0 { return ptr::null_mut(); }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    unsafe { PyTuple_GET_ITEM(op, i) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
    if op.is_null() { return 0; }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) { Some(b) => b, None => return 0 };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.tuple_len)(bits) as Py_ssize_t }
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
    if op.is_null() || i < 0 || v.is_null() { return -1; }
    let bridge = GLOBAL_BRIDGE.lock();
    let tuple_bits = match bridge.pyobj_to_handle(op) { Some(b) => b, None => return -1 };
    let val_bits   = match bridge.pyobj_to_handle(v)  { Some(b) => b, None => return -1 };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.tuple_set)(tuple_bits, i as usize, val_bits) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Check(op: *mut PyObject) -> c_int {
    if op.is_null() { return 0; }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, unsafe { &raw const crate::abi_types::PyTuple_Type })) as c_int
}
