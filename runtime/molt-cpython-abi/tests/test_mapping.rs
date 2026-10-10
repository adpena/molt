//! Tests for PyDict_* mapping API.

#![allow(non_snake_case)]

mod support;

use std::ptr;
// Every test in this binary uses one process-owned hook profile. Lists have
// the shared ownership-bearing fixture storage; dictionaries fail closed.
fn init() -> support::AbiTestThreadStateTransaction {
    let mut hooks = support::stub_runtime_hooks();
    support::fake_runtime::wire_sequences(&mut hooks);
    hooks.alloc_dict = molt_cpython_abi::hooks::STUB_HOOKS.alloc_dict;
    hooks.dict_op = molt_cpython_abi::hooks::STUB_HOOKS.dict_op;
    support::enter_runtime_class_abi_test(hooks)
}

fn assert_empty_list_allocation_works() {
    unsafe {
        let list = molt_cpython_abi::api::sequences::PyList_New(0);
        assert!(
            !list.is_null(),
            "the empty-list placeholder must be distinguishable"
        );
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        assert_eq!(molt_cpython_abi::api::sequences::PyList_Size(list), 0);
        molt_cpython_abi::api::refcount::Py_DECREF(list);
    }
}

// ---------------------------------------------------------------------------
// PyDict_New
// ---------------------------------------------------------------------------

#[test]
fn test_dict_new_fails_closed_on_alloc_failure() {
    // F4 teeth: with the selected hooks, alloc_dict returns 0 (allocation failure).
    // PyDict_New MUST fail closed with NULL + a set MemoryError, NOT a non-NULL
    // Py_None placeholder that defeats the caller's `if (dict == NULL)` guard.
    let _abi_test = init();
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
    // NULL input must fail closed with NULL + an exception, never a
    // fabricated empty dict/list. Valid-mapping dispatch is exercised by the
    // dictionary protocol fixtures; this test owns the NULL-input boundary.
    let _abi_test = init();
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
    // PyDict_Items(NULL) must fail closed with NULL + an exception, never a
    // fabricated empty list. This proves NULL handling, not valid-dict dispatch.
    //
    // The hook table gives alloc_list_presized a WORKING allocator on purpose: it makes
    // this test distinguish the real routing from the old `PyList_New(0)`
    // placeholder. If PyDict_Items regressed to returning an empty list, that list
    // would now allocate to a NON-null value and this assertion would fail —
    // giving the burndown real mutation teeth (a fail-closed-under-stubs-only test
    // cannot tell the two apart, since PyList_New(0) itself fails closed there).
    let _abi_test = init();
    assert_empty_list_allocation_works();
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
    // (silent data loss). NULL input must fail closed with an exception.
    // The real list fixture works here, so a
    // regression to the PyList_New(0) placeholder would return non-null and be
    // caught (mutation teeth) — see test_dict_items_fails_closed_without_runtime.
    let _abi_test = init();
    assert_empty_list_allocation_works();
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
    let _abi_test = init();
    let key = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(ptr::null_mut(), key, val) };
    assert_eq!(result, -1);
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "PyDict_SetItem(NULL, ...) must set an exception with its -1 sentinel"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(key);
        molt_cpython_abi::api::refcount::Py_DECREF(val);
    }
}

#[test]
fn test_dict_setitem_null_key_returns_error() {
    let _abi_test = init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(dict, ptr::null_mut(), val) };
    assert_eq!(result, -1);
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "PyDict_SetItem(..., NULL, ...) must set an exception"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(val);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}

#[test]
fn test_dict_setitem_null_value_returns_error() {
    let _abi_test = init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let key = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(dict, key, ptr::null_mut()) };
    assert_eq!(result, -1);
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "PyDict_SetItem(..., NULL) must set an exception"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(key);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}

#[test]
fn test_dict_setitem_all_null_returns_error() {
    let _abi_test = init();
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_SetItem(
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert_eq!(result, -1);
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "PyDict_SetItem(NULL, NULL, NULL) must set an exception"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyDict_SetItemString — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_setitemstring_null_dict_returns_error() {
    let _abi_test = init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_SetItemString(ptr::null_mut(), c"key".as_ptr(), val)
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(val) };
}

#[test]
fn test_dict_setitemstring_null_key_returns_error() {
    let _abi_test = init();
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
    let _abi_test = init();
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
    let _abi_test = init();
    let key = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(ptr::null_mut(), key) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(key) };
}

#[test]
fn test_dict_getitem_null_key_returns_null() {
    let _abi_test = init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(dict, ptr::null_mut()) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(dict) };
}

#[test]
fn test_dict_getitem_both_null_returns_null() {
    let _abi_test = init();
    let result =
        unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(ptr::null_mut(), ptr::null_mut()) };
    assert!(result.is_null());
}

// ---------------------------------------------------------------------------
// PyDict_GetItemString — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_getitemstring_null_dict_returns_null() {
    let _abi_test = init();
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_GetItemString(ptr::null_mut(), c"key".as_ptr())
    };
    assert!(result.is_null());
}

#[test]
fn test_dict_getitemstring_null_key_returns_null() {
    let _abi_test = init();
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
    let _abi_test = init();
    let result = unsafe {
        molt_cpython_abi::api::mapping::PyDict_DelItemString(ptr::null_mut(), c"key".as_ptr())
    };
    assert_eq!(result, -1);
}

#[test]
fn test_dict_delitemstring_null_key_returns_error() {
    let _abi_test = init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_DelItemString(dict, ptr::null()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(dict) };
}

// ---------------------------------------------------------------------------
// PyDict_Size — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_dict_size_null_sets_error_and_returns_minus_one() {
    // CPython: PyDict_Size(non-dict/NULL) → PyErr_BadInternalCall() + return -1,
    // NOT a fabricated 0 (which PyDict_Merge read as "empty"). Sentinel sweep.
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let size = unsafe { molt_cpython_abi::api::mapping::PyDict_Size(ptr::null_mut()) };
    assert_eq!(size, -1, "PyDict_Size(NULL) must be -1, not a fabricated 0");
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a -1 return from PyDict_Size must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyDict_Check
// ---------------------------------------------------------------------------

#[test]
fn test_dict_check_null_returns_zero() {
    let _abi_test = init();
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_dict_check_on_int_returns_zero() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyDict_Copy
// ---------------------------------------------------------------------------

// PyDict_Copy / PyDict_Keys / PyDict_Values fail-closed behavior without dictionary
// capabilities is proved by `test_dict_copy_keys_values_fail_closed_without_runtime`
// above. Their real (non-empty) results require the runtime dict authority and
// are exercised by the runtime-side / differential integration tests, not by
// these dictionary-failure fixture tests.
//
// PyDict_Next / PyDict_Merge real-iteration teeth (which need a fake dict model
// whose `dict_next`/`dict_mutate` hooks conflict with this file's first-wins hook
// OnceLock) live in their own binary, `tests/test_dict_cursor.rs`.
