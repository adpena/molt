//! Detached-runtime contracts live in their own integration binary so the
//! process-global hook table remains genuinely unregistered.

use molt_cpython_abi::abi_types::{Py_OptimizeFlag, PyMutex, PyObject};
use std::ptr;

#[test]
fn detached_abi_does_not_fabricate_runtime_or_thread_state() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::Py_IsInitialized() },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyGILState_Check() },
        0
    );
    assert!(unsafe { molt_cpython_abi::api::object::_PyThreadState_UncheckedGet() }.is_null());
    assert_eq!(molt_cpython_abi::api::object::PY_GIL_STATE_LOCKED, 0);
    assert_eq!(molt_cpython_abi::api::object::PY_GIL_STATE_UNLOCKED, 1);
}

#[test]
fn detached_mutex_and_unique_reference_queries_keep_real_state() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyGILState_Check() },
        0
    );

    let mut mutex = PyMutex { _bits: 0 };
    unsafe { molt_cpython_abi::api::object::PyMutex_Lock(&mut mutex) };
    assert_eq!(mutex._bits, 1);
    unsafe { molt_cpython_abi::api::object::PyMutex_Unlock(&mut mutex) };
    assert_eq!(mutex._bits, 0);

    let mut obj = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(&mut obj) },
        1
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::object::PyUnstable_Object_IsUniqueReferencedTemporary(&mut obj)
        },
        1
    );
    obj.ob_refcnt = 2;
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(&mut obj) },
        0
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(ptr::null_mut())
        },
        0
    );
    assert_eq!(unsafe { Py_OptimizeFlag }, 0);
}
