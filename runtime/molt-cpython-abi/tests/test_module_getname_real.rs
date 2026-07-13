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

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::{BorrowedHandleResult, RuntimeHooks};
use std::collections::HashMap;
use std::ffi::CStr;
use std::sync::Mutex;

// Interned string bytes, keyed by handle, so str_data can return the real name.
static STR_BYTES: Mutex<Option<HashMap<u64, Vec<u8>>>> = Mutex::new(None);
static STR_INTERN: Mutex<Option<HashMap<Vec<u8>, u64>>> = Mutex::new(None);
static DICTS: Mutex<Option<HashMap<u64, HashMap<u64, u64>>>> = Mutex::new(None);
static MODULE_DICT: Mutex<Option<HashMap<u64, u64>>> = Mutex::new(None);

/// Mint a fresh NaN-boxed TAG_PTR handle backed by a real (leaked) allocation,
/// so `MoltObject::from_bits(h).is_ptr()` is true and the bridge consults the
/// `classify_heap` hook (which stamps the wrapper's ob_type). The backing buffer
/// is leaked and zeroed, so the handle stays valid and any incidental read is
/// safe; the address is only ever used as an opaque identity key.
fn fresh_handle() -> u64 {
    let buf: Box<[u8; 64]> = Box::new([0u8; 64]);
    let ptr = Box::into_raw(buf) as *mut u8;
    molt_lang_obj_model::MoltObject::from_ptr(ptr).bits()
}

fn with<T, R>(m: &Mutex<Option<T>>, f: impl FnOnce(&mut T) -> R) -> R
where
    T: Default,
{
    let mut g = m.lock().unwrap();
    if g.is_none() {
        *g = Some(T::default());
    }
    f(g.as_mut().unwrap())
}

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes = if data.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    // Intern by content so equal strings (e.g. the "__name__" key at store and
    // lookup time) share a handle, exactly like CPython's interned identifiers.
    let handle = with(&STR_INTERN, |m| {
        *m.entry(bytes.clone()).or_insert_with(fresh_handle)
    });
    // Store WITH a trailing NUL: PyUnicode_AsUTF8 returns a NUL-terminated
    // buffer, and callers (PyModule_GetName -> CStr::from_ptr) rely on it.
    let mut with_nul = bytes;
    with_nul.push(0);
    with(&STR_BYTES, |m| {
        m.entry(handle).or_insert(with_nul);
    });
    handle
}

unsafe extern "C" fn fake_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    // Return a pointer into the recorded bytes for this string handle. The bytes
    // live in the static map for the whole test, so the pointer stays valid —
    // mirroring CPython's contract that the __name__ str keeps the buffer alive.
    with(&STR_BYTES, |m| match m.get(&bits) {
        Some(v) => {
            // v is NUL-terminated; report the content length (excluding the NUL).
            if !out_len.is_null() {
                unsafe { *out_len = v.len().saturating_sub(1) };
            }
            v.as_ptr()
        }
        None => {
            if !out_len.is_null() {
                unsafe { *out_len = 0 };
            }
            std::ptr::null()
        }
    })
}

unsafe extern "C" fn fake_alloc_dict() -> u64 {
    let h = fresh_handle();
    with(&DICTS, |m| {
        m.insert(h, HashMap::new());
    });
    h
}

unsafe extern "C" fn fake_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> i32 {
    with(&DICTS, |m| {
        if let Some(d) = m.get_mut(&dict_bits) {
            d.insert(key_bits, val_bits);
        }
    });
    0
}

unsafe extern "C" fn fake_dict_get(dict_bits: u64, key_bits: u64) -> BorrowedHandleResult {
    match with(&DICTS, |m| {
        m.get(&dict_bits).and_then(|d| d.get(&key_bits).copied())
    }) {
        Some(bits) => BorrowedHandleResult::ok(bits),
        None => BorrowedHandleResult::missing(),
    }
}

unsafe extern "C" fn fake_alloc_module(data: *const u8, len: usize) -> u64 {
    let module = fresh_handle();
    // Each module gets its own dict (like CPython's md_dict).
    let dict = unsafe { fake_alloc_dict() };
    with(&MODULE_DICT, |m| {
        m.insert(module, dict);
    });
    // Note: we deliberately do NOT pre-populate __name__ here; the test sets it
    // via the real PyDict_SetItemString path to exercise the full read chain.
    let _ = (data, len);
    module
}

unsafe extern "C" fn fake_module_get_dict(module_bits: u64) -> BorrowedHandleResult {
    with(&MODULE_DICT, |m| {
        m.get(&module_bits)
            .copied()
            .map_or_else(BorrowedHandleResult::error, BorrowedHandleResult::ok)
    })
}

unsafe extern "C" fn fake_classify_heap(bits: u64) -> u8 {
    // A string handle must classify as Str so the bridge stamps its wrapper
    // pyobj with PyUnicode_Type — otherwise PyUnicode_Check(name) fails and
    // PyModule_GetName treats a valid __name__ as "nameless".
    let is_str = with(&STR_BYTES, |m| m.contains_key(&bits));
    if is_str {
        MoltTypeTag::Str as u8
    } else {
        MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn fake_noop_ref(_bits: u64) {}

fn install_hooks() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_str = fake_alloc_str;
    hooks.str_data = fake_str_data;
    hooks.alloc_dict = fake_alloc_dict;
    hooks.dict_set = fake_dict_set;
    hooks.dict_get = fake_dict_get;
    hooks.alloc_module = fake_alloc_module;
    hooks.module_get_dict_borrowed = fake_module_get_dict;
    hooks.classify_heap = fake_classify_heap;
    hooks.inc_ref = fake_noop_ref;
    hooks.dec_ref = fake_noop_ref;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

/// Build a module named `name` and set its `__name__` in its own dict via the
/// real PyDict path, then return the module pointer.
unsafe fn module_named(name: &CStr) -> *mut PyObject {
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(name.as_ptr()) };
    assert!(!m.is_null(), "PyModule_New must return a module");
    let dict = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(m) };
    assert!(!dict.is_null(), "module must have a dict");
    let name_obj = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(name.as_ptr()) };
    assert!(!name_obj.is_null());
    let rc = unsafe {
        molt_cpython_abi::api::mapping::PyDict_SetItemString(dict, c"__name__".as_ptr(), name_obj)
    };
    assert_eq!(rc, 0, "storing __name__ must succeed");
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
