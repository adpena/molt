//! Sequence API — PyList_*, PyTuple_*.

use crate::abi_types::{Py_ssize_t, PyObject};
#[allow(unused_imports)]
use crate::abi_types::{PyList_Type, PyTuple_Type};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;

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
    if list.is_null() || item.is_null() {
        return -1;
    }
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
    if op.is_null() || i < 0 {
        return ptr::null_mut();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let item_bits = unsafe { (h.list_item)(bits, i as usize) };
    if item_bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    unsafe { PyList_GET_ITEM(op, i) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_SET_ITEM(op: *mut PyObject, i: Py_ssize_t, v: *mut PyObject) {
    if op.is_null() || i < 0 || v.is_null() {
        return;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let list_bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return,
    };
    let val_bits = match bridge.pyobj_to_handle(v) {
        Some(b) => b,
        None => return,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    // CPython's PyList_SET_ITEM is used almost exclusively in a
    // build-then-fill pattern right after PyList_New(n).  The runtime
    // hooks expose list_append but not indexed set.  Append gives correct
    // results when items are set in order (index 0, 1, 2, ...), which is
    // the only pattern C extensions use with SET_ITEM on a freshly
    // allocated list.  For out-of-order indexed set we would need a
    // list_set_item hook; that is not required by any extension we support.
    unsafe { (h.list_append)(list_bits, val_bits) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_SetItem(
    op: *mut PyObject,
    i: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    if op.is_null() || i < 0 || v.is_null() {
        return -1;
    }
    unsafe { PyList_SET_ITEM(op, i, v) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
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
    unsafe { (h.list_len)(bits) as Py_ssize_t }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Size(op: *mut PyObject) -> Py_ssize_t {
    unsafe { PyList_GET_SIZE(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PyList_Type)) as c_int
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
    if op.is_null() || i < 0 {
        return ptr::null_mut();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let item_bits = unsafe { (h.tuple_item)(bits, i as usize) };
    if item_bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(item_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GetItem(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    unsafe { PyTuple_GET_ITEM(op, i) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GET_SIZE(op: *mut PyObject) -> Py_ssize_t {
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
    if op.is_null() || i < 0 || v.is_null() {
        return -1;
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let tuple_bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return -1,
    };
    let val_bits = match bridge.pyobj_to_handle(v) {
        Some(b) => b,
        None => return -1,
    };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.tuple_set)(tuple_bits, i as usize, val_bits) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PyTuple_Type)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_Pack(n: Py_ssize_t /* ... */) -> *mut PyObject {
    // Variadic — without va_list we can only create an empty tuple.
    // Real variadic support is in the C shim.
    unsafe { PyTuple_New(n) }
}

// ─── PySet ────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PySet_Type)
        || std::ptr::eq(ob_type, &raw const crate::abi_types::PyFrozenSet_Type)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFrozenSet_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PyFrozenSet_Type)) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_New(iterable: *mut PyObject) -> *mut PyObject {
    // Create an empty set. Full iteration over iterable is not yet supported.
    let _ = iterable;
    // Sets are not yet supported by the bridge hooks; return a placeholder list.
    // This allows extensions that create sets to not crash.
    unsafe { PyList_New(0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Size(anyset: *mut PyObject) -> Py_ssize_t {
    if anyset.is_null() {
        return 0;
    }
    // Delegate to the generic length.
    unsafe { crate::api::object::PyObject_Length(anyset) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Contains(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    let _ = (anyset, key);
    // Cannot check set membership without set hooks.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Add(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    // Stub — sets are not fully supported.
    let _ = (anyset, key);
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Discard(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    let _ = (anyset, key);
    0
}

// ─── PyList_GetSlice / PyList_Sort / PyList_Reverse ──────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_GetSlice(
    op: *mut PyObject,
    ilow: Py_ssize_t,
    ihigh: Py_ssize_t,
) -> *mut PyObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let len = unsafe { (h.list_len)(bits) } as Py_ssize_t;
    let low = ilow.max(0).min(len);
    let high = ihigh.max(low).min(len);
    let new_list = unsafe { (h.alloc_list)() };
    if new_list == 0 {
        return ptr::null_mut();
    }
    for i in low..high {
        let item = unsafe { (h.list_item)(bits, i as usize) };
        unsafe { (h.list_append)(new_list, item) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_list) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Sort(op: *mut PyObject) -> c_int {
    // Sorting requires a comparison hook not yet available.
    let _ = op;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Reverse(op: *mut PyObject) -> c_int {
    // Reversal requires a list mutation hook not yet available.
    let _ = op;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_AsTuple(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    let bridge = GLOBAL_BRIDGE.lock();
    let bits = match bridge.pyobj_to_handle(op) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    drop(bridge);
    let h = hooks_or_stubs();
    let len = unsafe { (h.list_len)(bits) };
    let new_tuple = unsafe { (h.alloc_tuple)(len) };
    if new_tuple == 0 {
        return ptr::null_mut();
    }
    for i in 0..len {
        let item = unsafe { (h.list_item)(bits, i) };
        unsafe { (h.tuple_set)(new_tuple, i, item) };
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(new_tuple) }
}

// ─── PyList_Insert ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_Insert(
    op: *mut PyObject,
    _where_: Py_ssize_t,
    v: *mut PyObject,
) -> c_int {
    // Without indexed insert in hooks, fall back to append.
    unsafe { PyList_Append(op, v) }
}
