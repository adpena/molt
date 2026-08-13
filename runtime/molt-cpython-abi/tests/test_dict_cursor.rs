//! Mask-proof teeth for real dict iteration: `PyDict_Next` (allocation-free O(1)
//! cursor) and `PyDict_Merge` (native-dict fast path).
//!
//! These need a fake dict model whose `dict_entry`/`dict_set`/`classify_heap`
//! hooks would collide with another test file's first-wins `RUNTIME_HOOKS`
//! OnceLock, so they get their own test binary (fresh OnceLock). A process-wide
//! `TEST_LOCK` serializes the two tests because they share the fake dict statics.
//!
//! LOAD-BEARING revert proof (reproduced manually per M05): reverting
//! `PyDict_Next` to its pre-fix stub (`*pos = size; return 0` + RuntimeError when
//! size>0) makes `collected` come back EMPTY with a stray pending exception, so
//! `next_yields_all_entries` fails both assertions; restoring `PyDict_Merge`'s old
//! `RuntimeError` body makes `merge_populates_target` fail with rc == -1 / 0 sets.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{MoltTypeTag, Py_ssize_t, PyObject};
use molt_lang_obj_model::MoltObject;
use std::ptr;
use std::sync::Mutex;

// Serializes the two tests below (they share the fake dict statics).
static TEST_LOCK: Mutex<()> = Mutex::new(());
// The fake `other` dict's entries (key_bits, val_bits), indexed by the cursor.
static ENTRIES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
// Recorded (dict_bits, key_bits, val_bits) writes via dict_set.
static SETS: Mutex<Vec<(u64, u64, u64)>> = Mutex::new(Vec::new());
// Keys reported present by dict_get (drives the merge override path).
static PRESENT: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static CLEARS: Mutex<Vec<u64>> = Mutex::new(Vec::new());

unsafe extern "C" fn fx_dict_entry(
    _d: u64,
    index: usize,
    out_key: *mut u64,
    out_val: *mut u64,
) -> std::os::raw::c_int {
    let e = ENTRIES.lock().unwrap();
    match e.get(index) {
        Some(&(k, v)) => {
            unsafe {
                if !out_key.is_null() {
                    *out_key = k;
                }
                if !out_val.is_null() {
                    *out_val = v;
                }
            }
            1
        }
        None => 0,
    }
}
unsafe extern "C" fn fx_classify_heap(_b: u64) -> u8 {
    MoltTypeTag::Dict as u8
}
unsafe extern "C" fn fx_dict_set(d: u64, k: u64, v: u64) -> i32 {
    SETS.lock().unwrap().push((d, k, v));
    0
}
unsafe extern "C" fn fx_dict_get(_d: u64, k: u64) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    if PRESENT.lock().unwrap().contains(&k) {
        molt_cpython_abi::hooks::BorrowedHandleResult::ok(k)
    } else {
        molt_cpython_abi::hooks::BorrowedHandleResult::missing()
    }
}
unsafe extern "C" fn fx_dict_len(_b: u64) -> usize {
    ENTRIES.lock().unwrap().len()
}
unsafe extern "C" fn fx_dict_op(op: u32, dict: u64) -> u64 {
    if op == molt_cpython_abi::DictOp::Clear as u32 {
        CLEARS.lock().unwrap().push(dict);
        ENTRIES.lock().unwrap().clear();
        MoltObject::none().bits()
    } else {
        0
    }
}

fn install() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.dict_entry = fx_dict_entry;
    hooks.classify_heap = fx_classify_heap;
    hooks.dict_set = fx_dict_set;
    hooks.dict_get = fx_dict_get;
    hooks.dict_len = fx_dict_len;
    hooks.dict_op = fx_dict_op;
    support::prepare_abi_test_thread(hooks);
}

// A ptr-tagged handle whose inner pointer is never dereferenced: classify is
// faked and the handle only flows through faked hooks + bridge identity maps.
fn fake_dict_handle(addr: usize) -> u64 {
    MoltObject::from_ptr(addr as *mut u8).bits()
}

fn register(handle: u64) -> *mut PyObject {
    unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(handle) }
}
fn handle_of(p: *mut PyObject) -> u64 {
    molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .pyobj_to_handle(p)
        .map(|identity| identity.as_handle())
        .expect("pointer must round-trip to a handle")
}

