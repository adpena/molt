//! Tests for Py_INCREF / Py_DECREF / Py_XINCREF / Py_XDECREF / Py_CLEAR.
//!
//! Validates reference counting semantics on bridge-managed PyObject headers,
//! including immortal singleton handling and release-on-zero behaviour.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{Py_False, Py_None, Py_True, PyObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ptr;

/// Ensure bridge + static types are initialised.
fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// Basic INCREF / DECREF on a bridge-allocated int object
// ---------------------------------------------------------------------------

#[test]
fn test_incref_increments_refcount() {
    init();
    let bits = MoltObject::from_int(42).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(!py.is_null());

    let rc_before = unsafe { (*py).ob_refcnt };
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(py) };
    let rc_after = unsafe { (*py).ob_refcnt };
    assert_eq!(rc_after, rc_before + 1);

    // Clean up
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_decref_decrements_refcount() {
    init();
    let bits = MoltObject::from_int(99).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(!py.is_null());

    // INCREF to get rc=2 so we can DECREF without releasing
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(py) };
    let rc_before = unsafe { (*py).ob_refcnt };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
    let rc_after = unsafe { (*py).ob_refcnt };
    assert_eq!(rc_after, rc_before - 1);

    // Final cleanup
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_decref_to_zero_releases_bridge_entry() {
    init();
    let bits = MoltObject::from_int(7777).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(!py.is_null());

    // Confirm it's in the bridge
    assert!(GLOBAL_BRIDGE.lock().pyobj_to_handle(py).is_some());

    // DECREF to zero
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };

    // After release, the mapping should be gone
    assert!(GLOBAL_BRIDGE.lock().pyobj_to_handle(py).is_none());
}

// ---------------------------------------------------------------------------
// NULL safety
// ---------------------------------------------------------------------------

#[test]
fn test_incref_null_is_noop() {
    init();
    // Must not crash
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(ptr::null_mut()) };
}

#[test]
fn test_decref_null_is_noop() {
    init();
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(ptr::null_mut()) };
}

#[test]
fn test_xincref_null_is_noop() {
    init();
    unsafe { molt_cpython_abi::api::refcount::Py_XINCREF(ptr::null_mut()) };
}

#[test]
fn test_xdecref_null_is_noop() {
    init();
    unsafe { molt_cpython_abi::api::refcount::Py_XDECREF(ptr::null_mut()) };
}

// ---------------------------------------------------------------------------
// Py_XINCREF / Py_XDECREF with non-null
// ---------------------------------------------------------------------------

#[test]
fn test_xincref_on_valid_object() {
    init();
    let bits = MoltObject::from_int(123).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    let rc_before = unsafe { (*py).ob_refcnt };
    unsafe { molt_cpython_abi::api::refcount::Py_XINCREF(py) };
    let rc_after = unsafe { (*py).ob_refcnt };
    assert_eq!(rc_after, rc_before + 1);

    // Cleanup
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

// ---------------------------------------------------------------------------
// Immortal singletons: refcount should NOT change
// ---------------------------------------------------------------------------

#[test]
fn test_incref_on_none_singleton_is_immortal() {
    init();
    let none_ptr = &raw mut Py_None;
    let rc_before = unsafe { (*none_ptr).ob_refcnt };
    assert!(rc_before >= (1 << 29), "None should be immortal");

    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(none_ptr) };
    let rc_after = unsafe { (*none_ptr).ob_refcnt };
    // Immortal objects skip the increment
    assert_eq!(rc_after, rc_before);
}

#[test]
fn test_decref_on_true_singleton_is_immortal() {
    init();
    let true_ptr = &raw mut Py_True;
    let rc_before = unsafe { (*true_ptr).ob_refcnt };
    assert!(rc_before >= (1 << 29), "True should be immortal");

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(true_ptr) };
    let rc_after = unsafe { (*true_ptr).ob_refcnt };
    assert_eq!(rc_after, rc_before);
}

#[test]
fn test_decref_on_false_singleton_is_immortal() {
    init();
    let false_ptr = &raw mut Py_False;
    let rc_before = unsafe { (*false_ptr).ob_refcnt };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(false_ptr) };
    let rc_after = unsafe { (*false_ptr).ob_refcnt };
    assert_eq!(rc_after, rc_before);
}

// ---------------------------------------------------------------------------
// Py_CLEAR
// ---------------------------------------------------------------------------

#[test]
fn test_clear_sets_pointer_to_null() {
    init();
    let bits = MoltObject::from_int(555).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    let mut slot: *mut PyObject = py;

    unsafe { molt_cpython_abi::api::refcount::Py_CLEAR(&mut slot) };
    assert!(slot.is_null());
}

#[test]
fn test_clear_null_pointer_is_noop() {
    init();
    unsafe { molt_cpython_abi::api::refcount::Py_CLEAR(ptr::null_mut()) };
}

#[test]
fn test_clear_already_null_slot() {
    init();
    let mut slot: *mut PyObject = ptr::null_mut();
    unsafe { molt_cpython_abi::api::refcount::Py_CLEAR(&mut slot) };
    assert!(slot.is_null());
}

// ---------------------------------------------------------------------------
// Multiple INCREF/DECREF cycles
// ---------------------------------------------------------------------------

#[test]
fn test_multiple_incref_decref_cycle() {
    init();
    let bits = MoltObject::from_int(9999).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };

    // INCREF 5 times
    for _ in 0..5 {
        unsafe { molt_cpython_abi::api::refcount::Py_INCREF(py) };
    }
    // rc should be initial(1) + 5 = 6
    assert_eq!(unsafe { (*py).ob_refcnt }, 6);

    // DECREF 5 times back to 1
    for _ in 0..5 {
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
    }
    assert_eq!(unsafe { (*py).ob_refcnt }, 1);

    // Final DECREF releases
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}
