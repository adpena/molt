//! Mask-proof teeth for real dict iteration: `PyDict_Next` (allocation-free O(1)
//! cursor) and `PyDict_Merge` (native-dict fast path).
//!
//! These need a fake dict model whose `dict_entry`/`dict_set`/`classify_heap`
//! hooks would collide with another test file's first-wins `RUNTIME_HOOKS`
//! OnceLock, so they get their own test binary (fresh OnceLock). A process-wide
//! shared support transaction serializes the fixture and builtin-root lifetime.
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
use std::collections::{HashMap, HashSet};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// The fake `other` dict's entries (key_bits, val_bits), indexed by the cursor.
static ENTRIES: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());
// Recorded (dict_bits, key_bits, val_bits) writes via dict_set.
static SETS: Mutex<Vec<(u64, u64, u64)>> = Mutex::new(Vec::new());
// Keys reported present by dict_get (drives the merge override path).
static PRESENT: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static CLEARS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static LOOKUP_KEYS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static FIXTURE_DICTS: Mutex<Option<HashSet<u64>>> = Mutex::new(None);
static PROXIES: Mutex<Option<HashMap<u64, u64>>> = Mutex::new(None);
static FOREIGN_C_PTR: AtomicUsize = AtomicUsize::new(0);
static FOREIGN_WRAPPER: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn fx_dict_entry(
    d: u64,
    index: usize,
    out_key: *mut u64,
    out_val: *mut u64,
) -> std::os::raw::c_int {
    if !is_fixture_dict(d) {
        return unsafe { support::fake_runtime::dict_entry(d, index, out_key, out_val) };
    }
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
unsafe extern "C" fn fx_classify_heap(bits: u64) -> u8 {
    if is_fixture_dict(bits) {
        MoltTypeTag::Dict as u8
    } else {
        unsafe { support::fake_runtime::classify_heap(bits) }
    }
}
unsafe extern "C" fn fx_dict_set(d: u64, k: u64, v: u64) -> i32 {
    if !is_fixture_dict(d) {
        return unsafe { support::fake_runtime::dict_set(d, k, v) };
    }
    SETS.lock().unwrap().push((d, k, v));
    0
}
unsafe extern "C" fn resolve_fixture_dict(
    bits: u64,
    merge_source: u8,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    if is_fixture_dict(bits) {
        molt_cpython_abi::hooks::BorrowedHandleResult::ok(bits)
    } else {
        unsafe { support::fake_runtime::dict_resolve(bits, merge_source) }
    }
}

unsafe extern "C" fn fx_dict_get(
    d: u64,
    k: u64,
    source: molt_cpython_abi::hooks::DictHashSource,
    hash: i64,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    if !is_fixture_dict(d) {
        return unsafe { support::fake_runtime::dict_get(d, k, source, hash) };
    }
    LOOKUP_KEYS.lock().unwrap().push(k);
    if PRESENT.lock().unwrap().contains(&k) {
        molt_cpython_abi::hooks::BorrowedHandleResult::ok(k)
    } else {
        molt_cpython_abi::hooks::BorrowedHandleResult::missing()
    }
}
unsafe extern "C" fn fx_foreign_new(c_ptr: usize) -> u64 {
    let bits = unsafe { support::fake_runtime::foreign_new(c_ptr) };
    // Only the deliberately opaque foreign key is this test's tracked probe.
    // Native exception/type helpers use the same shared foreign custody model.
    if unsafe { (*(c_ptr as *mut PyObject)).ob_type.is_null() } {
        assert_eq!(FOREIGN_C_PTR.swap(c_ptr, Ordering::SeqCst), 0);
        assert_eq!(FOREIGN_WRAPPER.swap(bits, Ordering::SeqCst), 0);
    }
    bits
}
unsafe extern "C" fn fx_dec_ref(bits: u64) {
    unsafe { support::fake_runtime::dec_ref(bits) };
    if unsafe { support::fake_runtime::ref_count(bits) } == 0 {
        if let Some(dicts) = FIXTURE_DICTS.lock().unwrap().as_mut() {
            dicts.remove(&bits);
        }
        let dict = PROXIES
            .lock()
            .unwrap()
            .as_mut()
            .and_then(|proxies| proxies.remove(&bits));
        if let Some(dict) = dict {
            unsafe { fx_dec_ref(dict) };
        }
    }
    if bits != 0
        && bits == FOREIGN_WRAPPER.load(Ordering::SeqCst)
        && unsafe { support::fake_runtime::ref_count(bits) } == 0
    {
        FOREIGN_WRAPPER.store(0, Ordering::SeqCst);
        assert_ne!(FOREIGN_C_PTR.swap(0, Ordering::SeqCst), 0);
    }
}
unsafe extern "C" fn fx_mappingproxy_new(dict: u64) -> molt_cpython_abi::hooks::OwnedHandleResult {
    let proxy = support::fake_runtime::fresh_handle();
    unsafe { support::fake_runtime::inc_ref(dict) };
    PROXIES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(proxy, dict);
    molt_cpython_abi::hooks::OwnedHandleResult::ok(proxy)
}
unsafe extern "C" fn fx_object_set_item(object: u64, _: u64, _: *const u64) -> i32 {
    assert!(
        PROXIES
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|proxies| proxies.contains_key(&object))
    );
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
            c"mappingproxy does not support item assignment".as_ptr(),
        );
    }
    -1
}
unsafe extern "C" fn fx_dict_len(bits: u64) -> usize {
    if is_fixture_dict(bits) {
        ENTRIES.lock().unwrap().len()
    } else {
        unsafe { support::fake_runtime::dict_len(bits) }
    }
}
unsafe extern "C" fn fx_dict_op(op: u32, dict: u64) -> u64 {
    if !is_fixture_dict(dict) {
        return unsafe { support::fake_runtime::dict_op(op, dict) };
    }
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
    support::fake_runtime::wire(&mut hooks);
    hooks.mappingproxy_new = fx_mappingproxy_new;
    hooks.object_set_item = fx_object_set_item;
    hooks.dict_entry = fx_dict_entry;
    hooks.classify_heap = fx_classify_heap;
    hooks.dict_set = fx_dict_set;
    hooks.dict_resolve = resolve_fixture_dict;
    hooks.dict_get = fx_dict_get;
    hooks.dict_len = fx_dict_len;
    hooks.dict_op = fx_dict_op;
    hooks.foreign_new = fx_foreign_new;
    hooks.dec_ref = fx_dec_ref;
    support::prepare_abi_test_thread(hooks);
}

