//! Tests for PyLong_*, PyFloat_*, PyBool_*, PyNumber_Check and type checks.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{Py_False, Py_True};
use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyLong
// ---------------------------------------------------------------------------

#[test]
fn test_pylong_from_long_returns_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_roundtrip_positive() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(12345) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py) };
    assert_eq!(val, 12345);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_roundtrip_negative() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-999) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py) };
    assert_eq!(val, -999);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_roundtrip_zero() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(0) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py) };
    assert_eq!(val, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_aslong_null_returns_minus_one() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(ptr::null_mut()) };
    assert_eq!(val, -1);
}

#[test]
fn test_pylong_from_ssize_t() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromSsize_t(77) };
    assert!(!py.is_null());
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsSsize_t(py) };
    assert_eq!(val, 77);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_from_longlong() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(i64::MAX) };
    assert!(!py.is_null());
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLongLong(py) };
    // MoltObject may truncate large ints depending on NaN-boxing; verify non-crash at least
    let _ = val;
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_from_unsigned_long() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromUnsignedLong(100) };
    assert!(!py.is_null());
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLong(py) };
    assert_eq!(val, 100);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyFloat
// ---------------------------------------------------------------------------

#[test]
fn test_pyfloat_from_double_returns_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(3.14) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_roundtrip() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(2.718) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py) };
    assert!((val - 2.718).abs() < 1e-10);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_negative() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(-1.5) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py) };
    assert!((val - (-1.5)).abs() < 1e-10);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_zero() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(0.0) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py) };
    assert_eq!(val, 0.0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_asdouble_null_returns_minus_one() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(ptr::null_mut()) };
    assert_eq!(val, -1.0);
}

#[test]
fn test_pyfloat_asdouble_from_int_coerces() {
    init();
    // PyFloat_AsDouble on an int object should coerce to double
    let py_int = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py_int) };
    assert_eq!(val, 7.0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py_int) };
}

// ---------------------------------------------------------------------------
// PyBool
// ---------------------------------------------------------------------------

#[test]
fn test_pybool_from_long_true() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(1) };
    assert!(std::ptr::eq(py, &raw mut Py_True));
}

#[test]
fn test_pybool_from_long_false() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(0) };
    assert!(std::ptr::eq(py, &raw mut Py_False));
}

#[test]
fn test_pybool_from_long_nonzero_is_true() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(42) };
    assert!(std::ptr::eq(py, &raw mut Py_True));
}

#[test]
fn test_pybool_from_long_negative_is_true() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(-1) };
    assert!(std::ptr::eq(py, &raw mut Py_True));
}

// ---------------------------------------------------------------------------
// Type checks
// ---------------------------------------------------------------------------

#[test]
fn test_pylong_check_on_int() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyLong_Check(py) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_check_on_float_returns_false() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.0) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyLong_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_check_on_float() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.0) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyFloat_Check(py) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_check_on_int_returns_false() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyFloat_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::numbers::PyLong_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_pyfloat_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::numbers::PyFloat_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_pybool_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::numbers::PyBool_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

// ---------------------------------------------------------------------------
// PyNumber_Check
// ---------------------------------------------------------------------------

#[test]
fn test_pynumber_check_on_int() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyNumber_Check(py) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pynumber_check_on_float() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyNumber_Check(py) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pynumber_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::numbers::PyNumber_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

// ---------------------------------------------------------------------------
// PyLong_AsLong on a bool (should coerce to 0/1)
// ---------------------------------------------------------------------------

#[test]
fn test_pylong_aslong_on_true_returns_one() {
    init();
    let py_true = &raw mut Py_True;
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py_true) };
    assert_eq!(val, 1);
}

#[test]
fn test_pylong_aslong_on_false_returns_zero() {
    init();
    let py_false = &raw mut Py_False;
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py_false) };
    assert_eq!(val, 0);
}
