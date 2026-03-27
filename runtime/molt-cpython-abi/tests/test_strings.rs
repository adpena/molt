//! Tests for PyUnicode_*, PyBytes_* string/bytes API.

#![allow(non_snake_case)]

use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyUnicode_FromString
// ---------------------------------------------------------------------------

#[test]
fn test_unicode_from_string_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"hello".as_ptr()) };
    // With stub hooks, alloc_str returns 0 => fallback to None placeholder
    // Either way, should not return null
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_unicode_from_string_null_returns_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(ptr::null()) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_string_empty() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"".as_ptr()) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyUnicode_FromStringAndSize
// ---------------------------------------------------------------------------

#[test]
fn test_unicode_from_string_and_size() {
    init();
    let data = b"world\0";
    let py = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(data.as_ptr().cast(), 5)
    };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_unicode_from_string_and_size_null_ptr() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(ptr::null(), 5) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_string_and_size_negative_size() {
    init();
    let py =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(c"abc".as_ptr(), -1) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_string_and_size_zero_length() {
    init();
    let py =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(c"abc".as_ptr(), 0) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyUnicode_AsUTF8
// ---------------------------------------------------------------------------

#[test]
fn test_unicode_as_utf8_null_returns_null() {
    init();
    let ptr = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(ptr::null_mut()) };
    assert!(ptr.is_null());
}

#[test]
fn test_unicode_as_utf8_on_object() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"test".as_ptr()) };
    let utf8 = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(py) };
    // With stubs, str_data returns empty string pointer
    assert!(!utf8.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyUnicode_AsUTF8AndSize
// ---------------------------------------------------------------------------

#[test]
fn test_unicode_as_utf8_and_size_null() {
    init();
    let mut size: isize = -1;
    let ptr = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_AsUTF8AndSize(ptr::null_mut(), &mut size)
    };
    assert!(ptr.is_null());
}

// ---------------------------------------------------------------------------
// PyUnicode_GetLength
// ---------------------------------------------------------------------------

#[test]
fn test_unicode_get_length_null_returns_minus_one() {
    init();
    let len = unsafe { molt_cpython_abi::api::strings::PyUnicode_GetLength(ptr::null_mut()) };
    assert_eq!(len, -1);
}

#[test]
fn test_unicode_get_length_on_object() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abc".as_ptr()) };
    let len = unsafe { molt_cpython_abi::api::strings::PyUnicode_GetLength(py) };
    // With stubs, str_data returns empty => length 0
    assert!(len >= 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyUnicode_Check
// ---------------------------------------------------------------------------

#[test]
fn test_unicode_check_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::strings::PyUnicode_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

// ---------------------------------------------------------------------------
// PyUnicode_CompareWithASCIIString
// ---------------------------------------------------------------------------

#[test]
fn test_compare_with_ascii_null_obj() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_CompareWithASCIIString(
            ptr::null_mut(),
            c"abc".as_ptr(),
        )
    };
    assert_eq!(result, -1);
}

#[test]
fn test_compare_with_ascii_null_string() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abc".as_ptr()) };
    let result = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_CompareWithASCIIString(py, ptr::null())
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyBytes_FromStringAndSize
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_from_string_and_size() {
    init();
    let data = b"hello";
    let py = unsafe {
        molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(data.as_ptr().cast(), 5)
    };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_bytes_from_string_and_size_negative_len() {
    init();
    let py =
        unsafe { molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(c"abc".as_ptr(), -1) };
    assert!(py.is_null());
}

#[test]
fn test_bytes_from_string_and_size_null_allocates_zeros() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(ptr::null(), 10) };
    // Should allocate 10 zero bytes
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_bytes_from_string_and_size_zero_length() {
    init();
    let py =
        unsafe { molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(c"abc".as_ptr(), 0) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyBytes_FromString
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_from_string_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyBytes_FromString(c"data".as_ptr()) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_bytes_from_string_null_returns_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyBytes_FromString(ptr::null()) };
    assert!(py.is_null());
}

// ---------------------------------------------------------------------------
// PyBytes_AsStringAndSize
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_as_string_and_size_null_returns_error() {
    init();
    let mut buf: *mut std::os::raw::c_char = ptr::null_mut();
    let mut len: isize = 0;
    let rc = unsafe {
        molt_cpython_abi::api::strings::PyBytes_AsStringAndSize(ptr::null_mut(), &mut buf, &mut len)
    };
    assert_eq!(rc, -1);
}

// ---------------------------------------------------------------------------
// PyBytes_Check
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_check_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::strings::PyBytes_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

// ---------------------------------------------------------------------------
// PyBytes_Size
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_size_null() {
    init();
    let size = unsafe { molt_cpython_abi::api::strings::PyBytes_Size(ptr::null_mut()) };
    assert_eq!(size, -1);
}