// Only these explicit probes use the scripted cursor model. Native type and
// exception dictionaries keep the shared runtime's real storage and ownership.
fn is_fixture_dict(bits: u64) -> bool {
    FIXTURE_DICTS
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|dicts| dicts.contains(&bits))
}
fn fake_dict_handle() -> u64 {
    let bits = support::fake_runtime::fresh_handle();
    FIXTURE_DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits);
    bits
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
fn setdefaultref_optional_sink_preserves_status_and_reference_ownership() {
    install();
    PRESENT.lock().unwrap().clear();
    SETS.lock().unwrap().clear();
    let dict = register(fake_dict_handle());
    let key = register(MoltObject::from_int(0x8511).bits());
    let default_value = register(MoltObject::from_int(0x8522).bits());
    let key_bits = handle_of(key);
    let default_bits = handle_of(default_value);
    let dict_bits = handle_of(dict);
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let key_refs = unsafe { (*key).ob_refcnt };
    let default_refs = unsafe { (*default_value).ob_refcnt };

    // CPython 3.13+: NULL means the caller wants only the status. The insert
    // still happens, but no result reference is created for the caller.
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::mapping::PyDict_SetDefaultRef(
                dict,
                key,
                default_value,
                ptr::null_mut(),
            )
        },
        0
    );
    assert_eq!(
        &*SETS.lock().unwrap(),
        &[(dict_bits, key_bits, default_bits)]
    );
    assert_eq!(unsafe { (*key).ob_refcnt }, key_refs);
    assert_eq!(unsafe { (*default_value).ob_refcnt }, default_refs);
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());

    // The fake lookup reports the key itself as its borrowed existing value.
    // That makes a found result distinguishable from the unused default.
    *PRESENT.lock().unwrap() = vec![key_bits];
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::mapping::PyDict_SetDefaultRef(
                dict,
                key,
                default_value,
                ptr::null_mut(),
            )
        },
        1
    );
    assert_eq!(SETS.lock().unwrap().len(), 1, "found must not insert again");
    assert_eq!(unsafe { (*key).ob_refcnt }, key_refs);
    assert_eq!(unsafe { (*default_value).ob_refcnt }, default_refs);

    let mut found = ptr::null_mut();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::mapping::PyDict_SetDefaultRef(
                dict,
                key,
                default_value,
                &raw mut found,
            )
        },
        1
    );
    assert_eq!(found, key);
    assert_eq!(unsafe { (*key).ob_refcnt }, key_refs + 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(found) };

    PRESENT.lock().unwrap().clear();
    let mut inserted = ptr::null_mut();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::mapping::PyDict_SetDefaultRef(
                dict,
                key,
                default_value,
                &raw mut inserted,
            )
        },
        0
    );
    assert_eq!(inserted, default_value);
    assert_eq!(unsafe { (*default_value).ob_refcnt }, default_refs + 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(inserted) };

    let mut on_error = default_value;
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::mapping::PyDict_SetDefaultRef(
                ptr::null_mut(),
                key,
                default_value,
                &raw mut on_error,
            )
        },
        -1
    );
    assert!(
        on_error.is_null(),
        "errors must clear a supplied result sink"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(default_value);
        molt_cpython_abi::api::refcount::Py_DECREF(key);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
    }
}

