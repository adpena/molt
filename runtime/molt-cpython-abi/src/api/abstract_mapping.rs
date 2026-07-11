//! Mapping abstract protocol — PyMapping_* operations.
//!
//! Faithful to CPython 3.12 `Objects/abstract.c`: `PyMapping_*` routes through
//! `PyObject_GetItem`/`PyObject_SetItem` (so foreign `mp_subscript` /
//! `mp_ass_subscript` slots are consulted) instead of the previous dict-only
//! shortcuts, and the non-exact-dict `Keys`/`Values`/`Items` call the object's
//! own `keys()`/`values()`/`items()` methods (CPython `method_output_as_list`).

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
    GLOBAL_BRIDGE
        .molt_handle_for_pyobj(op)
        .map(|value| value.bits())
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

#[inline]
fn tag_dict() -> u8 {
    crate::abi_types::MoltTypeTag::Dict as u8
}

/// Non-null `mp_subscript` slot presence on the object's type.
unsafe fn has_mp_subscript(o: *mut PyObject) -> bool {
    if o.is_null() {
        return false;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return false;
    }
    let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
    !m.is_null() && !unsafe { (*m).mp_subscript }.is_null()
}

unsafe fn set_null_error() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_SystemError,
            c"null argument to internal routine".as_ptr(),
        );
    }
}

