//! Integration gate for the native-container tier of the CPYTHON-ABI-AUDIT lane
//! F3 fixes (`object.rs`), which need the runtime hook boundary (a `classify_heap`
//! that reports `List`, plus `list_len`/`list_item`). Its own binary so the
//! process-global `RUNTIME_HOOKS` table it installs is isolated from the crate's
//! other tests. Companion to the `object::f3_divergence_tests` unit module (which
//! covers the foreign-slot dispatch tier on STUB hooks).
//!
//! The headline divergence: `bool([]) == 1`. A native empty container hit the
//! prior `else => 1` and was reported truthy; here it must be FALSY.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{MoltTypeTag, PyObject};
use molt_cpython_abi::api::object::{PyIter_Next, PyObject_IsTrue, PyObject_Size, PySeqIter_New};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::sync::atomic::{AtomicUsize, Ordering};

// A single hook table for this binary: every is_ptr handle classifies as `List`,
// `list_len` reads the `LIST_LEN` cell, and `list_item(i)` yields the native int
// `i` (so `PySequence_GetItem` drives the sequence iterator). `LIST_LEN` is only
// mutated inside the one sequential `#[test]`, so no cross-test race.
static LIST_LEN: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" fn list_classify(_bits: u64) -> u8 {
    MoltTypeTag::List as u8
}
unsafe extern "C" fn list_len_hook(_bits: u64) -> usize {
    LIST_LEN.load(Ordering::SeqCst)
}
unsafe extern "C" fn list_item_hook(_bits: u64, i: usize) -> u64 {
    MoltObject::from_int(i as i64).bits()
}

fn init_hooks() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.classify_heap = list_classify;
    hooks.list_len = list_len_hook;
    hooks.list_item = list_item_hook;
    unsafe {
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

fn native_list() -> *mut PyObject {
    // A genuine is_ptr handle -> classify_heap reports List for it.
    let backing: Box<u8> = Box::new(0);
    let bits = MoltObject::from_ptr(Box::into_raw(backing)).bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

#[test]
fn native_container_truthiness_size_and_seqiter() {
    init_hooks();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let list = native_list();

    // ── Empty native list is FALSY (the headline `bool([]) == 1` divergence) ──
    LIST_LEN.store(0, Ordering::SeqCst);
    assert_eq!(
        unsafe { PyObject_IsTrue(list) },
        0,
        "bool([]) must be 0 — an empty native list is falsy"
    );
    assert_eq!(
        unsafe { PyObject_Size(list) },
        0,
        "len([]) must be 0 via the native length authority"
    );

    // ── Non-empty native list is truthy, and Size reports its length ──
    LIST_LEN.store(3, Ordering::SeqCst);
    assert_eq!(
        unsafe { PyObject_IsTrue(list) },
        1,
        "bool([_, _, _]) must be 1"
    );
    assert_eq!(unsafe { PyObject_Size(list) }, 3, "len must be 3");

    // ── The real index-based sequence iterator drains to exhaustion, clearing
    // the terminal IndexError so the final PyIter_Next is a clean NULL. ──
    let it = unsafe { PySeqIter_New(list) };
    assert!(!it.is_null());
    let mut count = 0;
    loop {
        let item = unsafe { PyIter_Next(it) };
        if item.is_null() {
            break;
        }
        count += 1;
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(item) };
        assert!(count <= 3, "iterator must terminate at the sequence length");
    }
    assert_eq!(count, 3, "the sequence iterator must yield exactly 3 items");
    assert!(
        unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "end-of-iteration must leave NO pending exception (IndexError cleared)"
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(it) };
}
