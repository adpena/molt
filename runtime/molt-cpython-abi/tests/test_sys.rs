#![allow(non_snake_case)]

use std::ptr;

unsafe extern "C" fn fake_sys_get_object_borrowed(
    name: *const u8,
    len: usize,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    let name = unsafe { std::slice::from_raw_parts(name, len) };
    if name == b"flags" {
        molt_cpython_abi::hooks::BorrowedHandleResult::error()
    } else {
        molt_cpython_abi::hooks::BorrowedHandleResult::missing()
    }
}

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.sys_get_object_borrowed = fake_sys_get_object_borrowed;
    unsafe {
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

#[test]
fn test_pysys_getobject_hook_error_fails_closed_with_systemerror() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let flags = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"flags".as_ptr()) };
    assert!(flags.is_null());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError)
                    .cast::<molt_cpython_abi::abi_types::PyObject>(),
            )
        },
        1
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_pysys_getobject_unknown_returns_null_without_error() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let missing = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(c"not_present".as_ptr()) };
    assert!(missing.is_null());
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
}

#[test]
fn test_pysys_getobject_null_name_returns_null() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let missing = unsafe { molt_cpython_abi::api::sys::PySys_GetObject(ptr::null()) };
    assert!(missing.is_null());
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
}
