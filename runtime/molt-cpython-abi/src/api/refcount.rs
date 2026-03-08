//! Reference counting — Py_INCREF / Py_DECREF and variants.
//!
//! In a pure CPython world these are hot inlined macros. In our bridge they
//! are real functions because C extensions call them via PLT. We keep them as
//! `#[inline(always)]` to give the compiler maximum optimisation latitude when
//! the bridge itself calls them.
//!
//! The `ob_refcnt` field in the bridge `PyObject` header is a *logical* count
//! separate from Molt's garbage collector. The Molt GC holds the canonical
//! lifetime; bridge logical counts only drive `release_pyobj` when an
//! extension explicitly deallocates its references.

use crate::abi_types::PyObject;
use std::ptr;

/// Increment the reference count.
///
/// # Safety
/// `op` must be a non-null bridge-managed PyObject.
#[unsafe(no_mangle)]
#[inline(always)]
pub unsafe extern "C" fn Py_INCREF(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    // Immortal check: large refcnt means static singleton, skip.
    unsafe {
        let rc = (*op).ob_refcnt;
        if rc < (1 << 29) {
            (*op).ob_refcnt = rc.wrapping_add(1);
        }
    }
}

/// Decrement the reference count. Releases the bridge entry when it hits zero.
///
/// # Safety
/// `op` must be a non-null bridge-managed PyObject.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_DECREF(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe {
        let rc = (*op).ob_refcnt;
        if rc >= (1 << 29) {
            return; // immortal singleton
        }
        let new_rc = rc.wrapping_sub(1);
        (*op).ob_refcnt = new_rc;
        if new_rc == 0 {
            crate::bridge::GLOBAL_BRIDGE.lock().release_pyobj(op);
        }
    }
}

/// `Py_INCREF` that accepts null (null is silently ignored).
#[unsafe(no_mangle)]
#[inline(always)]
pub unsafe extern "C" fn Py_XINCREF(op: *mut PyObject) {
    if !op.is_null() {
        unsafe { Py_INCREF(op) };
    }
}

/// `Py_DECREF` that accepts null (null is silently ignored).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_XDECREF(op: *mut PyObject) {
    if !op.is_null() {
        unsafe { Py_DECREF(op) };
    }
}

/// Clear a `*mut PyObject` pointer: Py_XDECREF + set to NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn Py_CLEAR(op: *mut *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe {
        let tmp = *op;
        if !tmp.is_null() {
            *op = ptr::null_mut();
            Py_DECREF(tmp);
        }
    }
}
