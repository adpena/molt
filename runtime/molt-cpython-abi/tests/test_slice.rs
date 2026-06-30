//! Tests for PySlice_* ABI behavior.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{Py_None, PySliceObject};

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

#[test]
fn test_slice_new_owns_start_stop_and_normalizes_null_step() {
    init();
    let start = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let stop = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let start_refcnt_before = unsafe { (*start).ob_refcnt };
    let stop_refcnt_before = unsafe { (*stop).ob_refcnt };
    let slice =
        unsafe { molt_cpython_abi::api::slice::PySlice_New(start, stop, std::ptr::null_mut()) };
    assert!(!slice.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::slice::PySlice_Check(slice) },
        1
    );

    let layout = slice.cast::<PySliceObject>();
    assert!(std::ptr::eq(unsafe { (*layout).start }, start));
    assert!(std::ptr::eq(unsafe { (*layout).stop }, stop));
    assert!(std::ptr::eq(unsafe { (*layout).step }, &raw mut Py_None));
    assert_eq!(unsafe { (*start).ob_refcnt }, start_refcnt_before + 1);
    assert_eq!(unsafe { (*stop).ob_refcnt }, stop_refcnt_before + 1);

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(slice);
        molt_cpython_abi::api::refcount::Py_DECREF(start);
        molt_cpython_abi::api::refcount::Py_DECREF(stop);
    }
}

#[test]
fn test_slice_get_indices_ex_positive_step() {
    init();
    let start = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let stop = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(6) };
    let step = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let slice = unsafe { molt_cpython_abi::api::slice::PySlice_New(start, stop, step) };
    let mut out_start = 0;
    let mut out_stop = 0;
    let mut out_step = 0;
    let mut out_len = 0;

    assert_eq!(
        unsafe {
            molt_cpython_abi::api::slice::PySlice_GetIndicesEx(
                slice,
                10,
                &raw mut out_start,
                &raw mut out_stop,
                &raw mut out_step,
                &raw mut out_len,
            )
        },
        0
    );
    assert_eq!((out_start, out_stop, out_step, out_len), (1, 6, 2, 3));

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(slice);
        molt_cpython_abi::api::refcount::Py_DECREF(start);
        molt_cpython_abi::api::refcount::Py_DECREF(stop);
        molt_cpython_abi::api::refcount::Py_DECREF(step);
    }
}

#[test]
fn test_slice_get_indices_ex_negative_step_defaults() {
    init();
    let step = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-1) };
    let slice = unsafe {
        molt_cpython_abi::api::slice::PySlice_New(&raw mut Py_None, &raw mut Py_None, step)
    };
    let mut out_start = 0;
    let mut out_stop = 0;
    let mut out_step = 0;
    let mut out_len = 0;

    assert_eq!(
        unsafe {
            molt_cpython_abi::api::slice::PySlice_GetIndicesEx(
                slice,
                4,
                &raw mut out_start,
                &raw mut out_stop,
                &raw mut out_step,
                &raw mut out_len,
            )
        },
        0
    );
    assert_eq!((out_start, out_stop, out_step, out_len), (3, -1, -1, 4));

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(slice);
        molt_cpython_abi::api::refcount::Py_DECREF(step);
    }
}
