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
fn test_unicode_from_string_fails_closed_on_alloc_failure() {
    // F4 teeth: with stub hooks, alloc_str returns 0 (allocation failure).
    // PyUnicode_FromString MUST fail closed with NULL + MemoryError (CPython's
    // Objects/unicodeobject.c contract), NOT a fabricated Py_None placeholder
    // that reads as a non-NULL success and defeats `if (s == NULL)`.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"hello".as_ptr()) };
    assert!(
        py.is_null(),
        "PyUnicode_FromString must return NULL on alloc failure, not a placeholder"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL return from PyUnicode_FromString must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_unicode_from_string_null_returns_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(ptr::null()) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_string_empty_fails_closed_under_stubs() {
    // Even the empty string routes through alloc_str, which the stub table fails
    // (returns 0) — so under stubs the construction fails closed with NULL. With a
    // real runtime this returns the interned empty str; the stub table proves the
    // OOM path never fabricates a placeholder.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"".as_ptr()) };
    assert!(py.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyUnicode_FromStringAndSize
// ---------------------------------------------------------------------------

#[test]
fn test_unicode_from_string_and_size_fails_closed_on_alloc_failure() {
    // F4 teeth: alloc_str fails under stubs => NULL + MemoryError, not a placeholder.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let data = b"world\0";
    let py = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(data.as_ptr().cast(), 5)
    };
    assert!(
        py.is_null(),
        "PyUnicode_FromStringAndSize must fail closed (NULL) on alloc failure"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
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
fn test_unicode_from_string_and_size_zero_length_fails_closed_under_stubs() {
    // Zero-length still routes through alloc_str, which the stub fails => NULL.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(c"abc".as_ptr(), 0) };
    assert!(py.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
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
fn test_unicode_as_utf8_null_object_returns_null() {
    // Under stubs the source str construction fails closed (NULL); AsUTF8 of a
    // NULL object must itself return NULL rather than dereferencing a placeholder.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"test".as_ptr()) };
    assert!(py.is_null(), "str construction fails closed under stubs");
    let utf8 = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(py) };
    assert!(utf8.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
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

#[test]
fn test_unicode_as_ascii_string_null_returns_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsASCIIString(ptr::null_mut()) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_encoded_object_null_returns_null() {
    init();
    let py = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromEncodedObject(
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
        )
    };
    assert!(py.is_null());
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
fn test_unicode_get_length_null_object_returns_minus_one() {
    // Under stubs str construction fails closed (NULL); GetLength(NULL) is the
    // error sentinel -1, never a fabricated 0 length for a placeholder object.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abc".as_ptr()) };
    assert!(py.is_null(), "str construction fails closed under stubs");
    let len = unsafe { molt_cpython_abi::api::strings::PyUnicode_GetLength(py) };
    assert_eq!(len, -1);
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
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

#[test]
fn test_unicode_compare_null_operand_returns_minus_one() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abc".as_ptr()) };
    let result = unsafe { molt_cpython_abi::api::strings::PyUnicode_Compare(py, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_unicode_contains_null_operand_returns_minus_one() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abc".as_ptr()) };
    let result = unsafe { molt_cpython_abi::api::strings::PyUnicode_Contains(py, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_unicode_substring_null_returns_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_Substring(ptr::null_mut(), 0, 1) };
    assert!(py.is_null());
}

// ---------------------------------------------------------------------------
// PyBytes_FromStringAndSize
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_from_string_and_size_fails_closed_on_alloc_failure() {
    // F4 teeth: alloc_bytes fails under stubs => NULL + MemoryError, not a
    // placeholder None (Objects/bytesobject.c contract).
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let data = b"hello";
    let py = unsafe {
        molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(data.as_ptr().cast(), 5)
    };
    assert!(
        py.is_null(),
        "PyBytes_FromStringAndSize must fail closed (NULL) on alloc failure"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_bytes_from_string_and_size_negative_len() {
    init();
    let py =
        unsafe { molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(c"abc".as_ptr(), -1) };
    assert!(py.is_null());
}

#[test]
fn test_bytes_from_string_and_size_null_fails_closed_under_stubs() {
    // NULL source requests a zero-filled buffer, still via alloc_bytes, which the
    // stub fails => NULL. Proves the OOM path does not fabricate a placeholder.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(ptr::null(), 10) };
    assert!(py.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_bytes_from_string_and_size_zero_length_fails_closed_under_stubs() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py =
        unsafe { molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(c"abc".as_ptr(), 0) };
    assert!(py.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyBytes_FromString
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_from_string_fails_closed_on_alloc_failure() {
    // F4 teeth: alloc_bytes fails under stubs => NULL + MemoryError.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::strings::PyBytes_FromString(c"data".as_ptr()) };
    assert!(
        py.is_null(),
        "PyBytes_FromString must fail closed (NULL) on alloc failure"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
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

// ---------------------------------------------------------------------------
// PyByteArray
// ---------------------------------------------------------------------------

#[test]
fn test_bytearray_from_string_has_mutable_storage() {
    init();
    let py = unsafe {
        molt_cpython_abi::api::strings::PyByteArray_FromStringAndSize(c"abc".as_ptr(), 3)
    };
    assert!(!py.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::strings::PyByteArray_Check(py) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::strings::PyByteArray_Size(py) },
        3
    );
    let data = unsafe { molt_cpython_abi::api::strings::PyByteArray_AsString(py) };
    assert!(!data.is_null());
    unsafe {
        *data.add(1) = b'Z' as std::os::raw::c_char;
        assert_eq!(*data.add(0), b'a' as std::os::raw::c_char);
        assert_eq!(*data.add(1), b'Z' as std::os::raw::c_char);
        assert_eq!(*data.add(2), b'c' as std::os::raw::c_char);
        assert_eq!(*data.add(3), 0);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_bytearray_negative_len_returns_null() {
    init();
    let py = unsafe {
        molt_cpython_abi::api::strings::PyByteArray_FromStringAndSize(c"abc".as_ptr(), -1)
    };
    assert!(py.is_null());
}

// ---------------------------------------------------------------------------
// PyBytes_Concat / PyUnicode_Concat — fail-open burndown teeth
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_concat_null_args_are_noops() {
    // PyBytes_Concat(pv, w): NULL *pv or NULL w is a documented no-op; it must
    // not crash. (The real concat path needs a runtime and is exercised in the
    // c_extensions integration suite.)
    init();
    let mut pv: *mut molt_cpython_abi::abi_types::PyObject = ptr::null_mut();
    unsafe {
        molt_cpython_abi::api::strings::PyBytes_Concat(&mut pv, ptr::null_mut());
    }
    assert!(pv.is_null());
}

#[test]
fn test_unicode_concat_fails_closed_on_alloc_failure() {
    // F4 teeth: PyUnicode_Concat allocates the joined string via alloc_str, which
    // the stub fails => NULL + MemoryError, never a fabricated None placeholder.
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let left = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"a".as_ptr()) };
    let right = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"b".as_ptr()) };
    // Both operands already fail closed under stubs (NULL); Concat of NULL
    // operands must itself return NULL, not an empty-string placeholder.
    let joined = unsafe { molt_cpython_abi::api::strings::PyUnicode_Concat(left, right) };
    assert!(joined.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
