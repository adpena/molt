//! Mask-proof regression for POISON Lane A #2 — `add_methods_to_dict` fail-open.
//!
//! `PyType_Ready` populates `tp_dict` from `tp_methods` via
//! `add_methods_to_dict`: for each method it builds a `PyCFunction` and calls
//! `PyDict_SetItemString(dict, name, func)`. The bug: on a `PyDict_SetItemString`
//! failure it recorded a silent failure but returned READY(0) with the method
//! SILENTLY DROPPED from `tp_dict` — inverse of the sibling `add_members_` /
//! `add_getset_` paths (which return -1). A numpy scalar/DType type could be
//! marked ready while missing methods, surfacing much later as an
//! `AttributeError` / wrong dispatch with no exec-time failure.
//!
//! CPython's `add_methods` (Objects/typeobject.c) propagates a `PyDict` store
//! failure as -1 so `PyType_Ready` FAILS CLOSED. This test reproduces the store
//! failure deterministically: it installs a runtime backend WITHOUT
//! `register_c_function`, so `PyCFunction_NewEx` falls back to a raw,
//! non-bridge-registered object that `PyDict_SetItem` cannot store ("unresolved
//! value") — the exact witness failure shape. It asserts `PyType_Ready` returns
//! -1 with a pending exception (never READY-with-dropped-method).
//!
//! Pre-fix this test FAILS (rc == 0, no exception, method missing); post-fix it
//! PASSES (rc == -1, exception pending). Dedicated test binary so it owns a
//! fresh runtime-hooks `OnceLock` with the `register_c_function` stub intact.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::{BorrowedHandleResult, RuntimeHooks};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::os::raw::c_char;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x6100_0000);
static DICTS: Mutex<Option<HashMap<u64, HashMap<u64, u64>>>> = Mutex::new(None);

fn fresh_handle() -> u64 {
    let address = NEXT_HANDLE.fetch_add(0x10, Ordering::Relaxed) as usize;
    MoltObject::from_ptr(ptr::with_exposed_provenance_mut(address)).bits()
}

fn dicts() -> std::sync::MutexGuard<'static, Option<HashMap<u64, HashMap<u64, u64>>>> {
    let mut g = DICTS.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}

unsafe extern "C" fn fake_alloc_dict() -> u64 {
    let h = fresh_handle();
    dicts().as_mut().unwrap().insert(h, HashMap::new());
    h
}

unsafe extern "C" fn fake_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> i32 {
    if let Some(m) = dicts().as_mut().unwrap().get_mut(&dict_bits) {
        m.insert(key_bits, val_bits);
    }
    0
}

unsafe extern "C" fn fake_dict_get(dict_bits: u64, key_bits: u64) -> BorrowedHandleResult {
    match dicts()
        .as_ref()
        .unwrap()
        .get(&dict_bits)
        .and_then(|m| m.get(&key_bits).copied())
    {
        Some(bits) => BorrowedHandleResult::ok(bits),
        None => BorrowedHandleResult::missing(),
    }
}

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    static STR_HANDLES: Mutex<Option<HashMap<Vec<u8>, u64>>> = Mutex::new(None);
    let bytes = if data.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    let mut g = STR_HANDLES.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    *g.as_mut()
        .unwrap()
        .entry(bytes)
        .or_insert_with(fresh_handle)
}

unsafe extern "C" fn fake_classify_heap(_bits: u64) -> u8 {
    0xFF
}

unsafe extern "C" fn fake_noop_ref(_bits: u64) {}

/// Install a dict/str backend but deliberately leave `register_c_function` as
/// the STUB (returns 0), so `PyCFunction_NewEx` yields a raw, non-bridge-
/// registered object that cannot be stored in a dict.
fn install_hooks_without_cfunction_registration() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_dict = fake_alloc_dict;
    hooks.dict_set = fake_dict_set;
    hooks.dict_get = fake_dict_get;
    hooks.alloc_str = fake_alloc_str;
    hooks.classify_heap = fake_classify_heap;
    hooks.inc_ref = fake_noop_ref;
    hooks.dec_ref = fake_noop_ref;
    // NOTE: hooks.register_c_function is intentionally left as the stub.
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

unsafe extern "C" fn dummy_method(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

fn method_def(name: &'static [u8]) -> PyMethodDef {
    PyMethodDef {
        ml_name: name.as_ptr() as *const c_char,
        ml_meth: Some(dummy_method),
        ml_flags: METH_VARARGS,
        ml_doc: ptr::null(),
    }
}

#[test]
fn type_ready_fails_closed_when_method_store_fails() {
    install_hooks_without_cfunction_registration();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let mut methods = [
        method_def(b"reduce\0"),
        PyMethodDef {
            ml_name: ptr::null(),
            ml_meth: None,
            ml_flags: 0,
            ml_doc: ptr::null(),
        },
    ];
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"scalar_store_fail".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    tp.tp_methods = methods.as_mut_ptr();

    let rc = unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(&mut tp) };

    // The whole point of the fix: a method that cannot be stored in tp_dict must
    // FAIL PyType_Ready, not leave a "ready" type with a silently-dropped method.
    assert_eq!(
        rc, -1,
        "PyType_Ready must fail closed when a tp_methods entry cannot be stored \
         in tp_dict (pre-fix returned 0 with the method silently dropped)"
    );
    let pending = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(
        !pending.is_null(),
        "a method-store failure must leave a pending exception (never a contentless -1)"
    );
    // The type must NOT be advertised as READY when a declared method was dropped.
    assert_eq!(
        tp.tp_flags & Py_TPFLAGS_READY,
        0,
        "a type whose method population failed must not be marked READY"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
