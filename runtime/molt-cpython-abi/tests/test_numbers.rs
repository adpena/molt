//! Tests for PyLong_*, PyFloat_*, PyBool_*, PyNumber_Check and type checks.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{Py_False, Py_True};
use std::ffi::c_void;
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
fn test_pylong_aslonglong_and_overflow_reports_inline_value() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(12345) };
    let mut overflow = 99;
    let val =
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLongLongAndOverflow(py, &mut overflow) };
    assert_eq!(val, 12345);
    assert_eq!(overflow, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_aslonglong_and_overflow_null_sets_error() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut overflow = 99;
    let val = unsafe {
        molt_cpython_abi::api::numbers::PyLong_AsLongLongAndOverflow(ptr::null_mut(), &mut overflow)
    };
    assert_eq!(val, -1);
    assert_eq!(overflow, 0);
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
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
fn test_pylong_from_size_t_and_number_index() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromSize_t(55) };
    assert!(!py.is_null());
    let indexed = unsafe { molt_cpython_abi::api::abstract_number::PyNumber_Index(py) };
    assert!(std::ptr::eq(indexed, py));
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLong(indexed) },
        55
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(indexed);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_pylong_from_longlong_non_inline_requires_runtime_hook() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(i64::MAX) };
    assert!(
        py.is_null(),
        "heap BigInt construction requires registered runtime hooks"
    );
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

#[test]
fn test_pylong_as_unsigned_longlong_and_byte_array() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromUnsignedLong(0x1234) };
    assert!(!py.is_null());
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLong(py) };
    assert_eq!(val, 0x1234);

    let mut little = [0u8; 4];
    let little_rc = unsafe {
        molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
            py.cast(),
            little.as_mut_ptr(),
            little.len(),
            1,
            0,
        )
    };
    assert_eq!(little_rc, 0);
    assert_eq!(little, [0x34, 0x12, 0x00, 0x00]);

    let mut big = [0u8; 4];
    let big_rc = unsafe {
        molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
            py.cast(),
            big.as_mut_ptr(),
            big.len(),
            0,
            0,
        )
    };
    assert_eq!(big_rc, 0);
    assert_eq!(big, [0x00, 0x00, 0x12, 0x34]);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_as_byte_array_rejects_unsigned_negative() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-1) };
    let mut bytes = [0u8; 1];
    let rc = unsafe {
        molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
            py.cast(),
            bytes.as_mut_ptr(),
            bytes.len(),
            1,
            0,
        )
    };
    assert_eq!(rc, -1);
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_pylong_from_unsigned_longlong_non_inline_requires_runtime_hook() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromUnsignedLongLong(u64::MAX) };
    assert!(
        py.is_null(),
        "heap unsigned BigInt construction requires registered runtime hooks"
    );
}

#[test]
fn test_pylong_void_ptr_roundtrip_inline_pointer_value() {
    init();
    let raw = 0x1234usize as *mut c_void;
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromVoidPtr(raw) };
    assert!(!py.is_null());
    let roundtrip = unsafe { molt_cpython_abi::api::numbers::PyLong_AsVoidPtr(py) };
    assert_eq!(roundtrip, raw);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_as_void_ptr_preserves_negative_signed_cast() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-1) };
    let roundtrip = unsafe { molt_cpython_abi::api::numbers::PyLong_AsVoidPtr(py) };
    assert_eq!(roundtrip as usize, usize::MAX);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_as_void_ptr_null_returns_null() {
    init();
    let roundtrip = unsafe { molt_cpython_abi::api::numbers::PyLong_AsVoidPtr(ptr::null_mut()) };
    assert!(roundtrip.is_null());
}

#[test]
fn test_pylong_from_double_truncates_toward_zero() {
    init();
    let positive = unsafe { molt_cpython_abi::api::numbers::PyLong_FromDouble(12.75) };
    let negative = unsafe { molt_cpython_abi::api::numbers::PyLong_FromDouble(-12.75) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(positive) },
        12
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(negative) },
        -12
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(positive);
        molt_cpython_abi::api::refcount::Py_DECREF(negative);
    }
}

#[test]
fn test_pylong_from_double_rejects_nan_and_infinity() {
    init();
    let nan = unsafe { molt_cpython_abi::api::numbers::PyLong_FromDouble(f64::NAN) };
    assert!(nan.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let inf = unsafe { molt_cpython_abi::api::numbers::PyLong_FromDouble(f64::INFINITY) };
    assert!(inf.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyFloat
// ---------------------------------------------------------------------------

#[test]
fn test_pyfloat_from_double_returns_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(PI) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_roundtrip() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(E) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py) };
    assert!((val - E).abs() < 1e-10);
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

#[test]
fn test_py_hash_double_matches_integer_hash_for_integral_values() {
    init();
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), 0.0) },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), 1.0) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), -1.0) },
        -2
    );
}

#[test]
fn test_py_hash_double_handles_infinity_and_nan() {
    init();
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), f64::INFINITY) },
        314159
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), f64::NEG_INFINITY)
        },
        -314159
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), f64::NAN) },
        0
    );
}

// ---------------------------------------------------------------------------
// PyComplex
// ---------------------------------------------------------------------------

#[test]
fn test_pycomplex_roundtrip() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyComplex_FromDoubles(1.25, -2.5) };
    assert!(!py.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyComplex_Check(py) },
        1
    );

    let value = unsafe { molt_cpython_abi::api::numbers::PyComplex_AsCComplex(py) };
    assert_eq!(value.real, 1.25);
    assert_eq!(value.imag, -2.5);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pycomplex_as_c_complex_from_int() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(9) };
    let value = unsafe { molt_cpython_abi::api::numbers::PyComplex_AsCComplex(py) };
    assert_eq!(value.real, 9.0);
    assert_eq!(value.imag, 0.0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
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

#[test]
fn test_pyindex_check_matches_integer_index_contract() {
    init();
    let py_int = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let py_float = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5) };

    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_number::PyIndex_Check(py_int) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_number::PyIndex_Check(&raw mut Py_True) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_number::PyIndex_Check(py_float) },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_number::PyIndex_Check(ptr::null_mut()) },
        0
    );

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py_int);
        molt_cpython_abi::api::refcount::Py_DECREF(py_float);
    }
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
use std::f64::consts::{E, PI};
