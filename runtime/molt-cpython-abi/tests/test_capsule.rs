//! Tests for PyCapsule_* object-backed ABI behavior.

#![allow(non_snake_case)]

use std::ffi::c_void;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

#[test]
fn test_capsule_pointer_context_and_import_registry() {
    init();
    let mut value = 7u32;
    let mut updated = 9u32;
    let mut context = 11u32;
    let name = c"demo._C_API";
    let renamed = c"demo._RENAMED_API";

    let capsule = unsafe {
        molt_cpython_abi::api::capsule::PyCapsule_New(
            (&mut value as *mut u32).cast::<c_void>(),
            name.as_ptr(),
            None,
        )
    };
    assert!(!capsule.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_CheckExact(capsule) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_IsValid(capsule, name.as_ptr()) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_GetPointer(capsule, name.as_ptr()) },
        (&mut value as *mut u32).cast::<c_void>()
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_Import(name.as_ptr(), 0) },
        (&mut value as *mut u32).cast::<c_void>()
    );

    assert_eq!(
        unsafe {
            molt_cpython_abi::api::capsule::PyCapsule_SetContext(
                capsule,
                (&mut context as *mut u32).cast::<c_void>(),
            )
        },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_GetContext(capsule) },
        (&mut context as *mut u32).cast::<c_void>()
    );

    assert_eq!(
        unsafe {
            molt_cpython_abi::api::capsule::PyCapsule_SetPointer(
                capsule,
                (&mut updated as *mut u32).cast::<c_void>(),
            )
        },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_Import(name.as_ptr(), 0) },
        (&mut updated as *mut u32).cast::<c_void>()
    );

    assert_eq!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_SetName(capsule, renamed.as_ptr()) },
        0
    );
    assert!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_Import(name.as_ptr(), 0) }.is_null()
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_Import(renamed.as_ptr(), 0) },
        (&mut updated as *mut u32).cast::<c_void>()
    );

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(capsule) };
    assert!(
        unsafe { molt_cpython_abi::api::capsule::PyCapsule_Import(renamed.as_ptr(), 0) }.is_null()
    );
}
