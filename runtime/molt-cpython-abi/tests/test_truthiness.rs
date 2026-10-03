//! The managed inquiry boundary must use semantic hooks even when physical
//! storage reports a different length. Runtime protocol behavior is exercised
//! in molt-runtime's cpython_abi_hooks::inquiry_tests; foreign C slot behavior
//! is covered by object::f3_divergence_tests. This binary isolates its hooks.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{MoltTypeTag, PyObject};
use molt_cpython_abi::api::object::{
    PyIter_Next, PyObject_IsTrue, PyObject_Length, PyObject_Not, PyObject_Size, PySeqIter_New,
};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::sync::atomic::{AtomicI32, AtomicIsize, AtomicUsize, Ordering};

// A single hook table for this binary: every is_ptr handle classifies as `List`,
// `list_len` reads the `LIST_LEN` cell, and `list_item(i)` yields the native int
// `i` (so `PySequence_GetItem` drives the sequence iterator). `LIST_LEN` is only
// mutated inside the one sequential `#[test]`, so no cross-test race.
static LIST_LEN: AtomicUsize = AtomicUsize::new(0);
static OBJECT_LENGTH: AtomicIsize = AtomicIsize::new(0);
static OBJECT_TRUTH: AtomicI32 = AtomicI32::new(0);

#[repr(align(16))]
struct ListBacking(u8);

static mut EMPTY_LIST_BACKING: ListBacking = ListBacking(0);
static mut NONEMPTY_LIST_BACKING: ListBacking = ListBacking(0);

unsafe extern "C" fn list_classify(_bits: u64) -> u8 {
    MoltTypeTag::List as u8
}
unsafe extern "C" fn list_len_hook(_bits: u64) -> usize {
    LIST_LEN.load(Ordering::SeqCst)
}
unsafe extern "C" fn object_length_hook(_bits: u64) -> isize {
    OBJECT_LENGTH.load(Ordering::SeqCst)
}
unsafe extern "C" fn object_truth_hook(_bits: u64) -> i32 {
    OBJECT_TRUTH.load(Ordering::SeqCst)
}
unsafe extern "C" fn list_item_hook(
    _bits: u64,
    i: usize,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    molt_cpython_abi::hooks::BorrowedHandleResult::ok(MoltObject::from_int(i as i64).bits())
}

fn init_hooks() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.classify_heap = list_classify;
    hooks.list_len = list_len_hook;
    hooks.list_item = list_item_hook;
    hooks.object_length = object_length_hook;
    hooks.object_is_true = object_truth_hook;
    support::prepare_abi_test_thread(hooks);
}

fn native_list(nonempty: bool) -> *mut PyObject {
    // Genuine, stable, aligned pointer handles whose storage outlives the
    // process-global bridge. Each projected list is an immutable runtime
    // snapshot; runtime mutations use the bridge publication authority.
    let backing = unsafe {
        if nonempty {
            &raw mut NONEMPTY_LIST_BACKING.0
        } else {
            &raw mut EMPTY_LIST_BACKING.0
        }
    };
    let bits = MoltObject::from_ptr(backing).bits();
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}

#[test]
fn native_container_truthiness_size_and_seqiter() {
    init_hooks();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    // ── Empty native list is FALSY (the headline `bool([]) == 1` divergence) ──
    LIST_LEN.store(0, Ordering::SeqCst);
    let empty = native_list(false);
    assert_eq!(
        unsafe { PyObject_IsTrue(empty) },
        0,
        "bool([]) must be 0 — an empty native list is falsy"
    );
    assert_eq!(
        unsafe { PyObject_Size(empty) },
        0,
        "len([]) must be 0 via the native length authority"
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(empty) };

    // ── Non-empty native list is truthy, and Size reports its length ──
    LIST_LEN.store(3, Ordering::SeqCst);
    OBJECT_LENGTH.store(3, Ordering::SeqCst);
    OBJECT_TRUTH.store(1, Ordering::SeqCst);
    let list = native_list(true);
    assert_eq!(
        unsafe { PyObject_IsTrue(list) },
        1,
        "bool([_, _, _]) must be 1"
    );
    assert_eq!(unsafe { PyObject_Size(list) }, 3, "len must be 3");

    // A nonempty physical list can be semantically empty. Size and its alias
    // must retain the complete signed result, not a status validator's zero.
    OBJECT_LENGTH.store(0, Ordering::SeqCst);
    OBJECT_TRUTH.store(0, Ordering::SeqCst);
    assert_eq!(unsafe { PyObject_IsTrue(list) }, 0);
    assert_eq!(unsafe { PyObject_Not(list) }, 1);
    assert_eq!(unsafe { PyObject_Size(list) }, 0);
    OBJECT_LENGTH.store(isize::MAX, Ordering::SeqCst);
    assert_eq!(unsafe { PyObject_Length(list) }, isize::MAX);
    OBJECT_LENGTH.store(3, Ordering::SeqCst);
    OBJECT_TRUTH.store(1, Ordering::SeqCst);

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
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}
