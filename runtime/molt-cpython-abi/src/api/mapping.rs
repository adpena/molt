//! Mapping API — PyDict_*.

use crate::abi_types::{Py_ssize_t, PyDictProxyObject, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use std::os::raw::c_int;
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_New() -> *mut PyObject {
    let h = hooks_or_stubs();
    let bits = unsafe { (h.alloc_dict)() };
    if bits == 0 {
        // Allocation failed. CPython's PyDict_New returns NULL with MemoryError
        // set. Returning Py_None (non-NULL) would defeat the extension's
        // `if (dict == NULL)` guard and let it treat None as a dict — silent
        // corruption. Fail closed with NULL + a set exception.
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_MemoryError,
                c"PyDict_New: failed to allocate dict".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyDict_NewPresized(_minused: Py_ssize_t) -> *mut PyObject {
    unsafe { PyDict_New() }
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
        None => {
            // Address-only (no deref): an unresolved dict receiver is often a wild
            // / non-canonical pointer that is NOT safe to dereference, so we must
            // not call `describe_unresolved_pyobject` here (it would trap on
            // out-of-bounds linear memory).
            let detail = format!("unresolved dict @ {:p}", op);
            crate::capi_trace::record_silent_failure("PyDict_SetItem", Some(&detail));
            return -1;
        }
    };
    let key_bits = match bridge.pyobj_to_handle(key) {
        Some(b) => b,
        None => {
            // Address-only (no deref) for the same reason as the dict receiver.
            let detail = format!("unresolved key @ {:p}", key);
            crate::capi_trace::record_silent_failure("PyDict_SetItem", Some(&detail));
            return -1;
        }
    };
    let val_bits = match bridge.pyobj_to_handle(value) {
        Some(b) => b,
        None => {
            let detail = format!(
                "unresolved value: {}",
                unsafe { crate::abi_types::describe_unresolved_pyobject(value) }
            );
            crate::capi_trace::record_silent_failure("PyDict_SetItem", Some(&detail));
            return -1;
        }
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
    if rc != 0 {
        // Re-record with the string key so the diagnostic names the exact dict
        // entry that failed (e.g. numpy's `error`, `__cpu_features__`). This
        // overwrites the inner PyDict_SetItem record (last-write-wins) and is
        // free on the normal path (only runs on failure).
        let key_str = unsafe { std::ffi::CStr::from_ptr(key) }
            .to_str()
            .unwrap_or("<non-utf8 key>");
        let detail = format!(
            "key='{key_str}' value: {}",
            unsafe { crate::abi_types::describe_unresolved_pyobject(value) }
        );
        crate::capi_trace::record_silent_failure("PyDict_SetItemString", Some(&detail));
    }
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Merge(
    op: *mut PyObject,
    other: *mut PyObject,
    _override: c_int,
) -> c_int {
    if op.is_null() || other.is_null() {
        return -1;
    }
    let size = unsafe { PyDict_Size(other) };
    if size == 0 {
        return 0;
    }
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_RuntimeError,
            c"PyDict_Merge requires Molt dict iteration hook".as_ptr(),
        );
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDictProxy_New(mapping: *mut PyObject) -> *mut PyObject {
    if mapping.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"PyDictProxy_New mapping must not be NULL".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    unsafe { crate::api::refcount::Py_INCREF(mapping) };
    let proxy = Box::new(PyDictProxyObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyDictProxy_Type,
        },
        mapping,
    });
    Box::into_raw(proxy).cast::<PyObject>()
}

pub unsafe extern "C" fn molt_dictproxy_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    let proxy = op.cast::<PyDictProxyObject>();
    unsafe {
        crate::api::refcount::Py_XDECREF((*proxy).mapping);
        drop(Box::from_raw(proxy));
    }
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
    unsafe { GLOBAL_BRIDGE.lock().handle_to_borrowed_pyobj(val_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemWithError(
    op: *mut PyObject,
    key: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyDict_GetItem(op, key) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemRef(
    op: *mut PyObject,
    key: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    let value = unsafe { PyDict_GetItemWithError(op, key) };
    if value.is_null() {
        0
    } else {
        unsafe {
            crate::api::refcount::Py_INCREF(value);
            *result = value;
        }
        1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemStringRef(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if key.is_null() {
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyDict_GetItemRef(op, key_obj, result) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyDict_GetItem_KnownHash(
    op: *mut PyObject,
    key: *mut PyObject,
    _hash: crate::abi_types::Py_hash_t,
) -> *mut PyObject {
    unsafe { PyDict_GetItem(op, key) }
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
pub unsafe extern "C" fn PyDict_SetDefault(
    op: *mut PyObject,
    key: *mut PyObject,
    default_value: *mut PyObject,
) -> *mut PyObject {
    if op.is_null() || key.is_null() || default_value.is_null() {
        return ptr::null_mut();
    }
    let existing = unsafe { PyDict_GetItem(op, key) };
    if !existing.is_null() {
        return existing;
    }
    if unsafe { PyDict_SetItem(op, key, default_value) } != 0 {
        return ptr::null_mut();
    }
    default_value
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_SetDefaultRef(
    op: *mut PyObject,
    key: *mut PyObject,
    default_value: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if op.is_null() || key.is_null() || default_value.is_null() {
        return -1;
    }
    let existing = unsafe { PyDict_GetItem(op, key) };
    if !existing.is_null() {
        unsafe {
            crate::api::refcount::Py_INCREF(existing);
            *result = existing;
        }
        return 1;
    }
    if unsafe { PyDict_SetItem(op, key, default_value) } != 0 {
        return -1;
    }
    unsafe {
        crate::api::refcount::Py_INCREF(default_value);
        *result = default_value;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_DelItem(op: *mut PyObject, key: *mut PyObject) -> c_int {
    if op.is_null() || key.is_null() {
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
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.dict_del)(dict_bits, key_bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_DelItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> c_int {
    if op.is_null() || key.is_null() {
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyDict_DelItem(op, key_obj) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    rc
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
pub unsafe extern "C" fn PyDict_Next(
    op: *mut PyObject,
    pos: *mut Py_ssize_t,
    key: *mut *mut PyObject,
    value: *mut *mut PyObject,
) -> c_int {
    if op.is_null() || pos.is_null() {
        return 0;
    }
    let size = unsafe { PyDict_Size(op) };
    if unsafe { *pos } >= size {
        return 0;
    }
    if size > 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_RuntimeError,
                c"PyDict_Next requires Molt dict iteration hook".as_ptr(),
            );
        }
    }
    unsafe {
        *pos = size;
        if !key.is_null() {
            *key = ptr::null_mut();
        }
        if !value.is_null() {
            *value = ptr::null_mut();
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Contains(op: *mut PyObject, key: *mut PyObject) -> c_int {
    let value = unsafe { PyDict_GetItemWithError(op, key) };
    if value.is_null() { 0 } else { 1 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_ContainsString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> c_int {
    if key.is_null() {
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyDict_Contains(op, key_obj) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    (std::ptr::eq(ob_type, &raw const crate::abi_types::PyDict_Type)) as c_int
}

/// Dispatch a dict-iteration op through the runtime dict authority.
///
/// Ignoring `op` and returning an empty dict/list (the previous behavior) is
/// silent data loss — every `dict.copy()`, `.keys()`, `.values()` from an
/// extension came back empty. Route to the runtime, which reads the real dict.
unsafe fn dict_op(op: crate::hooks::DictOp, dict: *mut PyObject) -> *mut PyObject {
    // Resolve and drop the bridge lock before any PyErr_SetString / hook call:
    // PyErr_SetString and handle_to_pyobj re-acquire GLOBAL_BRIDGE, so holding
    // the guard across them would self-deadlock.
    let handle = {
        let bridge = GLOBAL_BRIDGE.lock();
        bridge.pyobj_to_handle(dict)
    };
    let bits = match handle {
        Some(b) => b,
        None => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"PyDict op: argument is not a bridge-managed object".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
    };
    let h = hooks_or_stubs();
    let result = unsafe { (h.dict_op)(op as u32, bits) };
    if result == 0 {
        // Runtime set a pending exception, or hooks are unregistered. Ensure a
        // NULL return always carries an exception (ABI contract).
        let pending = crate::hooks::hooks()
            .map(|h| unsafe { (h.exception_pending)() } != 0)
            .unwrap_or(false);
        if !pending {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_SystemError,
                    c"PyDict op failed: runtime dict authority unavailable".as_ptr(),
                );
            }
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Copy(op: *mut PyObject) -> *mut PyObject {
    unsafe { dict_op(crate::hooks::DictOp::Copy, op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Keys(op: *mut PyObject) -> *mut PyObject {
    unsafe { dict_op(crate::hooks::DictOp::Keys, op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Values(op: *mut PyObject) -> *mut PyObject {
    unsafe { dict_op(crate::hooks::DictOp::Values, op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Items(op: *mut PyObject) -> *mut PyObject {
    unsafe { dict_op(crate::hooks::DictOp::Items, op) }
}
