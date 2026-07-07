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
fn test_contextvar_new_name_alloc_fails_closed_under_stubs() {
    // PyContextVar_New builds the variable's name via PyUnicode_FromString. After
    // the fail-open burndown that allocation fails closed (NULL) under the stub
    // hook table, so PyContextVar_New propagates the failure and returns NULL
    // rather than constructing a var around a fabricated None-name placeholder.
    // (Get/Set semantics with a real name require a runtime and are covered by the
    // c_extensions integration suite.)
    init();
    let default_value = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) };
    let var = unsafe {
        molt_cpython_abi::api::contextvars::PyContextVar_New(c"answer".as_ptr(), default_value)
    };
    assert!(
        var.is_null(),
        "PyContextVar_New must fail closed when its name string cannot be allocated"
    );
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(default_value);
    }
}
