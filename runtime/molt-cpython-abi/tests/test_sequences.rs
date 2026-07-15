//! Tests for PyList_* and PyTuple_* sequence API.

#![allow(non_snake_case)]

use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyList_New
// ---------------------------------------------------------------------------

#[test]
fn test_list_new_fails_closed_on_alloc_failure() {
    // F4 teeth: with stub hooks, alloc_list returns 0 (allocation failure).
    // PyList_New MUST fail closed with NULL + a set MemoryError, NOT return a
    // non-NULL Py_None placeholder (which would defeat the caller's
    // `if (list == NULL)` guard and let it operate on None as a list).
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    assert!(
        py.is_null(),
        "PyList_New must return NULL on alloc failure, not a placeholder"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL return from PyList_New must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_list_new_with_size_fails_closed_on_alloc_failure() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::sequences::PyList_New(5) };
    assert!(py.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_set_new_fails_closed() {
    // F5 teeth: PySet_New must fail closed (NULL + NotImplementedError) rather
    // than return a *list* with list semantics (no dedup, no membership).
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::sequences::PySet_New(ptr::null_mut()) };
    assert!(
        py.is_null(),
        "PySet_New must fail closed, not return a list"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_set_membership_ops_fail_closed() {
    // F5 teeth: PySet_Contains/Add/Discard must return the error sentinel (-1)
    // with an exception set, NOT fake success/absence (0).
    init();
    for result in [
        unsafe {
            molt_cpython_abi::api::sequences::PySet_Contains(ptr::null_mut(), ptr::null_mut())
        },
        unsafe { molt_cpython_abi::api::sequences::PySet_Add(ptr::null_mut(), ptr::null_mut()) },
        unsafe {
            molt_cpython_abi::api::sequences::PySet_Discard(ptr::null_mut(), ptr::null_mut())
        },
    ] {
        assert_eq!(result, -1, "set op must fail closed with -1, not fake 0");
    }
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PySet_Size(ptr::null_mut()) },
        -1,
        "PySet_Size must fail closed with -1, not fake 0"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyList_Append — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_append_null_list_returns_error() {
    init();
    let item = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_Append(ptr::null_mut(), item) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(item) };
}

#[test]
fn test_list_append_null_item_returns_error() {
    init();
    let list = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_Append(list, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn test_list_append_both_null_returns_error() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::sequences::PyList_Append(ptr::null_mut(), ptr::null_mut())
    };
    assert_eq!(result, -1);
}

#[test]
fn test_list_append_rejects_non_list() {
    init();
    let tuple = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    let item = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyList_Append(tuple, item) },
        -1
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(item);
        molt_cpython_abi::api::refcount::Py_DECREF(tuple);
    }
}

// ---------------------------------------------------------------------------
// PyList_GetItem / PyList_GET_ITEM — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_getitem_null_returns_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_GetItem(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

#[test]
fn test_list_get_item_negative_index_returns_null() {
    init();
    let list = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_GET_ITEM(list, -1) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn test_list_get_item_ref_null_returns_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_GetItemRef(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

// ---------------------------------------------------------------------------
// PyList_SetItem — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_setitem_null_list_returns_error() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result =
        unsafe { molt_cpython_abi::api::sequences::PyList_SetItem(ptr::null_mut(), 0, val) };
    assert_eq!(result, -1);
    // `PyList_SetItem` steals `val` even when the container is not a list (it
    // `Py_XDECREF`s the item before `PyErr_BadInternalCall`, matching CPython);
    // decref'ing `val` again here would be a double-free.
}

#[test]
fn test_list_setitem_null_value_returns_error() {
    init();
    let list = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    let result =
        unsafe { molt_cpython_abi::api::sequences::PyList_SetItem(list, 0, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn test_list_setitem_negative_index_returns_error() {
    init();
    let list = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_SetItem(list, -1, val) };
    assert_eq!(result, -1);
    // `PyList_SetItem` STEALS the item reference even on the out-of-range error
    // path (it `Py_XDECREF`s `val` before returning -1, matching CPython's
    // `listobject.c`). The caller therefore must NOT decref `val` again — doing
    // so is a double-free (a use-after-free Miri catches at `refcount.rs`).
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(list);
    }
}

// ---------------------------------------------------------------------------
// PyList_Size / PyList_GET_SIZE — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_size_null_sets_error_and_returns_minus_one() {
    // CPython: PyList_Size(non-list/NULL) → PyErr_BadInternalCall() + -1, not a
    // fabricated 0 (sentinel sweep; same class as the PyDict_Size fix).
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let size = unsafe { molt_cpython_abi::api::sequences::PyList_Size(ptr::null_mut()) };
    assert_eq!(size, -1, "PyList_Size(NULL) must be -1, not a fabricated 0");
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a -1 return from PyList_Size must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_list_get_size_null_returns_zero() {
    init();
    let size = unsafe { molt_cpython_abi::api::sequences::PyList_GET_SIZE(ptr::null_mut()) };
    assert_eq!(size, 0);
}

// ---------------------------------------------------------------------------
// PyList_Check
// ---------------------------------------------------------------------------

#[test]
fn test_list_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_list_check_on_int_returns_zero() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyList_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyTuple_New
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_new_returns_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_tuple_new_with_size() {
    init();
    let py = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(3) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_tuple_new_negative_size_rejects_with_system_error() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(-5) };
    assert!(py.is_null());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError)
                    .cast::<molt_cpython_abi::abi_types::PyObject>(),
            )
        },
        1
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyTuple_GetItem / PyTuple_GET_ITEM — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_getitem_null_returns_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_GetItem(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

#[test]
fn test_tuple_get_item_negative_index_returns_null() {
    init();
    let tup = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(3) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_GET_ITEM(tup, -1) };
    assert!(result.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(tup) };
}

