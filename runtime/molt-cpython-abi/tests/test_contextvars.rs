//! Tests for the concrete CPython context variable C-API surface.

#![allow(non_snake_case)]

use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

#[test]
fn test_contextvar_new_rejects_null_name() {
    init();
    let var = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_New(ptr::null(), ptr::null_mut())
    };
    assert!(var.is_null());
}

#[test]
fn test_contextvar_get_uses_constructor_default() {
    init();
    let default_value = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) };
    let var = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_New(c"answer".as_ptr(), default_value)
    };
    assert!(!var.is_null());

    let mut out = ptr::null_mut();
    let rc = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_Get(var, ptr::null_mut(), &mut out)
    };
    assert_eq!(rc, 0);
    assert_eq!(out, default_value);

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(out);
        molt_cpython_abi::api::refcount::Py_DECREF(var);
        molt_cpython_abi::api::refcount::Py_DECREF(default_value);
    }
}

#[test]
fn test_contextvar_set_updates_current_value() {
    init();
    let var = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_New(c"state".as_ptr(), ptr::null_mut())
    };
    assert!(!var.is_null());
    let value = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(11) };
    let token = unsafe { molt_cpython_abi::api::contextvars::PyContextVar_Set(var, value) };
    assert!(std::ptr::eq(
        token,
        &raw mut molt_cpython_abi::abi_types::Py_None
    ));

    let mut out = ptr::null_mut();
    let rc = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_Get(var, ptr::null_mut(), &mut out)
    };
    assert_eq!(rc, 0);
    assert_eq!(out, value);

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(out);
        molt_cpython_abi::api::refcount::Py_DECREF(token);
        molt_cpython_abi::api::refcount::Py_DECREF(var);
        molt_cpython_abi::api::refcount::Py_DECREF(value);
    }
}
