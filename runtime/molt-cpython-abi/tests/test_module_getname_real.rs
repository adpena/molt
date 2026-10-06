//! Mask-proof regression for POISON Lane A #4 — `PyModule_GetName` theater.
//!
//! `PyModule_GetName` returned the HARDCODED constant `c"molt.module"` for every
//! non-null module instead of the module's real `__name__`. CPython's
//! `PyImport_AddModule(PyModule_GetName(m))` keys the module registry by that
//! name, so a fabricated constant collapses every module under one key
//! (HIDDEN_THEATER, M05). The fix reads the actual `__name__` from the module
//! dict (moduleobject.c `PyModule_GetNameObject` → `PyUnicode_AsUTF8`).
//!
//! This test builds two modules with distinct names, sets each one's `__name__`
//! in its own dict, and asserts `PyModule_GetName` returns each real name and
//! that the two differ. Pre-fix both returned "molt.module" (the names are equal
//! and wrong) → FAILS; post-fix each returns its own name → PASSES. Dedicated
//! test binary: it owns a fresh runtime-hooks `OnceLock` with a string/dict/
//! module backend rich enough to round-trip `__name__`.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::RuntimeHooks;
use std::ffi::CStr;

fn install_hooks() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    support::prepare_runtime_class_abi_test_thread(hooks);
}

/// Build a module named `name` and set its `__name__` in its own dict via the
/// real PyDict path, then return the module pointer.
unsafe fn module_named(name: &CStr) -> *mut PyObject {
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(name.as_ptr()) };
    assert!(!m.is_null(), "PyModule_New must return a module");
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyModule_CheckExact(m) },
        1
    );
    let dict = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(m) };
    assert!(!dict.is_null(), "module must have a dict");
    let name_obj = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(name.as_ptr()) };
    assert!(!name_obj.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::strings::PyUnicode_CheckExact(name_obj) },
        1
    );
    let rc = unsafe {
        molt_cpython_abi::api::mapping::PyDict_SetItemString(dict, c"__name__".as_ptr(), name_obj)
    };
    assert_eq!(rc, 0, "storing __name__ must succeed");
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(name_obj) };
    m
}

#[test]
fn module_getname_returns_real_distinct_names() {
    install_hooks();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let m1 = unsafe { module_named(c"numpy._core._multiarray_umath") };
    let m2 = unsafe { module_named(c"scipy._lib._ccallback_c") };

    let n1 = unsafe { molt_cpython_abi::api::modules::PyModule_GetName(m1) };
    let n2 = unsafe { molt_cpython_abi::api::modules::PyModule_GetName(m2) };
    assert!(!n1.is_null() && !n2.is_null(), "names must resolve");

    let s1 = unsafe { CStr::from_ptr(n1) }.to_str().unwrap();
    let s2 = unsafe { CStr::from_ptr(n2) }.to_str().unwrap();

    // The core of the fix: each module reports its OWN name, not a constant.
    assert_eq!(s1, "numpy._core._multiarray_umath");
    assert_eq!(s2, "scipy._lib._ccallback_c");
    assert_ne!(
        s1, s2,
        "distinct modules must have distinct names (pre-fix both were 'molt.module')"
    );
    assert_ne!(
        s1, "molt.module",
        "PyModule_GetName must not fabricate a constant name"
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(m1);
        molt_cpython_abi::api::refcount::Py_DECREF(m2);
    }
}

#[test]
fn module_getname_null_sets_systemerror() {
    install_hooks();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let n = unsafe { molt_cpython_abi::api::modules::PyModule_GetName(std::ptr::null_mut()) };
    assert!(n.is_null(), "NULL module must return NULL");
    let pending = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(!pending.is_null(), "NULL module must set an exception");
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
