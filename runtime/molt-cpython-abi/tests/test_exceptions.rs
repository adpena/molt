//! Tests for PyErr_SetString, PyErr_Occurred, PyErr_Clear, PyErr_SetNone,
//! PyErr_Print, PyErr_Format.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{PyExc_TypeError, PyExc_ValueError};
use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyErr_SetString / PyErr_Occurred / PyErr_Clear
// ---------------------------------------------------------------------------

#[test]
fn test_no_exception_initially() {
    init();
    // Clear any leftover state from other tests
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null());
}

#[test]
fn test_set_string_and_occurred() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_ValueError;
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"test error".as_ptr());
    }

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null(), "Exception should be set");

    // Clear and verify
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred2 = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred2.is_null(), "Exception should be cleared");
}

#[test]
fn test_set_string_with_null_message() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_TypeError;
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, ptr::null());
    }

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_set_string_with_null_exc_type() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(ptr::null_mut(), c"msg".as_ptr());
    }

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyErr_SetNone
// ---------------------------------------------------------------------------

#[test]
fn test_set_none() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_ValueError;
    unsafe { molt_cpython_abi::api::errors::PyErr_SetNone(exc) };

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyErr_Clear idempotent
// ---------------------------------------------------------------------------

#[test]
fn test_clear_when_no_exception() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // Clearing when nothing set should be a noop
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null());
}

#[test]
fn test_double_clear() {
    init();
    let exc = &raw mut PyExc_ValueError;
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"err".as_ptr());
    }
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null());
}

// ---------------------------------------------------------------------------
// PyErr_Print
// ---------------------------------------------------------------------------

#[test]
fn test_print_clears_exception() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_ValueError;
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(exc, c"printed error".as_ptr());
    }

    // PyErr_Print should print and then clear
    unsafe { molt_cpython_abi::api::errors::PyErr_Print() };

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(occurred.is_null(), "Exception should be cleared after Print");
}

#[test]
fn test_print_when_no_exception() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // Should not crash
    unsafe { molt_cpython_abi::api::errors::PyErr_Print() };
}

// ---------------------------------------------------------------------------
// PyErr_Format
// ---------------------------------------------------------------------------

#[test]
fn test_format_sets_exception_returns_null() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let exc = &raw mut PyExc_TypeError;
    let result = unsafe {
        molt_cpython_abi::api::errors::PyErr_Format(exc, c"bad type".as_ptr())
    };
    assert!(result.is_null(), "PyErr_Format should return NULL");

    let occurred = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!occurred.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// Overwrite exception
// ---------------------------------------------------------------------------

#[test]
fn test_overwrite_exception() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let val_exc = &raw mut PyExc_ValueError;
    let type_exc = &raw mut PyExc_TypeError;

    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(val_exc, c"first".as_ptr());
    }
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());

    // Overwrite with a different exception
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(type_exc, c"second".as_ptr());
    }
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());

    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
