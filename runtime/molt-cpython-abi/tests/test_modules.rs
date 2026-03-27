//! Tests for PyModule_New, PyModule_GetDict, PyModule_Create2,
//! PyModule_AddObject, PyModule_AddIntConstant, PyModule_AddStringConstant,
//! PyModuleDef_Init.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use std::ptr;

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
}

// ---------------------------------------------------------------------------
// PyModule_New
// ---------------------------------------------------------------------------

#[test]
fn test_module_new_non_null() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"testmod".as_ptr()) };
    assert!(!m.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_new_null_name_returns_null() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(ptr::null()) };
    assert!(m.is_null());
}

// ---------------------------------------------------------------------------
// PyModule_GetDict
// ---------------------------------------------------------------------------

#[test]
fn test_module_getdict_non_null() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"mod".as_ptr()) };
    let d = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(m) };
    // Returns the module itself as a placeholder
    assert!(!d.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_getdict_null_returns_null() {
    init();
    let d = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(ptr::null_mut()) };
    assert!(d.is_null());
}

// ---------------------------------------------------------------------------
// PyModule_AddObject
// ---------------------------------------------------------------------------

#[test]
fn test_module_addobject_null_module_returns_error() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddObject(ptr::null_mut(), c"attr".as_ptr(), val)
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(val) };
}

#[test]
fn test_module_addobject_null_name_returns_error() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"mod".as_ptr()) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::modules::PyModule_AddObject(m, ptr::null(), val) };
    assert_eq!(result, -1);
    // val ref was not stolen on error, clean up
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(val);
        molt_cpython_abi::api::refcount::Py_DECREF(m);
    }
}

#[test]
fn test_module_addobject_null_value_returns_error() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"mod".as_ptr()) };
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddObject(m, c"attr".as_ptr(), ptr::null_mut())
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

// ---------------------------------------------------------------------------
// PyModule_AddIntConstant
// ---------------------------------------------------------------------------

#[test]
fn test_module_addintconstant_null_module() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddIntConstant(ptr::null_mut(), c"X".as_ptr(), 42)
    };
    assert_eq!(result, -1);
}

// ---------------------------------------------------------------------------
// PyModule_AddStringConstant
// ---------------------------------------------------------------------------

#[test]
fn test_module_addstringconstant_null_module() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddStringConstant(
            ptr::null_mut(),
            c"Y".as_ptr(),
            c"val".as_ptr(),
        )
    };
    assert_eq!(result, -1);
}

// ---------------------------------------------------------------------------
// PyModuleDef_Init
// ---------------------------------------------------------------------------

#[test]
fn test_moduledef_init_null_returns_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(ptr::null_mut()) };
    assert!(result.is_null());
}

// ---------------------------------------------------------------------------
// PyModule_Create2
// ---------------------------------------------------------------------------

#[test]
fn test_module_create2_null_returns_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

#[test]
fn test_module_create2_with_valid_def() {
    init();
    let mut def = PyModuleDef {
        m_base: PyModuleDef_Base {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: ptr::null_mut(),
            },
            m_init: None,
            m_index: 0,
            m_copy: ptr::null_mut(),
        },
        m_name: c"testmod2".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
    assert!(!m.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_create2_null_name_uses_unnamed() {
    init();
    let mut def = PyModuleDef {
        m_base: PyModuleDef_Base {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: ptr::null_mut(),
            },
            m_init: None,
            m_index: 0,
            m_copy: ptr::null_mut(),
        },
        m_name: ptr::null(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
    assert!(!m.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}
