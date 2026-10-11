//! Tests for PyUnicode_*, PyBytes_* string/bytes API.

#![allow(non_snake_case)]

mod support;

use std::ptr;

thread_local! {
    static STRING_ALLOCATION_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SELECTED_ALLOCATION_ERROR: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FAILED_ALLOCATION_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

unsafe extern "C" fn alloc_string(data: *const u8, len: usize) -> u64 {
    if STRING_ALLOCATION_ENABLED.with(std::cell::Cell::get) {
        unsafe { support::fake_strings::alloc_str(data, len) }
    } else {
        unsafe { fail_allocation(data, len) }
    }
}

unsafe extern "C" fn fail_allocation(_data: *const u8, _len: usize) -> u64 {
    FAILED_ALLOCATION_CALLS.with(|calls| calls.set(calls.get() + 1));
    let selected = SELECTED_ALLOCATION_ERROR.with(std::cell::Cell::get);
    if selected != 0 {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetRaisedException(
                molt_cpython_abi::api::object::Py_NewRef(std::ptr::with_exposed_provenance_mut(
                    selected,
                )),
            );
        }
    }
    0
}

unsafe extern "C" fn classify(bits: u64) -> u8 {
    if support::fake_strings::contains(bits) {
        molt_cpython_abi::abi_types::MoltTypeTag::Str as u8
    } else {
        molt_cpython_abi::abi_types::MoltTypeTag::Other as u8
    }
}

fn init() -> support::AbiTestThreadStateTransaction {
    STRING_ALLOCATION_ENABLED.with(|enabled| enabled.set(false));
    SELECTED_ALLOCATION_ERROR.with(|selected| selected.set(0));
    FAILED_ALLOCATION_CALLS.with(|calls| calls.set(0));
    let mut hooks = support::stub_runtime_hooks();
    support::fake_strings::wire(&mut hooks);
    hooks.alloc_str = Some(alloc_string);
    hooks.alloc_bytes = fail_allocation;
    hooks.alloc_bytearray = fail_allocation;
    hooks.classify_heap = Some(classify);
    support::enter_runtime_class_abi_test(hooks)
}

// ---------------------------------------------------------------------------
// PyUnicode_FromString
// ---------------------------------------------------------------------------

#[test]
fn test_unicode_from_string_fails_closed_on_alloc_failure() {
    // An installed malformed producer returns zero without selecting an error.
    // The boundary diagnoses that refusal; it must never publish a None placeholder.
    // Actual allocator-selected errors are covered independently below.
    let _abi_test = init();
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
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(ptr::null()) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_string_empty_fails_closed_under_stubs() {
    // Even the empty string routes through alloc_str, which the stub table fails
    // (returns 0) — so under stubs the construction fails closed with NULL. With a
    // real runtime this returns the interned empty str; the stub table proves the
    // refusal path never fabricates a placeholder.
    let _abi_test = init();
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
    // F4 teeth: alloc_str fails under stubs => NULL + SystemError, not a placeholder.
    let _abi_test = init();
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
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(ptr::null(), 5) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_string_and_size_negative_size() {
    let _abi_test = init();
    let py =
        unsafe { molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(c"abc".as_ptr(), -1) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_string_and_size_zero_length_fails_closed_under_stubs() {
    // Zero-length still routes through alloc_str, which the stub fails => NULL.
    let _abi_test = init();
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
    let _abi_test = init();
    let ptr = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8(ptr::null_mut()) };
    assert!(ptr.is_null());
}

#[test]
fn test_unicode_as_utf8_null_object_returns_null() {
    // Under stubs the source str construction fails closed (NULL); AsUTF8 of a
    // NULL object must itself return NULL rather than dereferencing a placeholder.
    let _abi_test = init();
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
    let _abi_test = init();
    let mut size: isize = -1;
    let ptr = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_AsUTF8AndSize(ptr::null_mut(), &mut size)
    };
    assert!(ptr.is_null());
}

#[test]
fn test_unicode_as_ascii_string_null_returns_null() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsASCIIString(ptr::null_mut()) };
    assert!(py.is_null());
}

#[test]
fn test_unicode_from_encoded_object_null_returns_null() {
    let _abi_test = init();
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
    let _abi_test = init();
    let len = unsafe { molt_cpython_abi::api::strings::PyUnicode_GetLength(ptr::null_mut()) };
    assert_eq!(len, -1);
}

