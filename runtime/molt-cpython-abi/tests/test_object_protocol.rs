//! Tests for object protocol: PyObject_Repr, Str, Hash, RichCompare,
//! TypeCheck, IsInstance, CallableCheck.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::Py_NotImplementedSentinel;
use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyObject_Repr / PyObject_Str
// ---------------------------------------------------------------------------

#[test]
fn test_object_repr_returns_string() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let repr = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(py) };
    assert!(!repr.is_null());
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(repr);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_object_repr_null_returns_null() {
    init();
    let repr = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(ptr::null_mut()) };
    assert!(repr.is_null());
}

#[test]
fn test_object_str_returns_string() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(py) };
    assert!(!s.is_null());
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(s);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

// ---------------------------------------------------------------------------
// PyObject_Hash
// ---------------------------------------------------------------------------

#[test]
fn test_object_hash_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let hash = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(py) };
    // Should return some non-zero value (pointer-based)
    assert_ne!(hash, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_object_hash_different_objects_differ() {
    init();
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let ha = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(a) };
    let hb = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(b) };
    // Different pointers => different hashes (pointer-based hash)
    assert_ne!(ha, hb);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

// ---------------------------------------------------------------------------
// PyObject_TypeCheck
// ---------------------------------------------------------------------------

#[test]
fn test_object_typecheck_matching_type() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let tp = unsafe { (*py).ob_type };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_TypeCheck(py, tp) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_object_typecheck_mismatched_type() {
    init();
    let py_int = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let py_float = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.0) };
    let float_tp = unsafe { (*py_float).ob_type };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_TypeCheck(py_int, float_tp) };
    assert_eq!(result, 0);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py_int);
        molt_cpython_abi::api::refcount::Py_DECREF(py_float);
    }
}

#[test]
fn test_object_typecheck_null_args() {
    init();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::typeobj::PyObject_TypeCheck(ptr::null_mut(), ptr::null_mut())
        },
        0
    );
}

// ---------------------------------------------------------------------------
// PyObject_IsInstance
// ---------------------------------------------------------------------------

#[test]
fn test_isinstance_null_returns_zero() {
    init();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::typeobj::PyObject_IsInstance(ptr::null_mut(), ptr::null_mut())
        },
        0
    );
}

// ---------------------------------------------------------------------------
// Py_TYPE
// ---------------------------------------------------------------------------

#[test]
fn test_py_type_returns_ob_type() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let tp = unsafe { molt_cpython_abi::api::typeobj::_Py_TYPE(py) };
    assert!(!tp.is_null());
    assert_eq!(tp, unsafe { (*py).ob_type });
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_py_type_null_returns_null() {
    init();
    let tp = unsafe { molt_cpython_abi::api::typeobj::_Py_TYPE(ptr::null_mut()) };
    assert!(tp.is_null());
}

// ---------------------------------------------------------------------------
// PyCallable_Check
// ---------------------------------------------------------------------------

#[test]
fn test_callable_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_callable_check_on_int_returns_zero() {
    init();
    // Integers don't have tp_call
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyObject_RichCompare / PyObject_RichCompareBool
// ---------------------------------------------------------------------------

const PY_LT: i32 = 0;
const PY_EQ: i32 = 2;
const PY_NE: i32 = 3;

#[test]
fn test_richcompare_same_object_eq() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    // Without tp_richcompare set, falls back to NotImplemented, then pointer identity
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(py, py, PY_EQ) };
    // Same pointer => EQ should be 1
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_richcompare_different_objects_ne() {
    init();
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(a, b, PY_NE) };
    // Different pointers => NE should be 1
    assert_eq!(result, 1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

#[test]
fn test_richcompare_returns_not_implemented_for_lt() {
    init();
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompare(a, b, PY_LT) };
    // Without tp_richcompare, returns NotImplemented sentinel
    assert!(std::ptr::eq(result, &raw mut Py_NotImplementedSentinel));
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

#[test]
fn test_richcompare_null_is_safe() {
    init();
    // v=NULL should not crash
    let result = unsafe {
        molt_cpython_abi::api::typeobj::PyObject_RichCompare(
            ptr::null_mut(),
            ptr::null_mut(),
            PY_EQ,
        )
    };
    // Returns NotImplemented sentinel
    assert!(std::ptr::eq(result, &raw mut Py_NotImplementedSentinel));
}

#[test]
fn test_richcomparebool_null_returns_error() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(
            ptr::null_mut(),
            ptr::null_mut(),
            PY_LT,
        )
    };
    // LT on null => cannot compare => -1 (error)
    assert_eq!(result, -1);
}
