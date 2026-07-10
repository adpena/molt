//! Weak reference C-API surface.
//!
//! This ABI mints no weakref objects (there is no `PyWeakref_NewRef` surface),
//! so `PyWeakref_Check` is honestly 0 for everything and `PyWeakref_GetObject`
//! fails loud for every argument — per CPython, a non-weakref argument is a
//! `PyErr_BadInternalCall` (SystemError). The previous body fabricated a
//! borrowed `Py_None` "referent" for ANY non-null argument, which callers then
//! treated as the real referent (silent wrong object, ledger THEATER row
//! `weakref.rs:13`). When a weakref object model lands, resolve the actual
//! referent here and return `Py_None` only for a cleared reference.

use crate::abi_types::PyObject;
use std::os::raw::c_int;
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyWeakref_Check(_op: *mut PyObject) -> c_int {
    // No weakref type exists in this ABI; nothing can be a weakref.
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyWeakref_GetObject(ref_obj: *mut PyObject) -> *mut PyObject {
    // CPython Objects/weakrefobject.c: non-weakref -> PyErr_BadInternalCall()
    // + NULL. Every argument is a non-weakref until a weakref model exists.
    let _ = ref_obj;
    unsafe { crate::api::errors::PyErr_BadInternalCall() };
    ptr::null_mut()
}