unsafe fn set_type_error(message: String) {
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return;
    }
    if let Ok(cmsg) = std::ffi::CString::new(message) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                cmsg.as_ptr(),
            );
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Check(o: *mut PyObject) -> c_int {
    // CPython: `o && tp_as_mapping && mp_subscript` — TRUE for any type with a
    // subscript slot: dict, list, tuple, str, bytes, AND foreign C mappings
    // (ndarray). The pre-fix body answered 1 only for the native Dict tag.
    if o.is_null() {
        return 0;
    }
    if let Some(bits) = resolve_bits(o) {
        let tag = classify(bits);
        if tag == tag_dict()
            || tag == crate::abi_types::MoltTypeTag::List as u8
            || tag == crate::abi_types::MoltTypeTag::Tuple as u8
            || tag == crate::abi_types::MoltTypeTag::Str as u8
            || tag == crate::abi_types::MoltTypeTag::Bytes as u8
        {
            return 1;
        }
    }
    unsafe { has_mp_subscript(o) as c_int }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Length(o: *mut PyObject) -> Py_ssize_t {
    unsafe { PyMapping_Size(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Size(o: *mut PyObject) -> Py_ssize_t {
    // CPython: NULL → null_error (SystemError); mp_length when present; else
    // TypeError "object of type '%.200s' has no len()". Every -1 carries an
    // exception (the pre-fix body returned silent -1 for non-dicts).
    if o.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    if let Some(bits) = resolve_bits(o) {
        let tag = classify(bits);
        if tag == tag_dict() {
            let h = hooks_or_stubs();
            return unsafe { (h.dict_len)(bits) as Py_ssize_t };
        }
        // Native list/tuple/str/bytes all expose mp_length in CPython; their
        // length authority here is PySequence_Size (code-point-correct str).
        if tag == crate::abi_types::MoltTypeTag::List as u8
            || tag == crate::abi_types::MoltTypeTag::Tuple as u8
            || tag == crate::abi_types::MoltTypeTag::Str as u8
            || tag == crate::abi_types::MoltTypeTag::Bytes as u8
        {
            return unsafe { crate::api::abstract_sequence::PySequence_Size(o) };
        }
    }
    // Foreign tier: mp_length via the type slot.
    let tp = unsafe { (*o).ob_type };
    if !tp.is_null() {
        let m = unsafe { (*tp).tp_as_mapping }.cast::<crate::abi_types::PyMappingMethods>();
        if !m.is_null() {
            let mp_length = unsafe { (*m).mp_length };
            if !mp_length.is_null() {
                type LenFunc = unsafe extern "C" fn(*mut PyObject) -> Py_ssize_t;
                let f: LenFunc =
                    unsafe { std::mem::transmute::<*mut std::os::raw::c_void, LenFunc>(mp_length) };
                return unsafe { f(o) };
            }
        }
    }
    unsafe {
        set_type_error(format!(
            "object of type '{}' has no len()",
            crate::api::object::type_name_lossy(o)
        ));
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_HasKey(o: *mut PyObject, key: *mut PyObject) -> c_int {
    // CPython: v = PyObject_GetItem(o, key); if (v) { DECREF; return 1; }
    // PyErr_Clear(); return 0 — ANY mapping's mp_subscript is consulted (the
    // pre-fix body probed only the native dict authority).
    if o.is_null() || key.is_null() {
        return 0;
    }
    let v = unsafe { crate::api::object::PyObject_GetItem(o, key) };
    if !v.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(v) };
        return 1;
    }
    unsafe { crate::api::errors::PyErr_Clear() };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_HasKeyString(o: *mut PyObject, key: *const c_char) -> c_int {
    if o.is_null() || key.is_null() {
        return 0;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        unsafe { crate::api::errors::PyErr_Clear() };
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
        unsafe { set_null_error() };
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
    // CPython: 1 + strong ref on found; 0 with *result=NULL on a KeyError
    // (treated as absence); -1 with *result=NULL and the exception left
    // pending on any OTHER error. The pre-fix body probed only PyDict_GetItem
    // and collapsed every failure to 0.
    if result.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if o.is_null() || key.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    let value = unsafe { crate::api::object::PyObject_GetItem(o, key) };
    if !value.is_null() {
        unsafe {
            *result = value;
        }
        return 1;
    }
    // NULL: KeyError == absent (0); anything else is a real error (-1).
    let key_error: *mut PyObject = (&raw mut crate::abi_types::PyExc_KeyError).cast::<PyObject>();
    if unsafe { crate::api::errors::PyErr_ExceptionMatches(key_error) } != 0 {
        unsafe { crate::api::errors::PyErr_Clear() };
        return 0;
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_SetItemString(
    o: *mut PyObject,
    key: *const c_char,
    v: *mut PyObject,
) -> c_int {
    // CPython builds a str key and calls PyObject_SetItem, dispatching to ANY
    // mapping's mp_ass_subscript (the pre-fix body was dict-only).
    if key.is_null() {
        unsafe { set_null_error() };
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { crate::api::object::PyObject_SetItem(o, key_obj, v) };
    unsafe { crate::api::refcount::Py_DECREF(key_obj) };
    rc
}

/// CPython `method_output_as_list(o, meth)`: call `o.<meth>()` and materialize
/// the result as a list via the iterator protocol.
unsafe fn method_output_as_list(o: *mut PyObject, meth: &'static core::ffi::CStr) -> *mut PyObject {
    let bound = unsafe { crate::api::object::PyObject_GetAttrString(o, meth.as_ptr()) };
    if bound.is_null() {
        return ptr::null_mut();
    }
    let output = unsafe { crate::api::object::PyObject_CallNoArgs(bound) };
    unsafe { crate::api::refcount::Py_DECREF(bound) };
    if output.is_null() {
        return ptr::null_mut();
    }
    let list = unsafe { crate::api::abstract_sequence::PySequence_List(output) };
    unsafe { crate::api::refcount::Py_DECREF(output) };
    list
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Keys(o: *mut PyObject) -> *mut PyObject {
    // CPython: exact dict → PyDict_Keys; otherwise call o.keys() and
    // materialize (the pre-fix body forced the runtime dict authority for
    // every receiver, SystemError-ing on real non-dict mappings).
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if let Some(bits) = resolve_bits(o)
        && classify(bits) == tag_dict()
    {
        return unsafe { crate::api::mapping::PyDict_Keys(o) };
    }
    unsafe { method_output_as_list(o, c"keys") }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Values(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if let Some(bits) = resolve_bits(o)
        && classify(bits) == tag_dict()
    {
        return unsafe { crate::api::mapping::PyDict_Values(o) };
    }
    unsafe { method_output_as_list(o, c"values") }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMapping_Items(o: *mut PyObject) -> *mut PyObject {
    // Native dict routes through the runtime dict authority (DictOp::Items)
    // and fails closed with an exception when unavailable; a non-dict mapping
    // gets its own items() method (CPython method_output_as_list).
    if o.is_null() {
        unsafe { set_null_error() };
        return ptr::null_mut();
    }
    if let Some(bits) = resolve_bits(o)
        && classify(bits) == tag_dict()
    {
        return unsafe { crate::api::mapping::PyDict_Items(o) };
    }
    unsafe { method_output_as_list(o, c"items") }
}
