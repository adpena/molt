mod support;

use std::ffi::c_void;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicUsize, Ordering};

use molt_cpython_abi::abi_types::{PyExc_SystemError, PyObject};
use molt_cpython_abi::api::errors::{PyErr_Clear, PyErr_ExceptionMatches, PyErr_Occurred};
use molt_cpython_abi::api::pending_calls::{
    PendingCallFn, Py_AddPendingCall, Py_MakePendingCalls, finish_pending_calls_before_teardown,
    register_main_thread,
};

static CALLBACKS_RUN: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn must_remain_queued(_arg: *mut c_void) -> c_int {
    CALLBACKS_RUN.fetch_add(1, Ordering::Relaxed);
    0
}

fn assert_system_error_pending() {
    assert!(!unsafe { PyErr_Occurred() }.is_null());
    assert_eq!(
        unsafe { PyErr_ExceptionMatches((&raw mut PyExc_SystemError).cast::<PyObject>()) },
        1
    );
}

#[test]
fn detached_main_direct_c_boundary_sets_and_clears_one_system_error() {
    support::prepare_abi_test_thread(support::stub_runtime_hooks());
    assert!(register_main_thread(std::thread::current().id()));
    assert_eq!(
        unsafe {
            Py_AddPendingCall(
                Some(must_remain_queued as PendingCallFn),
                std::ptr::null_mut(),
            )
        },
        0
    );

    assert_eq!(Py_MakePendingCalls(), -1);
    assert_system_error_pending();
    assert_eq!(CALLBACKS_RUN.load(Ordering::Relaxed), 0);

    unsafe { PyErr_Clear() };
    assert!(unsafe { PyErr_Occurred() }.is_null());

    assert_eq!(finish_pending_calls_before_teardown(), -1);
    assert_eq!(CALLBACKS_RUN.load(Ordering::Relaxed), 0);
    assert_eq!(
        unsafe {
            Py_AddPendingCall(
                Some(must_remain_queued as PendingCallFn),
                std::ptr::null_mut(),
            )
        },
        -1,
        "callbacks scheduled after finalization begins must be rejected"
    );

    assert!(register_main_thread(std::thread::current().id()));
    assert_eq!(
        unsafe {
            Py_AddPendingCall(
                Some(must_remain_queued as PendingCallFn),
                std::ptr::null_mut(),
            )
        },
        0,
        "an explicit new lifecycle reopens a fresh admission epoch"
    );
    assert_eq!(finish_pending_calls_before_teardown(), -1);
    assert_eq!(CALLBACKS_RUN.load(Ordering::Relaxed), 0);
}
