#![allow(non_snake_case)]

use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

#[test]
fn test_pysys_getobject_returns_null_without_runtime_sys_hook() {
    init();
    let flags = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"flags".as_ptr()) };
    assert!(flags.is_null());
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
}

#[test]
fn test_pysys_getobject_unknown_returns_null_without_error() {
    init();
    let missing = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"not_present".as_ptr()) };
    assert!(missing.is_null());
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
}

#[test]
fn test_pysys_getobject_null_name_returns_null() {
    init();
    let missing = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(ptr::null()) };
    assert!(missing.is_null());
}
