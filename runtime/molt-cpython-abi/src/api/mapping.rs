//! Mapping API — PyDict_*.

use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::{GLOBAL_BRIDGE, RuntimeValue};
use crate::hooks::hooks_or_stubs;
#[cfg(test)]
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;

/// Resolve the runtime's single dict storage authority, retaining its error.
fn resolve_dict(op: *mut PyObject, merge_source: bool) -> Result<Option<u64>, ()> {
    if op.is_null() {
        return Ok(None);
    }
    let Some(handle) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) else {
        return Ok(None);
    };
    match unsafe { (hooks_or_stubs().dict_resolve)(handle.bits(), u8::from(merge_source)) }.decode()
    {
        crate::hooks::DecodedHandleResult::Ok(bits) => Ok(Some(bits)),
        crate::hooks::DecodedHandleResult::Missing => Ok(None),
        crate::hooks::DecodedHandleResult::Error => {
            bad_dict_argument();
            Err(())
        }
    }
}

fn bad_dict_argument() {
    crate::api::errors::transfer_runtime_pending_to_current();
    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
    }
}

fn require_dict(op: *mut PyObject) -> Option<u64> {
    match resolve_dict(op, false) {
        Ok(Some(bits)) => Some(bits),
        Ok(None) => {
            bad_dict_argument();
            None
        }
        Err(()) => None,
    }
}

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
                (&raw mut crate::abi_types::PyExc_MemoryError).cast::<crate::abi_types::PyObject>(),
                c"PyDict_New: failed to allocate dict".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
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
    unsafe { dict_mutate(op, key, value, false, None, ptr::null_mut()) }
}

/// Shared ABI dictionary transaction. Callback publishes native derived fields after storage commit; the runtime retains the actual displaced edges.
pub(crate) unsafe fn dict_mutate(
    op: *mut PyObject,
    key: *mut PyObject,
    value: *mut PyObject,
    delete: bool,
    publish: Option<unsafe extern "C" fn(*mut std::ffi::c_void) -> i32>,
    context: *mut std::ffi::c_void,
) -> c_int {
    if op.is_null() || key.is_null() || (!delete && value.is_null()) {
        bad_dict_argument();
        return -1;
    }
    let Some(dict_bits) = require_dict(op) else {
        return -1;
    };
    let Some(key_value) = (unsafe { RuntimeValue::acquire(key) }) else {
        // The dict receiver already resolved, so we hold a well-formed dict —
        // fail loud rather than returning a contentless -1 when the key cannot
        // cross into the runtime. A failed observation of a known object is not
        // retried as a foreign value by RuntimeValue.
        let detail = format!("unresolved key @ {:p}: {}", key, unsafe {
            crate::abi_types::describe_unresolved_pyobject(key)
        });
        crate::capi_trace::record_silent_failure("PyDict_SetItem", Some(&detail));
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"PyDict_SetItem: key is not a bridge-managed object and no foreign wrapper could be minted"
                        .as_ptr(),
                );
            }
        }
        return -1;
    };
    let value_value = if delete {
        None
    } else {
        let Some(value) = (unsafe { RuntimeValue::acquire_edge(value) }) else {
            return -1;
        };
        Some(value)
    };
    let h = hooks_or_stubs();
    let rc = unsafe {
        (h.dict_mutate)(
            dict_bits,
            key_value.bits(),
            value_value.as_ref().map_or(0, RuntimeValue::bits),
            delete as u8,
            publish,
            context,
        )
    };
    let had_error = crate::api::errors::transfer_runtime_pending_to_current();
    drop(key_value);
    drop(value_value);
    // CPython contract: the dict takes its OWN strong references to key and
    // value (PyDict_SetItem does not steal). The runtime dict edges and
    // canonical ABI views jointly retain identity while either side can still
    // observe an entry. Without that custody, numpy's
    // `npy_cpu_dispatch_tracer_init` pattern — `PyDict_New()` →
    // `PyDict_SetItemString(mod_dict, …)` → `Py_DECREF(reg_dict)` → cache the
    // borrowed pointer — left `cpu_dispatch_registry` unresolvable and every
    // later `PyDict_SetItemString(registry, "argmin"/"argmax", …)` failed
    // "unresolved dict". Runtime mutation and ABI-view retirement now share
    // that lifecycle authority rather than retaining a permanent proxy anchor.
    if rc == 1 && delete && !had_error {
        unsafe {
            crate::api::errors::PyErr_SetObject(
                (&raw mut crate::abi_types::PyExc_KeyError).cast(),
                key,
            )
        };
        return -1;
    }
    match (rc == 0, had_error) {
        (true, false) => 0,
        (false, true) => -1,
        (false, false) => {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"PyDict_SetItem runtime hook failed without setting an exception".as_ptr(),
                )
            };
            -1
        }
        (true, true) => {
            unsafe {
                crate::api::errors::replace_current_with_system_error(
                    "PyDict_SetItem runtime hook returned success with an exception set",
                )
            };
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_SetItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
    value: *mut PyObject,
) -> c_int {
    if op.is_null() || key.is_null() || value.is_null() {
        bad_dict_argument();
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
        let detail = format!("key='{key_str}' value: {}", unsafe {
            crate::abi_types::describe_unresolved_pyobject(value)
        });
        crate::capi_trace::record_silent_failure("PyDict_SetItemString", Some(&detail));
    }
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::refcount::Py_DECREF(key_obj)
    });
    rc
}