#[test]
fn test_unicode_get_length_null_object_returns_minus_one() {
    // Under stubs str construction fails closed (NULL); GetLength(NULL) is the
    // error sentinel -1, never a fabricated 0 length for a placeholder object.
    let _abi_test = init();
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
    let _abi_test = init();
    let result = unsafe { molt_cpython_abi::api::strings::PyUnicode_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

// ---------------------------------------------------------------------------
// PyUnicode_CompareWithASCIIString
// ---------------------------------------------------------------------------

#[test]
fn test_compare_with_ascii_null_obj() {
    let _abi_test = init();
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
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abc".as_ptr()) };
    let result = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_CompareWithASCIIString(py, ptr::null())
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_unicode_compare_null_operand_returns_minus_one() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abc".as_ptr()) };
    let result = unsafe { molt_cpython_abi::api::strings::PyUnicode_Compare(py, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_unicode_contains_null_operand_returns_minus_one() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abc".as_ptr()) };
    let result = unsafe { molt_cpython_abi::api::strings::PyUnicode_Contains(py, ptr::null_mut()) };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_unicode_substring_null_returns_null() {
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::strings::PyUnicode_Substring(ptr::null_mut(), 0, 1) };
    assert!(py.is_null());
}

// ---------------------------------------------------------------------------
// PyBytes_FromStringAndSize
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_from_string_and_size_fails_closed_on_alloc_failure() {
    // F4 teeth: alloc_bytes fails under stubs => NULL + SystemError, not a
    // placeholder None (Objects/bytesobject.c contract).
    let _abi_test = init();
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
    let _abi_test = init();
    let py =
        unsafe { molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(c"abc".as_ptr(), -1) };
    assert!(py.is_null());
}

#[test]
fn test_bytes_from_string_and_size_null_fails_closed_under_stubs() {
    // NULL source requests a zero-filled buffer, still via alloc_bytes, which the
    // stub fails => NULL. Proves the OOM path does not fabricate a placeholder.
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::strings::PyBytes_FromStringAndSize(ptr::null(), 10) };
    assert!(py.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_bytes_from_string_and_size_zero_length_fails_closed_under_stubs() {
    let _abi_test = init();
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
    // F4 teeth: alloc_bytes fails under stubs => NULL + SystemError.
    let _abi_test = init();
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
    let _abi_test = init();
    let py = unsafe { molt_cpython_abi::api::strings::PyBytes_FromString(ptr::null()) };
    assert!(py.is_null());
}

// ---------------------------------------------------------------------------
// PyBytes_AsStringAndSize
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_as_string_and_size_null_returns_error() {
    let _abi_test = init();
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
    let _abi_test = init();
    let result = unsafe { molt_cpython_abi::api::strings::PyBytes_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

// ---------------------------------------------------------------------------
// PyBytes_Size
// ---------------------------------------------------------------------------

#[test]
fn test_bytes_size_null() {
    let _abi_test = init();
    let size = unsafe { molt_cpython_abi::api::strings::PyBytes_Size(ptr::null_mut()) };
    assert_eq!(size, -1);
}

// ---------------------------------------------------------------------------
// PyByteArray
// ---------------------------------------------------------------------------

#[test]
fn test_foreign_bytearray_fixture_has_mutable_storage() {
    let _abi_test = init();
    let py = unsafe { foreign_bytearray_fixture(c"abc".as_ptr(), 3) };
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
    let _abi_test = init();
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
    let _abi_test = init();
    let mut pv: *mut molt_cpython_abi::abi_types::PyObject = ptr::null_mut();
    unsafe {
        molt_cpython_abi::api::strings::PyBytes_Concat(&mut pv, ptr::null_mut());
    }
    assert!(pv.is_null());
}

#[test]
fn test_unicode_concat_fails_closed_on_alloc_failure() {
    // F4 teeth: PyUnicode_Concat allocates the joined string via alloc_str, which
    // the stub fails => NULL + SystemError, never a fabricated None placeholder.
    let _abi_test = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let left = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"a".as_ptr()) };
    let right = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"b".as_ptr()) };
    // Both operands already fail closed under stubs (NULL); Concat of NULL
    // operands must itself return NULL, not an empty-string placeholder.
    let joined = unsafe { molt_cpython_abi::api::strings::PyUnicode_Concat(left, right) };
    assert!(joined.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn native_bytearray_resize_preserves_aliases_until_last_export_releases() {
    let _abi_test = init();
    STRING_ALLOCATION_ENABLED.with(|enabled| enabled.set(true));
    use molt_cpython_abi::abi_types::{Py_buffer, PyBUF_WRITABLE, PyExc_BufferError, PyObject};
    use molt_cpython_abi::api::{buffer, errors, refcount, strings};
    unsafe {
        errors::PyErr_Clear();
        let object = foreign_bytearray_fixture(c"abc".as_ptr(), 3);
        assert!(!object.is_null());
        let data = strings::PyByteArray_AsString(object);
        let mut first: Py_buffer = std::mem::zeroed();
        let mut second: Py_buffer = std::mem::zeroed();
        assert_eq!(
            buffer::PyObject_GetBuffer(object, &mut first, PyBUF_WRITABLE),
            0
        );
        assert_eq!(
            buffer::PyObject_GetBuffer(object, &mut second, PyBUF_WRITABLE),
            0
        );
        assert_eq!(first.buf, data.cast());
        assert_eq!(second.buf, data.cast());
        first.buf.cast::<u8>().add(1).write(b'Z');
        assert_eq!(std::slice::from_raw_parts(data.cast::<u8>(), 4), b"aZc\0");
        assert_eq!(strings::PyByteArray_Resize(object, 3), 0);
        for len in [0, 2, 4] {
            assert_eq!(strings::PyByteArray_Resize(object, len), -1);
            assert_eq!(
                errors::PyErr_ExceptionMatches((&raw mut PyExc_BufferError).cast::<PyObject>()),
                1
            );
            errors::PyErr_Clear();
            assert_eq!(strings::PyByteArray_AsString(object), data);
        }
        buffer::PyBuffer_Release(&mut first);
        assert_eq!(strings::PyByteArray_Resize(object, 5), -1);
        errors::PyErr_Clear();
        buffer::PyBuffer_Release(&mut second);
        assert_eq!(strings::PyByteArray_Resize(object, 5), 0);
        assert_eq!(
            std::slice::from_raw_parts(strings::PyByteArray_AsString(object).cast::<u8>(), 6),
            b"aZc\0\0\0"
        );
        assert_eq!(strings::PyByteArray_Resize(object, 1), 0);
        assert_eq!(
            std::slice::from_raw_parts(strings::PyByteArray_AsString(object).cast::<u8>(), 2),
            b"a\0"
        );
        assert_eq!(strings::PyByteArray_Resize(object, 0), 0);
        assert_eq!(strings::PyByteArray_Size(object), 0);
        assert_eq!(*strings::PyByteArray_AsString(object), 0);
        refcount::Py_DECREF(object);
        assert!(errors::PyErr_Occurred().is_null());
    }
}

#[test]
fn native_bytearray_foreign_subtype_uses_physical_prefix_and_rejects_short_layout() {
    let _abi_test = init();
    use molt_cpython_abi::abi_types::{
        PyByteArray_Type, PyByteArrayObject, PyObject, PyTypeObject,
    };
    use molt_cpython_abi::api::{errors, memory, refcount, strings};
    unsafe {
        errors::PyErr_Clear();
        let mut subtype: PyTypeObject = std::mem::zeroed();
        subtype.tp_base = &raw mut PyByteArray_Type;
        subtype.tp_basicsize = std::mem::size_of::<PyByteArrayObject>() as isize;
        subtype.tp_dealloc = Some(strings::molt_bytearray_dealloc);
        subtype.tp_free = Some(memory::PyObject_Free);
        let object =
            memory::PyObject_Calloc(1, std::mem::size_of::<PyByteArrayObject>()).cast::<PyObject>();
        (*object).ob_refcnt = 1;
        (*object).ob_type = &mut subtype;
        assert_eq!(strings::PyByteArray_Check(object), 1);
        assert_eq!(strings::PyByteArray_CheckExact(object), 0);
        assert_eq!(*strings::PyByteArray_AsString(object), 0);
        assert_eq!(strings::PyByteArray_Resize(object, 2), 0);
        strings::PyByteArray_AsString(object)
            .cast::<u8>()
            .write(b'x');
        assert_eq!(
            std::slice::from_raw_parts(strings::PyByteArray_AsString(object).cast::<u8>(), 3),
            b"x\0\0"
        );
        refcount::Py_DECREF(object);

        subtype.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
        let mut short = PyObject {
            ob_refcnt: 1,
            ob_type: &mut subtype,
        };
        assert_eq!(strings::PyByteArray_Check(&mut short), 1);
        assert!(strings::PyByteArray_AsString(&mut short).is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
    }
}

#[test]
fn public_bytearray_constructor_fails_closed_without_runtime_allocation() {
    let _abi_test = init();
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        assert!(
            molt_cpython_abi::api::strings::PyByteArray_FromStringAndSize(c"abc".as_ptr(), 3)
                .is_null()
        );
        assert_eq!(
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
            ),
            1
        );
        molt_cpython_abi::api::errors::PyErr_Clear();
    }
}

/// An explicit foreign-extension allocation; public constructors return managed
/// objects and therefore never authorize a PyByteArrayObject payload cast.
unsafe fn foreign_bytearray_fixture(
    data: *const std::ffi::c_char,
    len: isize,
) -> *mut molt_cpython_abi::abi_types::PyObject {
    let object = unsafe {
        molt_cpython_abi::api::memory::PyObject_Calloc(
            1,
            std::mem::size_of::<molt_cpython_abi::abi_types::PyByteArrayObject>(),
        )
    }
    .cast::<molt_cpython_abi::abi_types::PyByteArrayObject>();
    assert!(!object.is_null());
    let bytes = unsafe { molt_cpython_abi::api::memory::PyMem_Calloc(1, len as usize + 1) }
        .cast::<std::ffi::c_char>();
    assert!(!bytes.is_null());
    unsafe {
        molt_cpython_abi::api::memory::PyObject_Init(
            object.cast(),
            &raw mut molt_cpython_abi::abi_types::PyByteArray_Type,
        );
        if len != 0 {
            std::ptr::copy_nonoverlapping(data, bytes, len as usize);
        }
        (*object).ob_base.ob_size = len;
        (*object).ob_alloc = len + 1;
        (*object).ob_bytes = bytes;
        (*object).ob_start = bytes;
    }
    object.cast()
}

#[test]
fn string_and_bytes_constructors_forward_the_selected_failure_once() {
    use molt_cpython_abi::abi_types::{PyExc_LookupError, PyExc_ValueError, PyObject};
    use molt_cpython_abi::api::{errors, object, refcount, strings};

    let _abi_test = init();
    unsafe {
        errors::PyErr_Clear();
        errors::PyErr_SetNone((&raw mut PyExc_LookupError).cast());
        let prior = errors::PyErr_GetRaisedException();
        let _prior_owner = refcount::OwnedPyObject::from_owned(prior);
        assert!(!prior.is_null());
        errors::PyErr_SetNone((&raw mut PyExc_ValueError).cast());
        let other = errors::PyErr_GetRaisedException();
        let _other_owner = refcount::OwnedPyObject::from_owned(other);
        assert!(!other.is_null());

        struct ResetAllocationError;
        impl Drop for ResetAllocationError {
            fn drop(&mut self) {
                SELECTED_ALLOCATION_ERROR.with(|selected| selected.set(0));
            }
        }
        let _reset = ResetAllocationError;
        for incoming in [false, true] {
            for selected in [prior, other] {
                for constructor in 0..4 {
                    if incoming {
                        errors::PyErr_SetRaisedException(object::Py_NewRef(prior));
                    }
                    SELECTED_ALLOCATION_ERROR.with(|slot| slot.set(selected.addr()));
                    FAILED_ALLOCATION_CALLS.with(|calls| calls.set(0));
                    let result: *mut PyObject = match constructor {
                        0 => strings::PyUnicode_FromString(c"failure payload".as_ptr()),
                        1 => strings::PyBytes_FromString(c"failure payload".as_ptr()),
                        2 => strings::PyBytes_FromStringAndSize(c"failure payload".as_ptr(), 15),
                        _ => {
                            strings::PyByteArray_FromStringAndSize(c"failure payload".as_ptr(), 15)
                        }
                    };
                    let _result_owner = refcount::OwnedPyObject::from_owned(result);
                    let raised = errors::PyErr_GetRaisedException();
                    let _raised_owner = refcount::OwnedPyObject::from_owned(raised);
                    assert!(result.is_null(), "constructor {constructor}");
                    assert_eq!(
                        raised, selected,
                        "constructor {constructor}, prior {incoming}"
                    );
                    assert_eq!(FAILED_ALLOCATION_CALLS.with(std::cell::Cell::get), 1);
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn missing_string_and_bytes_allocators_select_refusal_over_an_incoming_error() {
    use molt_cpython_abi::abi_types::{PyExc_LookupError, PyExc_SystemError, PyObject};
    use molt_cpython_abi::api::{errors, object, refcount, strings};

    const CHILD: &str = "MOLT_ABI_TEST_MISSING_TEXT_ALLOCATORS";
    if std::env::var_os(CHILD).is_none() {
        // Capability absence is an Option field of the process-owned table,
        // not a mutable callback mode. Use the existing fresh-self-image
        // registration control so sibling tests retain their installed table.
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "missing_string_and_bytes_allocators_select_refusal_over_an_incoming_error",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, "1");
        let output = support::captured_runtime_children::capture(
            &mut command,
            "abi-hook-registration",
            "missing-text-allocators",
        );
        assert!(
            output.status.success(),
            "child stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("missing-text-allocators-refused")
        );
        return;
    }
    assert!(molt_cpython_abi::hooks::hooks().is_none());

    // This is a registered partial provider: the transaction supplies native
    // GC and class custody, but text allocation remains absent. Diagnostic
    // construction must not recursively invoke its unavailable allocator.
    let _abi_test = support::enter_runtime_class_abi_test(support::stub_runtime_hooks());
    assert!(
        molt_cpython_abi::hooks::hooks_or_stubs()
            .native_gc_allocate
            .is_some()
    );
    assert!(
        molt_cpython_abi::hooks::hooks_or_stubs()
            .runtime_class_borrowed
            .is_some()
    );
    assert!(
        molt_cpython_abi::hooks::hooks_or_stubs()
            .alloc_str
            .is_none()
    );
    unsafe {
        errors::PyErr_Clear();
        errors::PyErr_SetNone((&raw mut PyExc_LookupError).cast());
        let prior = errors::PyErr_GetRaisedException();
        let _prior_owner = refcount::OwnedPyObject::from_owned(prior);
        assert!(!prior.is_null());
        for incoming in [false, true] {
            for constructor in 0..4 {
                if incoming {
                    errors::PyErr_SetRaisedException(object::Py_NewRef(prior));
                }
                let result: *mut PyObject = match constructor {
                    0 => strings::PyUnicode_FromString(c"unavailable text".as_ptr()),
                    1 => strings::PyBytes_FromString(c"unavailable text".as_ptr()),
                    2 => strings::PyBytes_FromStringAndSize(ptr::null(), 3),
                    _ => strings::PyByteArray_FromStringAndSize(ptr::null(), 3),
                };
                let _result_owner = refcount::OwnedPyObject::from_owned(result);
                assert!(result.is_null());
                assert_eq!(
                    errors::PyErr_Occurred(),
                    (&raw mut PyExc_SystemError).cast()
                );
                let selected = errors::PyErr_GetRaisedException();
                let _selected_owner = refcount::OwnedPyObject::from_owned(selected);
                assert!(!selected.is_null());
                assert_ne!(selected, prior);
            }
        }
    }
    println!("missing-text-allocators-refused");
}

#[test]
fn installed_diagnostic_allocator_zero_without_error_is_a_contract_failure() {
    use molt_cpython_abi::abi_types::PyExc_ValueError;
    use molt_cpython_abi::api::{errors, refcount};

    // The existing installed provider deliberately returns a clean zero when
    // no selected failure is armed. Keep the process's hook table unchanged.
    let _abi_test = init();
    unsafe {
        errors::PyErr_Clear();
        FAILED_ALLOCATION_CALLS.with(|calls| calls.set(0));
        errors::PyErr_SetString((&raw mut PyExc_ValueError).cast(), c"diagnostic".as_ptr());
        assert_eq!(
            errors::PyErr_Occurred(),
            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
        );
        assert_eq!(FAILED_ALLOCATION_CALLS.with(std::cell::Cell::get), 1);
        let mut selected_type = ptr::null_mut();
        let mut selected_value = ptr::null_mut();
        let mut selected_traceback = ptr::null_mut();
        errors::PyErr_Fetch(
            &mut selected_type,
            &mut selected_value,
            &mut selected_traceback,
        );
        let _type_owner = refcount::OwnedPyObject::from_owned(selected_type);
        let _value_owner = refcount::OwnedPyObject::from_owned(selected_value);
        let _traceback_owner = refcount::OwnedPyObject::from_owned(selected_traceback);
        assert_eq!(
            selected_type,
            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
        );
        assert!(
            selected_value.is_null(),
            "malformed-producer refusal must not retry allocation"
        );
        assert!(selected_traceback.is_null());
    }
}
