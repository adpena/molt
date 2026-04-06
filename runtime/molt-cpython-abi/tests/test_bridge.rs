//! Tests for the ObjectBridge: handle ↔ PyObject translation, tag table,
//! singleton handling, and the global bridge init function.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use std::{f64::consts::PI, ptr};

use molt_lang_obj_model::MoltObject;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// handle_to_pyobj: primitives
// ---------------------------------------------------------------------------

#[test]
fn test_bridge_int_roundtrip() {
    init();
    let bits = MoltObject::from_int(42).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(!py.is_null());

    let recovered = GLOBAL_BRIDGE.lock().pyobj_to_handle(py);
    assert_eq!(recovered, Some(bits));

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_bridge_float_roundtrip() {
    init();
    let bits = MoltObject::from_float(PI).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(!py.is_null());

    let recovered = GLOBAL_BRIDGE.lock().pyobj_to_handle(py);
    assert_eq!(recovered, Some(bits));

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// handle_to_pyobj: singletons
// ---------------------------------------------------------------------------

#[test]
fn test_bridge_none_returns_singleton() {
    init();
    let bits = MoltObject::none().bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(std::ptr::eq(py, &raw mut Py_None));
}

#[test]
fn test_bridge_true_returns_singleton() {
    init();
    let bits = MoltObject::from_bool(true).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(std::ptr::eq(py, &raw mut Py_True));
}

#[test]
fn test_bridge_false_returns_singleton() {
    init();
    let bits = MoltObject::from_bool(false).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(std::ptr::eq(py, &raw mut Py_False));
}

// ---------------------------------------------------------------------------
// pyobj_to_handle: singletons
// ---------------------------------------------------------------------------

#[test]
fn test_pyobj_to_handle_none() {
    init();
    let none_ptr = &raw mut Py_None;
    let handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(none_ptr);
    assert_eq!(handle, Some(MoltObject::none().bits()));
}

#[test]
fn test_pyobj_to_handle_true() {
    init();
    let true_ptr = &raw mut Py_True;
    let handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(true_ptr);
    assert_eq!(handle, Some(MoltObject::from_bool(true).bits()));
}

#[test]
fn test_pyobj_to_handle_false() {
    init();
    let false_ptr = &raw mut Py_False;
    let handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(false_ptr);
    assert_eq!(handle, Some(MoltObject::from_bool(false).bits()));
}

#[test]
fn test_pyobj_to_handle_null_returns_none() {
    init();
    let handle = GLOBAL_BRIDGE.lock().pyobj_to_handle(ptr::null_mut());
    assert_eq!(handle, None);
}

// ---------------------------------------------------------------------------
// handle_to_pyobj: caching (second call should incref, not allocate)
// ---------------------------------------------------------------------------

#[test]
fn test_bridge_caches_second_lookup() {
    init();
    let bits = MoltObject::from_int(12345).bits();
    let py1 = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    let py2 = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };

    // Same pointer should be returned (cached)
    assert_eq!(py1, py2);
    // Refcount should be 2 now (initial 1 + cache hit incref)
    assert_eq!(unsafe { (*py1).ob_refcnt }, 2);

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py1);
        molt_cpython_abi::api::refcount::Py_DECREF(py2);
    }
}

// ---------------------------------------------------------------------------
// release_pyobj removes mapping
// ---------------------------------------------------------------------------

#[test]
fn test_release_pyobj_removes_mapping() {
    init();
    let bits = MoltObject::from_int(77777).bits();
    let py = unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) };
    assert!(GLOBAL_BRIDGE.lock().pyobj_to_handle(py).is_some());

    GLOBAL_BRIDGE.lock().release_pyobj(py);
    assert!(GLOBAL_BRIDGE.lock().pyobj_to_handle(py).is_none());
}

// ---------------------------------------------------------------------------
// tag_to_type
// ---------------------------------------------------------------------------

#[test]
fn test_tag_to_type_int() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Int) };
    assert!(std::ptr::eq(tp, &raw mut PyLong_Type));
}

#[test]
fn test_tag_to_type_float() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Float) };
    assert!(std::ptr::eq(tp, &raw mut PyFloat_Type));
}

#[test]
fn test_tag_to_type_str() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Str) };
    assert!(std::ptr::eq(tp, &raw mut PyUnicode_Type));
}

#[test]
fn test_tag_to_type_list() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::List) };
    assert!(std::ptr::eq(tp, &raw mut PyList_Type));
}

#[test]
fn test_tag_to_type_tuple() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Tuple) };
    assert!(std::ptr::eq(tp, &raw mut PyTuple_Type));
}

#[test]
fn test_tag_to_type_dict() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Dict) };
    assert!(std::ptr::eq(tp, &raw mut PyDict_Type));
}

#[test]
fn test_tag_to_type_bool() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Bool) };
    assert!(std::ptr::eq(tp, &raw mut PyBool_Type));
}

#[test]
fn test_tag_to_type_bytes() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Bytes) };
    assert!(std::ptr::eq(tp, &raw mut PyBytes_Type));
}

#[test]
fn test_tag_to_type_set() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Set) };
    assert!(std::ptr::eq(tp, &raw mut PySet_Type));
}

#[test]
fn test_tag_to_type_module() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Module) };
    assert!(std::ptr::eq(tp, &raw mut PyModule_Type));
}

#[test]
fn test_tag_to_type_other_falls_back() {
    init();
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Other) };
    // Falls back to PyUnicode_Type
    assert!(std::ptr::eq(tp, &raw mut PyUnicode_Type));
}

// ---------------------------------------------------------------------------
// molt_cpython_abi_init is idempotent
// ---------------------------------------------------------------------------

#[test]
fn test_init_idempotent() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    // Should not panic or corrupt state
    let tp = unsafe { molt_cpython_abi::bridge::tag_to_type(MoltTypeTag::Int) };
    assert!(!tp.is_null());
}
