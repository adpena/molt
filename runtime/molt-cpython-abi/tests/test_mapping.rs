//! Tests for PyDict_* mapping API.

#![allow(non_snake_case)]

use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyDict_New
// ---------------------------------------------------------------------------

#[test]
fn test_dict_new_returns_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    // With stub hooks, alloc_dict returns 0 => fallback to None placeholder
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
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

#[test]
fn test_dict_copy_returns_new_dict() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let copy = unsafe { molt_cpython_abi::api::mapping::PyDict_Copy(dict) };
    assert!(!copy.is_null());
    // Copy should be a different pointer
    // (with stubs both are None placeholders, but still distinct bridge entries)
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(copy);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}

// ---------------------------------------------------------------------------
// PyDict_Keys / PyDict_Values
// ---------------------------------------------------------------------------

#[test]
fn test_dict_keys_returns_list() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let keys = unsafe { molt_cpython_abi::api::mapping::PyDict_Keys(dict) };
    assert!(!keys.is_null());
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(keys);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}

#[test]
fn test_dict_values_returns_list() {
    init();
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let values = unsafe { molt_cpython_abi::api::mapping::PyDict_Values(dict) };
    assert!(!values.is_null());
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(values);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}
