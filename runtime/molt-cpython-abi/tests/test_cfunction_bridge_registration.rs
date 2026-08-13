//! PyCFunction_NewEx must return a *bridge-registered* PyObject when a runtime
//! is wired, so the callable resolves back to a Molt handle via
//! `pyobj_to_handle`. This is the exact contract that PyType_Ready's tp_dict
//! method population depends on: `add_methods_to_dict` calls PyCFunction_NewEx
//! then `PyDict_SetItemString(dict, name, func)`; if `func` is not
//! bridge-resolvable, `PyDict_SetItem` records "unresolved value" and the method
//! descriptor is silently dropped — which is exactly how numpy's ufunc/dtype
//! scalar methods failed to land on the wasm witness path.
//!
//! This is a dedicated test binary so it owns a fresh `OnceLock` for the runtime
//! hook vtable: it installs a minimal hook set with a working
//! `register_c_function` and dict backend, then proves the full store/retrieve
//! chain. (Pure STUB hooks make register_c_function return 0, exercising only
//! the raw-object fallback; the real bridge path needs a live runtime, mirrored
//! here by the minimal fakes.)

#![allow(non_snake_case)]

mod support;
use support::fake_foreign;

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::{BorrowedHandleResult, RuntimeHooks};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

// ── Minimal fake runtime backend ───────────────────────────────────────────
// A tiny handle-keyed store: dicts are handles mapping key-bits -> value-bits.

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x4000_0000);
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

unsafe extern "C" fn fake_register_c_function(
    _meth: u64,
    _flags: c_int,
    _self_bits: u64,
    _data: *const u8,
    _len: usize,
) -> u64 {
    // A distinct, non-zero, ptr-shaped handle for each registered callable.
    fresh_handle()
}

unsafe extern "C" fn fake_alloc_dict() -> u64 {
    let h = fresh_handle();
    dicts().as_mut().unwrap().insert(h, HashMap::new());
    h
}

unsafe extern "C" fn fake_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> i32 {
    if let Some(map) = dicts().as_mut().unwrap().get_mut(&dict_bits) {
        map.insert(key_bits, val_bits);
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
    // Intern by content so equal strings share a handle (dict keys must match).
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
    // Treat everything as "Other"; the bridge only needs a stable tag.
    0xFF
}

unsafe extern "C" fn fake_noop_ref(_bits: u64) {}

fn install_hooks() {
    // Start from the crate's stub table and override only what this test needs,
    // so we do not have to spell out the entire ~40-entry vtable.
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.register_c_function = fake_register_c_function;
    hooks.alloc_dict = fake_alloc_dict;
    hooks.dict_set = fake_dict_set;
    hooks.dict_get = fake_dict_get;
    hooks.alloc_str = fake_alloc_str;
    hooks.classify_heap = fake_classify_heap;
    hooks.inc_ref = fake_noop_ref;
    hooks.dec_ref = fake_noop_ref;
    hooks.foreign_new = fake_foreign::foreign_new;
    support::prepare_abi_test_thread(hooks);
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

// ── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn cfunction_newex_returns_bridge_resolvable_object() {
    install_hooks();
    let mut ml = method_def(b"reduce\0");
    let func = unsafe {
        molt_cpython_abi::api::object::PyCFunction_NewEx(&mut ml, ptr::null_mut(), ptr::null_mut())
    };
    assert!(!func.is_null(), "PyCFunction_NewEx must return a callable");
    assert_eq!(
        unsafe { (*func).ob_type },
        &raw mut PyCFunction_Type,
        "runtime-backed C functions must retain exact PyCFunction_Type identity",
    );
    assert_eq!(
        unsafe { (*(func.cast::<PyCFunctionObject>())).m_ml },
        &raw mut ml,
        "runtime-backed C functions must retain their PyMethodDef layout",
    );
    // The returned object must resolve back to a Molt handle — the whole point
    // of the fix. A raw, unregistered object would return None here.
    let handle = molt_cpython_abi::bridge::GLOBAL_BRIDGE.pyobj_to_handle(func);
    assert!(
        handle.is_some(),
        "PyCFunction_NewEx result must be bridge-registered so PyDict_SetItem can store it"
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(func) };
}

#[test]
fn cfunction_descriptor_stores_and_retrieves_in_type_dict() {
    install_hooks();
    // Full chain: PyType_Ready populates tp_dict from tp_methods, storing each
    // method's PyCFunction; the descriptor must be retrievable by name.
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
    tp.ob_base.ob_base.ob_refcnt = 1;
    tp.tp_name = c"scalar".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    tp.tp_methods = methods.as_mut_ptr();

    let rc = unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(&mut tp) };
    assert_eq!(rc, 0);
    assert!(!tp.tp_dict.is_null());

    let key = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"reduce".as_ptr()) };
    let found = unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(tp.tp_dict, key) };
    assert!(
        !found.is_null(),
        "method descriptor stored by PyType_Ready must be retrievable from tp_dict"
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(key);
        molt_cpython_abi::api::refcount::Py_DECREF(tp.tp_mro);
        molt_cpython_abi::api::refcount::Py_DECREF(tp.tp_dict);
    }
}

#[test]
fn cfunction_newex_null_methoddef_returns_null() {
    install_hooks();
    let out = unsafe {
        molt_cpython_abi::api::object::PyCFunction_NewEx(
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    assert!(out.is_null());
}
