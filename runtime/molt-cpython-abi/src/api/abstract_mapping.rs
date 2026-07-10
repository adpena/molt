//! Mapping abstract protocol — PyMapping_* operations.
//!
//! These implement the abstract mapping operations that work on dicts
//! and other mapping-like objects.

use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::os::raw::{c_char, c_int};
use std::ptr;

/// Helper: resolve a PyObject to its Molt bits.
fn resolve_bits(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    GLOBAL_BRIDGE.lock().pyobj_to_handle(op)
}

/// Helper: classify a heap-pointer handle.
fn classify(bits: u64) -> u8 {
    let obj = MoltObject::from_bits(bits);
    if !obj.is_ptr() {
        return crate::abi_types::MoltTypeTag::Other as u8;
    }
    let h = hooks_or_stubs();
    unsafe { (h.classify_heap)(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Check(o: *mut PyObject) -> c_int {
    if o.is_null() {
        return 0;
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return 0,
    };
    let tag = classify(bits);
    (tag == crate::abi_types::MoltTypeTag::Dict as u8) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Length(o: *mut PyObject) -> Py_ssize_t {
    unsafe { PyMapping_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Size(o: *mut PyObject) -> Py_ssize_t {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return -1,
    };
    let tag = classify(bits);
    if tag != crate::abi_types::MoltTypeTag::Dict as u8 {
        return -1;
    }
    let h = hooks_or_stubs();
    unsafe { (h.dict_len)(bits) as Py_ssize_t }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_HasKey(o: *mut PyObject, key: *mut PyObject) -> c_int {
    if o.is_null() || key.is_null() {
        return 0;
    }
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return 0,
    };
    let key_bits = match resolve_bits(key) {
        Some(b) => b,
        None => return 0,
    };
    let tag = classify(bits);
    if tag != crate::abi_types::MoltTypeTag::Dict as u8 {
        return 0;
    }
    let h = hooks_or_stubs();
    let result = unsafe { (h.dict_get)(bits, key_bits) };
    (result != 0) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_HasKeyString(o: *mut PyObject, key: *const c_char) -> c_int {
    if o.is_null() || key.is_null() {
        return 0;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return 0;
    }
    let result = unsafe { PyMapping_HasKey(o, key_obj) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_GetItemString(
    o: *mut PyObject,
    key: *const c_char,
) -> *mut PyObject {
    if key.is_null() {
        return ptr::null_mut();
    }
    // CPython's `PyMapping_GetItemString` (Objects/abstract.c) builds a str key
    // and routes through `PyObject_GetItem`, so it works for ANY mapping (via
    // `mp_subscript`) and raises `KeyError` on a miss. The prior route through
    // `PyDict_GetItem` returned a bare NULL with no exception on a miss and did
    // nothing for a foreign mapping — the sentinel-without-exception bug this
    // sweep closes. `PyObject_GetItem` already owns the native dict/list/tuple
    // fast paths plus foreign `mp_subscript`/`sq_item` dispatch and INCREFs the
    // returned reference, so no extra refcount handling is needed here.
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { crate::api::object::PyObject_GetItem(o, key_obj) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_GetOptionalItem(
    o: *mut PyObject,
    key: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if o.is_null() || key.is_null() {
        return -1;
    }
    let value = unsafe { crate::api::mapping::PyDict_GetItem(o, key) };
    if value.is_null() {
        return 0;
    }
    unsafe {
        crate::api::refcount::Py_INCREF(value);
        *result = value;
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_SetItemString(
    o: *mut PyObject,
    key: *const c_char,
    v: *mut PyObject,
) -> c_int {
    unsafe { crate::api::mapping::PyDict_SetItemString(o, key, v) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Keys(o: *mut PyObject) -> *mut PyObject {
    // Delegate to PyDict_Keys (currently returns empty list).
    unsafe { crate::api::mapping::PyDict_Keys(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Values(o: *mut PyObject) -> *mut PyObject {
    unsafe { crate::api::mapping::PyDict_Values(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Items(o: *mut PyObject) -> *mut PyObject {
    // Delegate to PyDict_Items, which routes through the runtime dict authority
    // (DictOp::Items) and fails closed with an exception when unavailable. The
    // previous empty-list placeholder was silent data loss — every extension
    // PyMapping_Items(dict) came back empty.
    unsafe { crate::api::mapping::PyDict_Items(o) }
}
