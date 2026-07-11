//! Tests for PySlice_* ABI behavior.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{Py_None, PySliceObject};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Serializes this binary's slice tests. `PySlice_*` index resolution reads its
/// small-int operands back through the process-global `GLOBAL_BRIDGE` small-int
/// proxy cache (`slice_index` -> `is_int_like` -> `pyobj_to_handle`). Sibling
/// tests that create the SAME small-int value (e.g. `2`, used by both
/// `test_slice_new_owns_start_stop_and_normalizes_null_step` and
/// `test_slice_get_indices_ex_positive_step`) share ONE *deduped, mortal* proxy
/// whose `ob_refcnt` the ABI mutates WITHOUT the bridge lock (`Py_INCREF` /
/// `Py_DECREF`, api/refcount.rs). Under `cargo test`'s parallel threads those
/// non-atomic refcount read-modify-writes race and can drop the shared proxy to
/// zero mid-test, evicting a still-live handle so `slice_index` resolves it to
/// `-1` and `PySlice_GetIndicesEx` returns `-1`. Production never hits this:
/// C-extension ABI calls are GIL-serialized. This lock restores that
/// single-threaded invariant for the harness (the `TEST_LOCK` convention used
/// across this crate's integration tests). It is NOT a product bug — the value
/// checks below are unchanged.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the serialization guard (poison-tolerant, so one test's failure never
/// cascades into its siblings) and run the idempotent ABI init. The returned
/// `MutexGuard` is itself `#[must_use]`, so a test that drops it early is caught.
fn init() -> MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    guard
}

#[test]
fn test_slice_new_owns_start_stop_and_normalizes_null_step() {
    let _guard = init();
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
    let _guard = init();
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
    let _guard = init();
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