// ---------------------------------------------------------------------------
// PyTuple_SetItem — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_setitem_null_tuple_returns_error() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result =
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(ptr::null_mut(), 0, val) };
    assert_eq!(result, -1);
    // `PyTuple_SetItem` steals `val` even when the container is not a tuple (it
    // `Py_XDECREF`s the item before `PyErr_BadInternalCall`, matching CPython);
    // decref'ing `val` again here would be a double-free.
}

#[test]
fn test_tuple_setitem_null_value_returns_error() {
    init();
    let tup = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    let result =
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tup, 0, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(tup) };
}

#[test]
fn test_tuple_setitem_negative_index_returns_error() {
    init();
    let tup = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tup, -1, val) };
    assert_eq!(result, -1);
    // `PyTuple_SetItem` steals `val` on the out-of-range error path (it
    // `Py_XDECREF`s the item before returning -1, matching CPython); the caller
    // must NOT decref `val` again — that is a double-free.
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(tup);
    }
}

#[test]
fn test_tuple_setitem_rejects_shared_tuple() {
    init();
    let tup = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(1) };
    assert!(!tup.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(tup) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) };
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tup, 0, val) },
        -1,
        "published tuple mutation must fail once the tuple is shared"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    assert!(unsafe { molt_cpython_abi::api::sequences::PyTuple_GET_ITEM(tup, 0) }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(tup);
        molt_cpython_abi::api::refcount::Py_DECREF(tup);
    }
}

// ---------------------------------------------------------------------------
// PyTuple_Size / PyTuple_GET_SIZE — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_size_null_is_bad_internal_call() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let size = unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(ptr::null_mut()) };
    assert_eq!(size, -1);
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_tuple_get_size_null_returns_zero() {
    init();
    let size = unsafe { molt_cpython_abi::api::sequences::PyTuple_GET_SIZE(ptr::null_mut()) };
    assert_eq!(size, 0);
}

// ---------------------------------------------------------------------------
// PyTuple_Check
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_tuple_check_on_int_returns_zero() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::sequences::PyTuple_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_sequence_fast_items_returns_raw_tuple_storage() {
    init();
    let tuple = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(2) };
    let first = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) };
    let second = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(11) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tuple, 0, first) },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::sequences::PyTuple_SetItem(tuple, 1, second) },
        0
    );

    let fast =
        unsafe { molt_cpython_abi::api::abstract_sequence::PySequence_Fast(tuple, ptr::null()) };
    assert_eq!(fast, tuple);
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_sequence::PySequence_Fast_GET_SIZE(fast) },
        2
    );
    let items = unsafe { molt_cpython_abi::api::abstract_sequence::PySequence_Fast_ITEMS(fast) };
    assert!(!items.is_null());
    assert_eq!(unsafe { *items }, first);
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_sequence::PySequence_Fast_GET_ITEM(fast, 1) },
        second
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(fast);
        molt_cpython_abi::api::refcount::Py_DECREF(tuple);
    }
}
