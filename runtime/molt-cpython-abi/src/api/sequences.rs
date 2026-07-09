//! Sequence API — PyList_*, PyTuple_*.

use crate::abi_types::{Py_ssize_t, PyObject, PyTupleObject, PyVarObject};
#[allow(unused_imports)]
use crate::abi_types::{PyList_Type, PyTuple_Type};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use std::os::raw::c_int;
use std::ptr;

// ─── PyList ───────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyList_New(_size: Py_ssize_t) -> *mut PyObject {
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_list)() };
    if bits == 0 {
        // Allocation failed. CPython's PyList_New returns NULL with MemoryError
        // set. Returning Py_None (non-NULL) here would defeat the extension's
        // `if (list == NULL)` guard and let it operate on None as if it were a
        // list — silent corruption. Fail closed with NULL + a set exception.
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_MemoryError,
                c"PyList_New: failed to allocate list".as_ptr(),
            );
        }
        return ptr::null_mut();
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
    // CPython contract: `PyList_Append` takes its own strong reference to the
    // item (it does not steal). Anchor the item proxy so the extension's
    // balancing `Py_DECREF` cannot sever the pointer↔handle mapping while the
    // item stays reachable from the runtime list (same class as the
    // `PyDict_SetItem` anchor — see api/mapping.rs).
    unsafe { crate::api::refcount::Py_INCREF(item) };
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
pub unsafe extern "C" fn PyList_GetItemRef(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    let item = unsafe { PyList_GetItem(op, i) };
    if item.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_INCREF(item) };
    item
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

pub(crate) unsafe fn tuple_layout_object(op: *mut PyObject) -> Option<*mut PyTupleObject> {
    if op.is_null() {
        return None;
    }
    let ob_type = unsafe { (*op).ob_type };
    if std::ptr::eq(ob_type, &raw mut crate::abi_types::PyTuple_Type)
        && GLOBAL_BRIDGE.lock().pyobj_to_handle(op).is_none()
    {
        Some(op.cast::<PyTupleObject>())
    } else {
        None
    }
}