#[test]
fn next_yields_all_entries_no_exception() {
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

    let dict = register(fake_dict_handle());
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
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(dict) };
}

#[test]
fn merge_populates_target_override() {
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

    let op = register(fake_dict_handle());
    let other = register(fake_dict_handle());
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
    drop(sets);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(op);
        molt_cpython_abi::api::refcount::Py_DECREF(other);
    }
}

#[test]
fn update_overwrites_and_clear_empties() {
    install();
    let key = MoltObject::from_int(0x55).bits();
    let old_value = MoltObject::from_int(0x66).bits();
    let new_value = MoltObject::from_int(0x77).bits();
    *ENTRIES.lock().unwrap() = vec![(key, new_value)];
    *PRESENT.lock().unwrap() = vec![key];
    SETS.lock().unwrap().clear();
    CLEARS.lock().unwrap().clear();
    let target = register(fake_dict_handle());
    let source = register(fake_dict_handle());
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
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(target);
        molt_cpython_abi::api::refcount::Py_DECREF(source);
    }
}

#[test]
fn dict_proxy_is_read_only() {
    install();
    let dict = register(fake_dict_handle());
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
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
            )
        },
        1
    );
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(proxy);
        molt_cpython_abi::api::refcount::Py_DECREF(dict);
        molt_cpython_abi::api::refcount::Py_DECREF(key);
        molt_cpython_abi::api::refcount::Py_DECREF(value);
    }
}

#[test]
fn test_dict_getitem_preserves_entry_error_and_routes_foreign_key() {
    install();
    LOOKUP_KEYS.lock().unwrap().clear();
    PRESENT.lock().unwrap().clear();

    let dict = register(fake_dict_handle());
    let marker = register(MoltObject::from_int(0x5eed).bits());
    let exc_type = (&raw mut molt_cpython_abi::abi_types::PyExc_RuntimeError).cast::<PyObject>();
    molt_cpython_abi::api::errors::restore_current_error_exact(
        molt_cpython_abi::api::errors::OwnedCError {
            exc_type,
            value: marker,
            traceback: ptr::null_mut(),
        },
    );

    let mut foreign_key = Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    });
    let key = &raw mut *foreign_key;
    let result = unsafe { molt_cpython_abi::api::mapping::PyDict_GetItem(dict, key) };
    assert!(
        result.is_null(),
        "the fake dictionary does not contain the key"
    );

    let wrapper = *LOOKUP_KEYS
        .lock()
        .unwrap()
        .as_slice()
        .first()
        .expect("foreign key must reach the canonical dict_get hook");
    assert_ne!(wrapper, 0);
    assert_eq!(
        LOOKUP_KEYS.lock().unwrap().as_slice(),
        &[wrapper],
        "PyDict_GetItem must perform exactly one canonical lookup"
    );
    assert_eq!(
        FOREIGN_C_PTR.load(Ordering::SeqCst),
        0,
        "the temporary foreign runtime wrapper must release its C custody"
    );
    assert_eq!(
        foreign_key.ob_refcnt, 1,
        "the non-stealing lookup must leave the caller's key reference intact"
    );

    let pending = molt_cpython_abi::api::errors::take_current_error()
        .expect("PyDict_GetItem must restore the exact entry error");
    assert_eq!(pending.exc_type, exc_type);
    assert_eq!(pending.value, marker);
    assert!(pending.traceback.is_null());
    drop(pending);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(dict) };
}
