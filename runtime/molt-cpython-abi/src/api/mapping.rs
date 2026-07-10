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
    let mut bridge = GLOBAL_BRIDGE.lock();
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
    let mut key_is_foreign = false;
    let key_bits = match bridge.pyobj_to_handle(key) {
        Some(b) => b,
        None => match unsafe { bridge.molt_value_for_pyobj(key) } {
            // A genuine C-extension object key (numpy uses a builtin dtype/DType
            // singleton — a pure C `PyArray_Descr` — as the key when registering
            // its descriptor maps right after readying the DType classes): give it
            // the same first-class `TYPE_ID_FOREIGN` custody the value path below
            // already mints, instead of dropping the key. The wrapper is cached by
            // C-pointer identity (`foreign_wrapper_for`), so a later insert/lookup
            // with the same singleton pointer resolves to the same handle.
            Some(b) => {
                key_is_foreign = true;
                b
            }
            None => {
                // The dict receiver already resolved, so we hold a well-formed
                // dict — a `PyDict_SetItem` key is a real object the extension
                // constructed or a canonical runtime data symbol, safe to classify
                // (`describe_*` guards null + resolves exception singletons by
                // address before any deref). Reaching here means no foreign wrapper
                // could be minted (runtime hooks not installed); fail loud with an
                // honest exception rather than a contentless -1.
                let detail = format!("unresolved key @ {:p}: {}", key, unsafe {
                    crate::abi_types::describe_unresolved_pyobject(key)
                });
                crate::capi_trace::record_silent_failure("PyDict_SetItem", Some(&detail));
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_SystemError,
                            c"PyDict_SetItem: key is not a bridge-managed object and no foreign wrapper could be minted"
                                .as_ptr(),
                        );
                    }
                }
                return -1;
            }
        },
    };
    let mut val_is_foreign = false;
    let val_bits = match bridge.pyobj_to_handle(value) {
        Some(b) => b,
        None => match unsafe { bridge.molt_value_for_pyobj(value) } {
            // A genuine C-extension object value (a numpy dtype instance): give
            // it a first-class `TYPE_ID_FOREIGN` wrapper so it can be stored in
            // the Molt dict, instead of failing "unresolved value".
            Some(b) => {
                val_is_foreign = true;
                b
            }
            None => {
                let detail = format!("unresolved value: {}", unsafe {
                    crate::abi_types::describe_unresolved_pyobject(value)
                });
                crate::capi_trace::record_silent_failure("PyDict_SetItem", Some(&detail));
                return -1;
            }
        },
    };
    drop(bridge);
    let h = hooks_or_stubs();
    unsafe { (h.dict_set)(dict_bits, key_bits, val_bits) };
    // CPython contract: the dict takes its OWN strong references to key and
    // value (PyDict_SetItem does not steal). The bridge equivalent anchors the
    // key/value proxies so the extension's balancing `Py_DECREF` of its
    // temporaries cannot sever the pointer↔handle mapping while the object
    // stays reachable from the runtime dict. Without this, numpy's
    // `npy_cpu_dispatch_tracer_init` pattern — `PyDict_New()` →
    // `PyDict_SetItemString(mod_dict, …)` → `Py_DECREF(reg_dict)` → cache the
    // borrowed pointer — left `cpu_dispatch_registry` unresolvable and every
    // later `PyDict_SetItemString(registry, "argmin"/"argmax", …)` failed
    // "unresolved dict". Entries removed on the Molt side keep their anchor (a
    // small header) — a deliberate CPython-semantics trade the raw-registry
    // bridging already makes.
    unsafe {
        // A bridge-proxy key/value is anchored by INCREF'ing the C object, exactly
        // like CPython (the dict takes its own reference). A foreign-wrapped
        // key/value already holds a strong reference on the C object (minted at
        // refcount 1, ownership transferred to this dict entry), so it must NOT
        // be INCREF'd again — that would leak the C object.
        if !key_is_foreign {
            crate::api::refcount::Py_INCREF(key);
        }
        if !val_is_foreign {
            crate::api::refcount::Py_INCREF(value);
        }
    }
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
        let detail = format!("key='{key_str}' value: {}", unsafe {
            crate::abi_types::describe_unresolved_pyobject(value)
        });
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

#[cfg(test)]
mod dict_anchor_tests {
    use super::*;
    use molt_lang_obj_model::MoltObject;