/// Merge every key/value of `other` into `op`, honoring CPython's `override`
/// contract (matches `Objects/dictobject.c` `dict_merge`):
/// * `override == 1` — overwrite unconditionally (used by `PyDict_Update`);
/// * `override == 0` — keep the existing value, skip duplicate keys;
/// * `override == 2` — raise `KeyError` on the first duplicate key.
///
/// The previous body hard-failed with `RuntimeError` on any non-empty source,
/// blocking numpy `__dict__` / namespace population. Now the native-dict fast
/// path walks `other` through the allocation-free [`PyDict_Next`] cursor; a
/// non-dict mapping goes through the `keys()` / `__getitem__` protocol exactly
/// as CPython's `dict_merge` does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Merge(
    op: *mut PyObject,
    other: *mut PyObject,
    override_: c_int,
) -> c_int {
    let override_ = c_int::from(override_ != 0);
    // CPython: `a` must be a dict (`!PyDict_Check(a)` → `PyErr_BadInternalCall`).
    if other.is_null() {
        bad_dict_argument();
        return -1;
    }
    if require_dict(op).is_none() {
        return -1;
    }
    // ── Native-dict fast path: iterate `other` via the O(1) cursor. ──
    let source_backing = match resolve_dict(other, true) {
        Ok(backing) => backing,
        Err(()) => return -1,
    };
    if source_backing.is_some() {
        if std::ptr::eq(op, other) {
            return 0;
        }
        let mut pos: Py_ssize_t = 0;
        let mut key: *mut PyObject = ptr::null_mut();
        let mut val: *mut PyObject = ptr::null_mut();
        while unsafe { PyDict_Next(other, &raw mut pos, &raw mut key, &raw mut val) } == 1 {
            if override_ != 1 {
                // override 0 (skip) / 2 (raise) both first test for presence.
                let contains = unsafe { PyDict_Contains(op, key) };
                if contains < 0 {
                    return -1;
                }
                if contains == 1 {
                    if override_ == 2 {
                        unsafe {
                            crate::api::errors::PyErr_SetObject(
                                (&raw mut crate::abi_types::PyExc_KeyError)
                                    .cast::<crate::abi_types::PyObject>(),
                                key,
                            );
                        }
                        return -1;
                    }
                    continue; // override == 0: keep the existing value
                }
            }
            if unsafe { PyDict_SetItem(op, key, val) } != 0 {
                return -1;
            }
        }
        return if crate::api::errors::transfer_runtime_pending_to_current() {
            -1
        } else {
            0
        };
    }
    // ── Non-dict mapping: keys() + __getitem__ protocol (CPython slow path). ──
    unsafe { dict_merge_from_mapping(op, other, override_) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Update(op: *mut PyObject, other: *mut PyObject) -> c_int {
    unsafe { PyDict_Merge(op, other, 1) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_MergeFromSeq2(
    op: *mut PyObject,
    seq2: *mut PyObject,
    override_: c_int,
) -> c_int {
    if seq2.is_null() {
        bad_dict_argument();
        return -1;
    }
    if require_dict(op).is_none() {
        return -1;
    }
    let iter = unsafe { crate::api::object::PyObject_GetIter(seq2) };
    if iter.is_null() {
        return -1;
    }
    let mut index = 0usize;
    loop {
        let item = unsafe { crate::api::object::PyIter_Next(iter) };
        if item.is_null() {
            break;
        }
        let fast = unsafe {
            crate::api::abstract_sequence::PySequence_Fast(
                item,
                c"cannot convert dictionary update sequence element to a sequence".as_ptr(),
            )
        };
        unsafe { crate::api::refcount::Py_DECREF(item) };
        if fast.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(iter) };
            return -1;
        }
        let size = unsafe { crate::api::abstract_sequence::PySequence_Fast_GET_SIZE(fast) };
        if size != 2 {
            let message = format!(
                "dictionary update sequence element #{} has length {}; 2 is required",
                index, size
            );
            let c_message = std::ffi::CString::new(message).expect("dict merge error has no NUL");
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_ValueError)
                        .cast::<crate::abi_types::PyObject>(),
                    c_message.as_ptr(),
                );
                crate::api::refcount::Py_DECREF(fast);
                crate::api::refcount::Py_DECREF(iter);
            }
            return -1;
        }
        let key = unsafe { crate::api::abstract_sequence::PySequence_Fast_GET_ITEM(fast, 0) };
        let value = unsafe { crate::api::abstract_sequence::PySequence_Fast_GET_ITEM(fast, 1) };
        let contains = if override_ == 0 {
            unsafe { PyDict_Contains(op, key) }
        } else {
            0
        };
        if contains < 0 {
            unsafe {
                crate::api::refcount::Py_DECREF(fast);
                crate::api::refcount::Py_DECREF(iter);
            }
            return -1;
        }
        let should_set = override_ != 0 || contains == 0;
        let rc = if should_set {
            unsafe { PyDict_SetItem(op, key, value) }
        } else {
            0
        };
        unsafe { crate::api::refcount::Py_DECREF(fast) };
        if rc != 0 {
            unsafe { crate::api::refcount::Py_DECREF(iter) };
            return -1;
        }
        index += 1;
    }
    unsafe { crate::api::refcount::Py_DECREF(iter) };
    if !crate::api::errors::transfer_runtime_pending_to_current() {
        0
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Clear(op: *mut PyObject) {
    let Some(bits) = require_dict(op) else {
        return;
    };
    let h = hooks_or_stubs();
    let _ = unsafe { (h.dict_op)(crate::hooks::DictOp::Clear as u32, bits) };
}

/// CPython `dict_merge` non-dict branch: `PyMapping_Keys(other)` then, per key,
/// `PyObject_GetItem(other, key)` → set into `op` honoring `override`.
unsafe fn dict_merge_from_mapping(
    op: *mut PyObject,
    other: *mut PyObject,
    override_: c_int,
) -> c_int {
    let keys = unsafe { crate::api::abstract_mapping::PyMapping_Keys(other) };
    if keys.is_null() {
        // PyMapping_Keys already set the exception (foreign mapping without a
        // usable keys(), or the runtime authority is unavailable). Fail loud.
        return -1;
    }
    let n = unsafe { crate::api::sequences::PyList_Size(keys) };
    if n < 0 {
        unsafe { crate::api::errors::release_preserving_error(&[keys]) };
        return -1;
    }
    let mut rc = 0;
    for i in 0..n {
        let key = unsafe { crate::api::sequences::PyList_GetItem(keys, i) };
        if key.is_null() {
            rc = -1;
            break;
        }
        let contains = if override_ != 1 {
            unsafe { PyDict_Contains(op, key) }
        } else {
            0
        };
        if contains < 0 {
            rc = -1;
            break;
        }
        if override_ != 1 && contains == 1 {
            if override_ == 2 {
                unsafe {
                    crate::api::errors::PyErr_SetObject(
                        (&raw mut crate::abi_types::PyExc_KeyError)
                            .cast::<crate::abi_types::PyObject>(),
                        key,
                    );
                }
                rc = -1;
                break;
            }
            continue;
        }
        let value = unsafe { crate::api::object::PyObject_GetItem(other, key) };
        if value.is_null() {
            rc = -1;
            break;
        }
        let set_rc = unsafe { PyDict_SetItem(op, key, value) };
        unsafe { crate::api::refcount::Py_DECREF(value) };
        if set_rc != 0 {
            rc = -1;
            break;
        }
    }
    unsafe { crate::api::refcount::Py_DECREF(keys) };
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDictProxy_New(mapping: *mut PyObject) -> *mut PyObject {
    if mapping.is_null() {
        bad_dict_argument();
        return ptr::null_mut();
    }
    // Constructor admission belongs to the runtime owner for both managed
    // and native mappings; the C projection is not a second protocol test.
    let Some(mapping) = (unsafe { RuntimeValue::acquire(mapping) }) else {
        return ptr::null_mut();
    };
    let result = unsafe { (hooks_or_stubs().mappingproxy_new)(mapping.bits()) };
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItem(op: *mut PyObject, key: *mut PyObject) -> *mut PyObject {
    crate::api::errors::with_preserved_error(|| unsafe { PyDict_GetItemWithError(op, key) })
}

/// Like [`PyDict_GetItem`] but does NOT suppress errors: a non-dict receiver
/// sets `PyErr_BadInternalCall` (CPython), and only a genuinely absent key
/// returns NULL with no exception. Runtime lookup failures transfer their exact
/// pending instance into CURRENT_EXC before the C sentinel is returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemWithError(
    op: *mut PyObject,
    key: *mut PyObject,
) -> *mut PyObject {
    unsafe { dict_get_with_hash(op, key, crate::hooks::DictHashSource::Compute, 0) }
}

unsafe fn dict_get_with_hash(
    op: *mut PyObject,
    key: *mut PyObject,
    hash_source: crate::hooks::DictHashSource,
    hash: i64,
) -> *mut PyObject {
    if key.is_null() {
        bad_dict_argument();
        return ptr::null_mut();
    }
    let Some(dict_bits) = require_dict(op) else {
        return ptr::null_mut();
    };
    let Some(key_value) = (unsafe { RuntimeValue::acquire(key) }) else {
        let _ = crate::api::errors::transfer_runtime_pending_to_current();
        return ptr::null_mut();
    };
    let h = hooks_or_stubs();
    let result = unsafe {
        GLOBAL_BRIDGE.borrowed_result_to_borrowed_pyobj((h.dict_get)(
            dict_bits,
            key_value.bits(),
            hash_source,
            hash,
        ))
    };
    crate::api::errors::with_preserved_error(|| drop(key_value));
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemRef(
    op: *mut PyObject,
    key: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    if result.is_null() {
        bad_dict_argument();
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    let value = unsafe { PyDict_GetItemWithError(op, key) };
    if value.is_null() {
        // CPython: NULL + pending exception is an error (-1); NULL with no
        // exception is genuine absence (0). The prior code collapsed both to 0.
        if !crate::api::errors::transfer_runtime_pending_to_current() {
            0
        } else {
            -1
        }
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
        bad_dict_argument();
        return -1;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if op.is_null() || key.is_null() {
        bad_dict_argument();
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyDict_GetItemRef(op, key_obj, result) };
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::refcount::Py_DECREF(key_obj)
    });
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyDict_GetItem_KnownHash(
    op: *mut PyObject,
    key: *mut PyObject,
    hash: crate::abi_types::Py_hash_t,
) -> *mut PyObject {
    unsafe { dict_get_with_hash(op, key, crate::hooks::DictHashSource::Supplied, hash as i64) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_GetItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> *mut PyObject {
    crate::api::errors::with_preserved_error(|| unsafe {
        if op.is_null() || key.is_null() {
            return ptr::null_mut();
        }
        let key_obj = crate::api::strings::PyUnicode_FromString(key);
        if key_obj.is_null() {
            return ptr::null_mut();
        }
        let result = PyDict_GetItemWithError(op, key_obj);
        crate::api::refcount::Py_DECREF(key_obj);
        result
    })
}

/// CPython private `_PyDict_GetItemStringWithError(v, key)` (Objects/dictobject.c):
/// like [`PyDict_GetItemString`] but does NOT suppress errors — a non-dict `v`
/// sets `PyErr_BadInternalCall`, while a genuinely absent key returns NULL with
/// no exception. numpy links this private form. Builds the str key and routes
/// through [`PyDict_GetItemWithError`], returning a BORROWED reference.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyDict_GetItemStringWithError(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> *mut PyObject {
    if op.is_null() || key.is_null() {
        bad_dict_argument();
        return ptr::null_mut();
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return ptr::null_mut();
    }
    let result = unsafe { PyDict_GetItemWithError(op, key_obj) };
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::refcount::Py_DECREF(key_obj)
    });
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_SetDefault(
    op: *mut PyObject,
    key: *mut PyObject,
    default_value: *mut PyObject,
) -> *mut PyObject {
    if op.is_null() || key.is_null() || default_value.is_null() {
        bad_dict_argument();
        return ptr::null_mut();
    }
    let existing = unsafe { PyDict_GetItemWithError(op, key) };
    if !existing.is_null() {
        return existing;
    }
    if crate::api::errors::transfer_runtime_pending_to_current() {
        return ptr::null_mut();
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
    // Unlike GetItemRef, this result sink is optional. A caller that only
    // needs the found/inserted status must not acquire an extra C reference.
    if !result.is_null() {
        unsafe { *result = ptr::null_mut() };
    }
    if op.is_null() || key.is_null() || default_value.is_null() {
        bad_dict_argument();
        return -1;
    }
    let existing = unsafe { PyDict_GetItemWithError(op, key) };
    if !existing.is_null() {
        if !result.is_null() {
            unsafe {
                crate::api::refcount::Py_INCREF(existing);
                *result = existing;
            }
        }
        return 1;
    }
    if crate::api::errors::transfer_runtime_pending_to_current() {
        return -1;
    }
    if unsafe { PyDict_SetItem(op, key, default_value) } != 0 {
        return -1;
    }
    if !result.is_null() {
        unsafe {
            crate::api::refcount::Py_INCREF(default_value);
            *result = default_value;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_DelItem(op: *mut PyObject, key: *mut PyObject) -> c_int {
    unsafe { dict_mutate(op, key, ptr::null_mut(), true, None, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_DelItemString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> c_int {
    if op.is_null() || key.is_null() {
        bad_dict_argument();
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyDict_DelItem(op, key_obj) };
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::refcount::Py_DECREF(key_obj)
    });
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Size(op: *mut PyObject) -> Py_ssize_t {
    // CPython: `if (!PyDict_Check(mp)) { PyErr_BadInternalCall(); return -1; }` —
    // a non-dict / NULL yields -1 with SystemError, never a fabricated 0 (which
    // `PyDict_Merge` used to read as "empty", silently treating a non-dict as
    // mergeable).
    match require_dict(op) {
        Some(bits) => {
            let h = hooks_or_stubs();
            unsafe { (h.dict_len)(bits) as Py_ssize_t }
        }
        None => -1,
    }
}

/// Real dict iteration over a Molt-native dict.
///
/// Backed by the allocation-free O(1) [`crate::hooks::RuntimeHooks::dict_entry`]
/// cursor (indexes the runtime dict's flat entry vector by `*pos`), so a
/// `while (PyDict_Next(d, &pos, &k, &v))` loop yields every entry with **borrowed**
/// key/value refs and returns `0` only at the true end — with **no** exception,
/// matching CPython. The previous stub observed an empty dict and left a stray
/// pending `RuntimeError`, breaking exactly the walks numpy runs during type/module
/// init.
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
    let dict_bits = match resolve_dict(op, false) {
        Ok(Some(bits)) => bits,
        Ok(None) | Err(()) => return 0,
    };
    let index = unsafe { *pos };
    if index < 0 {
        return 0;
    }
    let h = hooks_or_stubs();
    let mut key_bits: u64 = 0;
    let mut val_bits: u64 = 0;
    let found = unsafe {
        (h.dict_entry)(
            dict_bits,
            index as usize,
            &raw mut key_bits,
            &raw mut val_bits,
        )
    };
    if found != 1 {
        // End of iteration (or non-dict): return 0 and set NO exception.
        return 0;
    }
    unsafe {
        *pos = index + 1;
    }
    // Borrowed references (CPython contract — caller must not DECREF). O(1) and
    // allocation-free per step for keys/values already anchored in the bridge
    // (the common init path, where every entry entered via PyDict_SetItem).
    let bridge = &*GLOBAL_BRIDGE;
    if !key.is_null() {
        unsafe { *key = bridge.handle_to_borrowed_pyobj(key_bits) };
    }
    if !value.is_null() {
        unsafe { *value = bridge.handle_to_borrowed_pyobj(val_bits) };
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Contains(op: *mut PyObject, key: *mut PyObject) -> c_int {
    let value = unsafe { PyDict_GetItemWithError(op, key) };
    if !value.is_null() {
        return 1;
    }
    // CPython: a genuine absent key returns 0, but any error (non-dict receiver
    // via BadInternalCall, or a lookup error) returns -1 with the exception left
    // pending — never a silent 0 that a caller reads as "not present".
    if !crate::api::errors::transfer_runtime_pending_to_current() {
        0
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_ContainsString(
    op: *mut PyObject,
    key: *const std::os::raw::c_char,
) -> c_int {
    if op.is_null() || key.is_null() {
        bad_dict_argument();
        return -1;
    }
    let key_obj = unsafe { crate::api::strings::PyUnicode_FromString(key) };
    if key_obj.is_null() {
        return -1;
    }
    let rc = unsafe { PyDict_Contains(op, key_obj) };
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::refcount::Py_DECREF(key_obj)
    });
    rc
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Check(op: *mut PyObject) -> c_int {
    unsafe {
        crate::bridge::is_semantic_instance_of(op, &raw mut crate::abi_types::PyDict_Type) as c_int
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_CheckExact(op: *mut PyObject) -> c_int {
    unsafe {
        crate::bridge::is_exact_semantic_type(op, &raw mut crate::abi_types::PyDict_Type) as c_int
    }
}

/// Dict's declaring slot validates dict peers and equality operations before
/// invoking the runtime dict comparison authority. Native foreign dict storage
/// is not admitted by this ABI's dict accessors; keep that failure explicit.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_dict_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    use molt_lang_obj_model::sequence_compare::RichCompareOp;
    if !RichCompareOp::from_i32(op).is_some_and(RichCompareOp::is_equality)
        || unsafe { PyDict_Check(v) } == 0
        || unsafe { PyDict_Check(w) } == 0
    {
        let ni = &raw mut crate::abi_types::Py_NotImplementedSentinel;
        unsafe { crate::api::refcount::Py_INCREF(ni) };
        return ni;
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let Some(left) = require_dict(v) else {
        return ptr::null_mut();
    };
    let Some(right) = require_dict(w) else {
        return ptr::null_mut();
    };
    unsafe {
        crate::api::typeobj::declaring_richcompare(
            &raw mut crate::abi_types::PyDict_Type,
            left,
            right,
            op,
        )
    }
}
/// Dispatch a dict-iteration op through the runtime dict authority.
///
/// Ignoring `op` and returning an empty dict/list (the previous behavior) is
/// silent data loss — every `dict.copy()`, `.keys()`, `.values()` from an
/// extension came back empty. Route to the runtime, which reads the real dict.
unsafe fn dict_op(op: crate::hooks::DictOp, dict: *mut PyObject) -> *mut PyObject {
    let Some(bits) = require_dict(dict) else {
        return ptr::null_mut();
    };
    unsafe { dispatch_dict_op(op, bits) }
}

/// Mapping queries retain the original receiver for dynamic method dispatch.
pub(crate) unsafe fn mapping_op(op: crate::hooks::DictOp, object: *mut PyObject) -> *mut PyObject {
    let Some(value) = (unsafe { RuntimeValue::acquire(object) }) else {
        return ptr::null_mut();
    };
    let result = unsafe { dispatch_dict_op(op, value.bits()) };
    crate::api::errors::with_preserved_error(|| drop(value));
    result
}

unsafe fn dispatch_dict_op(op: crate::hooks::DictOp, bits: u64) -> *mut PyObject {
    let result = unsafe { (hooks_or_stubs().dict_op)(op as u32, bits) };
    if result == 0 {
        bad_dict_argument();
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(result) }
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

#[cfg(test)]
mod dict_anchor_tests {
    use super::*;

    /// `_PyDict_GetItemStringWithError` is the error-propagating variant: a NULL
    /// key is a bad internal call (sets an exception), never a silent NULL. The
    /// found/absent dict-lookup paths are exercised end-to-end by the discovery
    /// engine (they need the runtime `dict` hooks); here we pin the guard that
    /// distinguishes it from the error-suppressing `PyDict_GetItemString`.
    #[test]
    fn getitemstring_witherror_null_key_is_bad_internal_call() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        crate::bridge::init_tag_table();
        unsafe {
            crate::api::errors::PyErr_Clear();
            let recv = GLOBAL_BRIDGE.owned_handle_to_pyobj(MoltObject::from_int(0xD1C7).bits());
            let got = _PyDict_GetItemStringWithError(recv, ptr::null());
            assert!(got.is_null(), "NULL key must yield NULL");
            assert!(
                !crate::api::errors::PyErr_Occurred().is_null(),
                "NULL key must set an exception (bad internal call), not a silent NULL"
            );
            crate::api::errors::PyErr_Clear();
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDict_Pop(
    op: *mut PyObject,
    key: *mut PyObject,
    result: *mut *mut PyObject,
) -> c_int {
    use crate::api::refcount::OwnedPyObject;
    if !result.is_null() {
        unsafe { *result = ptr::null_mut() };
    }
    if op.is_null() || key.is_null() {
        bad_dict_argument();
        return -1;
    }
    let dict = unsafe { OwnedPyObject::from_borrowed(op) };
    let key = unsafe { OwnedPyObject::from_borrowed(key) };
    let Some(bits) = require_dict(dict.as_ptr()) else {
        return -1;
    };
    let Some(key_value) = (unsafe { RuntimeValue::acquire(key.as_ptr()) }) else {
        return -1;
    };
    match unsafe { (hooks_or_stubs().dict_pop)(bits, key_value.bits()) }.decode() {
        crate::hooks::DecodedHandleResult::Missing => 0,
        crate::hooks::DecodedHandleResult::Error => {
            if !crate::api::errors::transfer_runtime_pending_to_current() {
                bad_dict_argument();
            }
            -1
        }
        crate::hooks::DecodedHandleResult::Ok(bits) => {
            let value = unsafe { RuntimeValue::from_owned(bits) };
            if !result.is_null() {
                let object =
                    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(value.into_owned_bits()) };
                if object.is_null() {
                    return -1;
                }
                unsafe { *result = object };
            }
            1
        }
    }
}
