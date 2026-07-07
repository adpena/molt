//! Tests for PyDict_* mapping API.

#![allow(non_snake_case)]

use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// A hook table whose `alloc_list` SUCCEEDS (so PyList_New returns a non-null
// empty list) while `dict_op` stays stubbed (returns 0). This makes the fail-open
// placeholder (`PyList_New(0)`) observably DIFFERENT from the real routing
// (`PyDict_Items` -> dict_op -> fail closed NULL): the mutation-proof for the
// Items burndown depends on that difference.
static LIST_HANDLE: AtomicU64 = AtomicU64::new(0x6100_0000);

unsafe extern "C" fn fake_alloc_list() -> u64 {
    LIST_HANDLE.fetch_add(0x10, Ordering::Relaxed)
}

fn init_with_working_list_alloc() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_list = fake_alloc_list;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

// ---------------------------------------------------------------------------
// PyDict_New
// ---------------------------------------------------------------------------

#[test]
fn test_dict_new_fails_closed_on_alloc_failure() {
    // F4 teeth: with stub hooks, alloc_dict returns 0 (allocation failure).
    // PyDict_New MUST fail closed with NULL + a set MemoryError, NOT a non-NULL
    // Py_None placeholder that defeats the caller's `if (dict == NULL)` guard.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    assert!(
        py.is_null(),
        "PyDict_New must return NULL on alloc failure, not a placeholder"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL return from PyDict_New must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_dict_copy_keys_values_fail_closed_without_runtime() {
    // F6 teeth: PyDict_Copy/Keys/Values route through the runtime dict
    // authority. They must NOT ignore their argument and return an empty
    // dict/list (silent data loss). Without runtime hooks the dict_op hook
    // returns 0, so each must fail closed with NULL + an exception, never a
    // fabricated empty result.
    init();
    type DictOpFn = unsafe extern "C" fn(
        *mut molt_cpython_abi::abi_types::PyObject,
    ) -> *mut molt_cpython_abi::abi_types::PyObject;
    let ops: [DictOpFn; 3] = [
        molt_cpython_abi::api::mapping::PyDict_Copy,
        molt_cpython_abi::api::mapping::PyDict_Keys,
        molt_cpython_abi::api::mapping::PyDict_Values,
    ];
    for op in ops {
        // Clear first so we prove THIS op set the exception, not a prior one.
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
        let result = unsafe { op(ptr::null_mut()) };
        assert!(
            result.is_null(),
            "PyDict copy/keys/values must fail closed (NULL), not return empty"
        );
        assert!(
            !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
            "a NULL return must leave an exception set"
        );
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    }
}

#[test]
fn test_dict_items_fails_closed_without_runtime() {
    // F6 teeth (fail-open burndown): PyDict_Items routes through the runtime dict
    // authority (DictOp::Items). The dict_op hook returns 0 here, so it must fail
    // closed with NULL + an exception, never a fabricated empty list.
    //
    // The hook table gives alloc_list a WORKING allocator on purpose: it makes
    // this test distinguish the real routing from the old `PyList_New(0)`
    // placeholder. If PyDict_Items regressed to returning an empty list, that list
    // would now allocate to a NON-null value and this assertion would fail —
    // giving the burndown real mutation teeth (a fail-closed-under-stubs-only test
    // cannot tell the two apart, since PyList_New(0) itself fails closed there).
    init_with_working_list_alloc();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_Items(ptr::null_mut()) };
    assert!(
        result.is_null(),
        "PyDict_Items must fail closed (NULL), not return an (allocatable) empty list"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL return from PyDict_Items must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_mapping_items_fails_closed_without_runtime() {
    // PyMapping_Items delegated to an empty-list placeholder before the burndown
    // (silent data loss). It now routes through PyDict_Items -> runtime authority
    // and must fail closed with NULL + an exception. alloc_list works here so a
    // regression to the PyList_New(0) placeholder would return non-null and be
    // caught (mutation teeth) — see test_dict_items_fails_closed_without_runtime.
    init_with_working_list_alloc();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let result =
        unsafe { molt_cpython_abi::api::abstract_mapping::PyMapping_Items(ptr::null_mut()) };
    assert!(
        result.is_null(),
        "PyMapping_Items must fail closed (NULL), not return an (allocatable) empty list"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL return from PyMapping_Items must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyDict_SetItem — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_setitem_null_dict_returns_error() {
    init();
    let key = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(ptr::null_mut(), key, val) };
    assert_eq!(result, -1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(key);
        molt_cpython_abi::api::refcount::Py_DECREF(val);
    }
}

#[test]
fn test_dict_setitem_null_key_returns_error() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(dict, ptr::null_mut(), val) };
    assert_eq!(result, -1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(val);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}

#[test]
fn test_dict_setitem_null_value_returns_error() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let key = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(dict, key, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(key);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}

#[test]
fn test_dict_setitem_all_null_returns_error() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_SetItem(
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert_eq!(result, -1);
}

// ---------------------------------------------------------------------------
// PyDict_SetItemString — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_setitemstring_null_dict_returns_error() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_SetItemString(ptr::null_mut(), c"key".as_ptr(), val)
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(val) };
}

#[test]
fn test_dict_setitemstring_null_key_returns_error() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItemString(dict, ptr::null(), val) };
    assert_eq!(result, -1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(val);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}

#[test]
fn test_dict_setitemstring_null_value_returns_error() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_SetItemString(dict, c"key".as_ptr(), ptr::null_mut())
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(dict) };
}

// ---------------------------------------------------------------------------
// PyDict_GetItem — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_getitem_null_dict_returns_null() {
    init();
    let key = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(ptr::null_mut(), key) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(key) };
}

#[test]
fn test_dict_getitem_null_key_returns_null() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(dict, ptr::null_mut()) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(dict) };
}

#[test]
fn test_dict_getitem_both_null_returns_null() {
    init();
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(ptr::null_mut(), ptr::null_mut()) };
    assert!(result.is_null());
}

// ---------------------------------------------------------------------------
// PyDict_GetItemString — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_getitemstring_null_dict_returns_null() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_GetItemString(ptr::null_mut(), c"key".as_ptr())
    };
    assert!(result.is_null());
}

#[test]
fn test_dict_getitemstring_null_key_returns_null() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_GetItemString(dict, ptr::null()) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(dict) };
}

// ---------------------------------------------------------------------------
// PyDict_DelItemString — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_delitemstring_null_dict_returns_error() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_DelItemString(ptr::null_mut(), c"key".as_ptr())
    };
    assert_eq!(result, -1);
}

#[test]
fn test_dict_delitemstring_null_key_returns_error() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_DelItemString(dict, ptr::null()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(dict) };
}

// ---------------------------------------------------------------------------
// PyDict_Size — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_size_null_returns_zero() {
    init();
    let size = unsafe { molt_cpython_abi::api::mapping::PyDict_Size(ptr::null_mut()) };
    assert_eq!(size, 0);
}

// ---------------------------------------------------------------------------
// PyDict_Check
// ---------------------------------------------------------------------------

#[test]
fn test_dict_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_dict_check_on_int_returns_zero() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyDict_Copy
// ---------------------------------------------------------------------------

// PyDict_Copy / PyDict_Keys / PyDict_Values fail-closed behavior without a
// registered runtime is proved by `test_dict_copy_keys_values_fail_closed_without_runtime`
// above. Their real (non-empty) results require the runtime dict authority and
// are exercised by the runtime-side / differential integration tests, not by
// these stub-only unit tests.