#[test]
fn next_yields_all_entries_no_exception() {
    let _guard = TEST_LOCK.lock().unwrap();
    install();
    let (k1, v1) = (
        MoltObject::from_int(0x1111).bits(),
        MoltObject::from_int(0x2222).bits(),
    );
    let (k2, v2) = (
        MoltObject::from_int(0x3333).bits(),
        MoltObject::from_int(0x4444).bits(),
    );
    *ENTRIES.lock().unwrap() = vec![(k1, v1), (k2, v2)];

    let dict = register(fake_dict_handle(0x5000));
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let mut pos: Py_ssize_t = 0;
    let mut key: *mut PyObject = ptr::null_mut();
    let mut val: *mut PyObject = ptr::null_mut();
    let mut collected: Vec<(u64, u64)> = Vec::new();
    while unsafe {
        molt_cpython_abi::api::mapping::PyDict_Next(dict, &raw mut pos, &raw mut key, &raw mut val)
    } == 1
    {
        collected.push((handle_of(key), handle_of(val)));
        assert!(collected.len() <= 2, "cursor failed to terminate");
    }
    assert_eq!(
        collected,
        vec![(k1, v1), (k2, v2)],
        "PyDict_Next must yield every entry in order, not observe an empty dict"
    );
    assert!(
        unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "PyDict_Next must NOT leave a stray pending exception on normal termination"
    );
}

#[test]
fn merge_populates_target_override() {
    let _guard = TEST_LOCK.lock().unwrap();
    install();
    let (k1, v1) = (
        MoltObject::from_int(0x1a1a).bits(),
        MoltObject::from_int(0x2b2b).bits(),
    );
    let (k2, v2) = (
        MoltObject::from_int(0x3c3c).bits(),
        MoltObject::from_int(0x4d4d).bits(),
    );
    *ENTRIES.lock().unwrap() = vec![(k1, v1), (k2, v2)];
    SETS.lock().unwrap().clear();
    PRESENT.lock().unwrap().clear();

    let op = register(fake_dict_handle(0x6000));
    let other = register(fake_dict_handle(0x7000));
    let op_bits = handle_of(op);
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    // override == 1: overwrite unconditionally — must copy BOTH pairs into op.
    let rc = unsafe { molt_cpython_abi::api::mapping::PyDict_Merge(op, other, 1) };
    assert_eq!(rc, 0, "PyDict_Merge must succeed, not RuntimeError");
    let sets = SETS.lock().unwrap();
    assert_eq!(
        sets.len(),
        2,
        "every source entry must be set into the target"
    );
    assert!(
        sets.iter().all(|(d, _, _)| *d == op_bits),
        "merge must write into the target dict handle"
    );
    let keys: Vec<u64> = sets.iter().map(|(_, k, _)| *k).collect();
    assert!(
        keys.contains(&k1) && keys.contains(&k2),
        "both source keys must be merged"
    );
}

#[test]
fn update_overwrites_and_clear_empties() {
    let _guard = TEST_LOCK.lock().unwrap();
    install();
    let key = MoltObject::from_int(0x55).bits();
    let old_value = MoltObject::from_int(0x66).bits();
    let new_value = MoltObject::from_int(0x77).bits();
    *ENTRIES.lock().unwrap() = vec![(key, new_value)];
    *PRESENT.lock().unwrap() = vec![key];
    SETS.lock().unwrap().clear();
    CLEARS.lock().unwrap().clear();
    let target = register(fake_dict_handle(0x8100));
    let source = register(fake_dict_handle(0x8200));
    let target_bits = handle_of(target);
    SETS.lock().unwrap().push((target_bits, key, old_value));

    assert_eq!(
        unsafe { molt_cpython_abi::api::mapping::PyDict_Update(target, source) },
        0
    );
    assert!(
        SETS.lock()
            .unwrap()
            .contains(&(target_bits, key, new_value))
    );

    unsafe { molt_cpython_abi::api::mapping::PyDict_Clear(target) };
    assert_eq!(&*CLEARS.lock().unwrap(), &[target_bits]);
    assert!(ENTRIES.lock().unwrap().is_empty());
}

#[test]
fn dict_proxy_is_read_only() {
    let _guard = TEST_LOCK.lock().unwrap();
    install();
    let dict = register(fake_dict_handle(0x8300));
    let proxy = unsafe { molt_cpython_abi::api::mapping::PyDictProxy_New(dict) };
    assert!(!proxy.is_null());
    let key = register(MoltObject::from_int(1).bits());
    let value = register(MoltObject::from_int(2).bits());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyObject_SetItem(proxy, key, value) },
        -1
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(proxy);
    }
}