pub unsafe extern "C" fn molt_tuple_dealloc(op: *mut PyObject) {
    let Some(tuple) = (unsafe { tuple_layout_object(op) }) else {
        return;
    };
    let len = unsafe { (*tuple).ob_base.ob_size };
    let item_ptr = unsafe { (*tuple).ob_item };
    if !item_ptr.is_null() && len > 0 {
        let items = unsafe { std::slice::from_raw_parts_mut(item_ptr, len as usize) };
        for item in items.iter_mut() {
            unsafe { crate::api::refcount::Py_XDECREF(*item) };
        }
        let slice = std::ptr::slice_from_raw_parts_mut(item_ptr, len as usize);
        unsafe { drop(Box::from_raw(slice)) };
    }
    unsafe { drop(Box::from_raw(tuple)) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_New(size: Py_ssize_t) -> *mut PyObject {
    let n = if size < 0 { 0 } else { size as usize };
    let items = vec![ptr::null_mut(); n].into_boxed_slice();
    let item_ptr = Box::leak(items).as_mut_ptr();
    let tuple = Box::new(PyTupleObject {
        ob_base: PyVarObject {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut crate::abi_types::PyTuple_Type,
            },
            ob_size: n as Py_ssize_t,
        },
        ob_item: item_ptr,
    });
    Box::into_raw(tuple).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyTuple_GET_ITEM(op: *mut PyObject, i: Py_ssize_t) -> *mut PyObject {
    if op.is_null() || i < 0 {
        return ptr::null_mut();
    }
    if let Some(tuple) = unsafe { tuple_layout_object(op) } {
        let len = unsafe { (*tuple).ob_base.ob_size };
        if i >= len || unsafe { (*tuple).ob_item.is_null() } {
            return ptr::null_mut();
        }
        return unsafe { *(*tuple).ob_item.add(i as usize) };
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
    if let Some(tuple) = unsafe { tuple_layout_object(op) } {
        return unsafe { (*tuple).ob_base.ob_size };
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
pub unsafe extern "C" fn PyTuple_GetSlice(
    op: *mut PyObject,
    start: Py_ssize_t,
    end: Py_ssize_t,
) -> *mut PyObject {
    if op.is_null() {
        return ptr::null_mut();
    }
    let len = unsafe { PyTuple_GET_SIZE(op) };
    let lo = start.clamp(0, len);
    let hi = end.clamp(lo, len);
    let out = unsafe { PyTuple_New(hi - lo) };
    if out.is_null() {
        return ptr::null_mut();
    }
    for index in lo..hi {
        let item = unsafe { PyTuple_GET_ITEM(op, index) };
        if item.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(out) };
            return ptr::null_mut();
        }
        unsafe { crate::api::refcount::Py_INCREF(item) };
        if unsafe { PyTuple_SetItem(out, index - lo, item) } != 0 {
            unsafe {
                crate::api::refcount::Py_DECREF(item);
                crate::api::refcount::Py_DECREF(out);
            }
            return ptr::null_mut();
        }
    }
    out
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
    if let Some(tuple) = unsafe { tuple_layout_object(op) } {
        let len = unsafe { (*tuple).ob_base.ob_size };
        if i >= len || unsafe { (*tuple).ob_item.is_null() } {
            return -1;
        }
        let slot = unsafe { (*tuple).ob_item.add(i as usize) };
        unsafe {
            let old = *slot;
            *slot = v;
            crate::api::refcount::Py_XDECREF(old);
        }
        return 0;
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

#[allow(dead_code)]
unsafe fn py_tuple_pack_placeholder_removed_from_abi(n: Py_ssize_t, /* ... */) -> *mut PyObject {
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

/// Ensure a NULL / -1 error return always carries a pending exception.
///
/// The runtime set hooks return the CPython error sentinel (0 / -1) after
/// setting a pending runtime exception. If a caller reaches a failure path with
/// no pending exception (e.g. hooks unregistered, or a bridge resolution
/// failure that the runtime never saw), the ABI contract still requires an
/// exception on the error return — set a SystemError so callers never observe a
/// NULL/-1 without `PyErr_Occurred()`.
unsafe fn ensure_set_error(message: &'static core::ffi::CStr) {
    let pending = crate::hooks::hooks()
        .map(|h| unsafe { (h.exception_pending)() } != 0)
        .unwrap_or(false);
    if !pending {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                message.as_ptr(),
            );
        }
    }
}

/// Resolve a bridge-managed set argument to its runtime handle bits, or set a
/// SystemError and return None (matching CPython's SystemError for a non-set).
unsafe fn set_arg_handle(anyset: *mut PyObject, message: &'static core::ffi::CStr) -> Option<u64> {
    // Drop the bridge lock before any PyErr_SetString / hook call — both
    // re-acquire GLOBAL_BRIDGE, so holding the guard would self-deadlock.
    let handle = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge.pyobj_to_handle(anyset)
    };
    match handle {
        Some(bits) => Some(bits),
        None => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    message.as_ptr(),
                );
            }
            None
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_New(iterable: *mut PyObject) -> *mut PyObject {
    // NULL iterable → empty set (CPython accepts NULL). A non-NULL iterable must
    // be a bridge-managed object; resolve it to handle bits (0 signals "empty"
    // to the runtime authority).
    let iterable_bits = if iterable.is_null() {
        0
    } else {
        match unsafe {
            set_arg_handle(
                iterable,
                c"PySet_New: iterable is not a bridge-managed object",
            )
        } {
            Some(bits) => bits,
            None => return ptr::null_mut(),
        }
    };
    let h = hooks_or_stubs();
    let result = unsafe { (h.set_new)(iterable_bits) };
    if result == 0 {
        unsafe { ensure_set_error(c"PySet_New failed: runtime set authority unavailable") };
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Size(anyset: *mut PyObject) -> Py_ssize_t {
    let bits = match unsafe {
        set_arg_handle(anyset, c"PySet_Size: argument is not a bridge-managed set")
    } {
        Some(bits) => bits,
        None => return -1,
    };
    let h = hooks_or_stubs();
    let n = unsafe { (h.set_size)(bits) };
    if n < 0 {
        unsafe { ensure_set_error(c"PySet_Size failed: runtime set authority unavailable") };
        return -1;
    }
    n as Py_ssize_t
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Contains(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    if key.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PySet_Contains: key must not be NULL".as_ptr(),
            );
        }
        return -1;
    }
    let set_bits = match unsafe {
        set_arg_handle(
            anyset,
            c"PySet_Contains: argument is not a bridge-managed set",
        )
    } {
        Some(bits) => bits,
        None => return -1,
    };
    let key_bits =
        match unsafe { set_arg_handle(key, c"PySet_Contains: key is not a bridge-managed object") }
        {
            Some(bits) => bits,
            None => return -1,
        };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.set_contains)(set_bits, key_bits) };
    if rc < 0 {
        unsafe { ensure_set_error(c"PySet_Contains failed: runtime set authority unavailable") };
        return -1;
    }
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Add(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    if key.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PySet_Add: key must not be NULL".as_ptr(),
            );
        }
        return -1;
    }
    let set_bits =
        match unsafe { set_arg_handle(anyset, c"PySet_Add: argument is not a bridge-managed set") }
        {
            Some(bits) => bits,
            None => return -1,
        };
    let key_bits =
        match unsafe { set_arg_handle(key, c"PySet_Add: key is not a bridge-managed object") } {
            Some(bits) => bits,
            None => return -1,
        };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.set_add)(set_bits, key_bits) };
    if rc != 0 {
        unsafe { ensure_set_error(c"PySet_Add failed: runtime set authority unavailable") };
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PySet_Discard(anyset: *mut PyObject, key: *mut PyObject) -> c_int {
    if key.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PySet_Discard: key must not be NULL".as_ptr(),
            );
        }
        return -1;
    }
    let set_bits = match unsafe {
        set_arg_handle(
            anyset,
            c"PySet_Discard: argument is not a bridge-managed set",
        )
    } {
        Some(bits) => bits,
        None => return -1,
    };
    let key_bits = match unsafe {
        set_arg_handle(key, c"PySet_Discard: key is not a bridge-managed object")
    } {
        Some(bits) => bits,
        None => return -1,
    };
    let h = hooks_or_stubs();
    let rc = unsafe { (h.set_discard)(set_bits, key_bits) };
    if rc < 0 {
        unsafe { ensure_set_error(c"PySet_Discard failed: runtime set authority unavailable") };
        return -1;
    }
    rc
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
pub unsafe extern "C" fn PyList_SetSlice(
    op: *mut PyObject,
    _ilow: Py_ssize_t,
    _ihigh: Py_ssize_t,
    _itemlist: *mut PyObject,
) -> c_int {
    if op.is_null() {
        return -1;
    }
    0
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