    /// Regression for the numpy `npy_cpu_dispatch_tracer_init` unresolved-dict
    /// class: `PyDict_SetItem` must anchor the key/value proxies (CPython's
    /// dict takes its own strong references — the API does not steal), so the
    /// extension's balancing `Py_DECREF` of its temporaries cannot sever the
    /// pointer↔handle mapping while the object stays reachable from the
    /// runtime dict. numpy caches the borrowed registry pointer and every later
    /// `PyDict_SetItemString(registry, "argmin"/"argmax", …)` failed
    /// "unresolved dict" without this.
    #[test]
    fn dict_set_item_anchors_key_and_value_proxies() {
        crate::bridge::init_tag_table();
        let (recv, key, val) = {
            let mut bridge = GLOBAL_BRIDGE.lock();
            unsafe {
                (
                    bridge.handle_to_pyobj(MoltObject::from_int(0xD1C7).bits()),
                    bridge.handle_to_pyobj(MoltObject::from_int(0x6EE7).bits()),
                    bridge.handle_to_pyobj(MoltObject::from_int(0x7A17).bits()),
                )
            }
        };
        assert_eq!(unsafe { PyDict_SetItem(recv, key, val) }, 0);
        // The extension's balancing DECREF of its temporaries…
        unsafe { crate::api::refcount::Py_DECREF(key) };
        unsafe { crate::api::refcount::Py_DECREF(val) };
        // …must leave the mapping intact (the dict's own reference anchors it).
        let bridge = GLOBAL_BRIDGE.lock();
        assert!(
            bridge.pyobj_to_handle(key).is_some(),
            "key mapping severed by balancing DECREF"
        );
        assert!(
            bridge.pyobj_to_handle(val).is_some(),
            "value mapping severed by balancing DECREF"
        );
    }

    /// Regression for the numpy `_multiarray_umath` DType-registration class:
    /// its `Py_mod_exec` slot uses a builtin dtype/DType singleton (a pure C
    /// `PyArray_Descr`, unknown to the bridge) as a `PyDict_SetItem` KEY right
    /// after readying the DType classes. `PyDict_SetItem` already minted a
    /// `TYPE_ID_FOREIGN` wrapper for an unresolved VALUE but dropped an
    /// unresolved KEY with a bare `-1` and no exception — the silent failure the
    /// exec slot propagated as "returned non-zero without setting an exception".
    /// The key must get the same foreign custody (stable pointer-keyed identity)
    /// and be accepted.
    #[test]
    fn dict_set_item_gives_foreign_custody_to_key() {
        crate::bridge::init_tag_table();
        // A genuine C-extension object (numpy dtype singleton stand-in) that the
        // bridge does not know, with a pre-installed foreign wrapper identity: a
        // first real crossing would have minted this via `foreign_wrapper_for`
        // (the runtime `foreign_new` hook is not linked in a pure-ABI test).
        let mut foreign_key = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let key_ptr = &raw mut foreign_key as *mut PyObject;
        let wrapper_bits = 0xF0DE_0000_0000_0010u64;
        let (recv, val) = {
            let mut bridge = GLOBAL_BRIDGE.lock();
            bridge.insert_foreign_for_test(key_ptr, wrapper_bits);
            unsafe {
                (
                    bridge.handle_to_pyobj(MoltObject::from_int(0xD1C7).bits()),
                    bridge.handle_to_pyobj(MoltObject::from_int(0x7A17).bits()),
                )
            }
        };
        unsafe { crate::api::errors::PyErr_Clear() };
        // Before the fix this returned -1 with NO exception set (the silent
        // failure that stranded the exec slot). After the fix the key is given
        // foreign custody and accepted.
        let rc = unsafe { PyDict_SetItem(recv, key_ptr, val) };
        assert_eq!(rc, 0, "foreign C-extension object rejected as a dict key");
        assert!(
            unsafe { crate::api::errors::PyErr_Occurred() }.is_null(),
            "foreign-key path must not leave a stray pending exception",
        );
        // The wrapper keeps its stable pointer-keyed identity, so a later
        // insert/lookup with the same singleton pointer resolves to the same
        // handle rather than silently missing.
        {
            let mut bridge = GLOBAL_BRIDGE.lock();
            assert_eq!(
                unsafe { bridge.handle_to_pyobj(wrapper_bits) },
                key_ptr,
                "foreign key wrapper lost its round-trip identity",
            );
        }
        unsafe { GLOBAL_BRIDGE.lock().release_foreign(key_ptr as usize) };
    }
}
