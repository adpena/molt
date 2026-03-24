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
fn test_list_new_returns_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::sequences::PyList_New(0) };
    // With stub hooks, alloc_list returns 0 => fallback to None placeholder
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_list_new_with_size() {
    init();
    let py = unsafe { molt_cpython_abi::api::sequences::PyList_New(5) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
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
    let result =
        unsafe { molt_cpython_abi::api::sequences::PyList_Append(list, ptr::null_mut()) };
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
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(val) };
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
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(val);
        molt_cpython_abi::api::refcount::Py_DECREF(list);
    }
}

// ---------------------------------------------------------------------------
// PyList_Size / PyList_GET_SIZE — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_list_size_null_returns_zero() {
    init();
    let size = unsafe { molt_cpython_abi::api::sequences::PyList_Size(ptr::null_mut()) };
    assert_eq!(size, 0);
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
fn test_tuple_new_negative_size_clamps_to_zero() {
    init();
    let py = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(-5) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
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
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(val) };
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
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(val);
        molt_cpython_abi::api::refcount::Py_DECREF(tup);
    }
}

// ---------------------------------------------------------------------------
// PyTuple_Size / PyTuple_GET_SIZE — null safety
// ---------------------------------------------------------------------------

#[test]
fn test_tuple_size_null_returns_zero() {
    init();
    let size = unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(ptr::null_mut()) };
    assert_eq!(size, 0);
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
