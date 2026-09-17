//! Mask-proof teeth for the real indexed `PyList_SetItem` store (ledger H row
//! `sequences.rs:133`) + `PyList_New` presizing.
//!
//! Backed by a Vec-of-Vec fake list model so the assertions observe PLACEMENT,
//! not just return codes: before the fix SetItem APPENDED (mis-placing any
//! out-of-order fill, duplicating on replace), silently DROPPED foreign items
//! while returning success, and never bounds-checked.
//!
//! Refcount contract locked here (CPython Objects/listobject.c):
//! * success STEALS the reference to v (refcount unchanged — no Append-style
//!   INCREF);
//! * OOB releases the stolen reference (Py_XDECREF) and sets IndexError;
//! * a foreign item gets TYPE_ID_FOREIGN custody: the wrapper takes ONE strong
//!   C reference and the stolen caller reference is consumed — net refcount
//!   unchanged, item retrievable through GetItem as the SAME C pointer.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{MoltTypeTag, PyList_Type, PyListObject, PyObject};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::ptr;
use std::sync::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());
// handle bits → the list's item vector.
static LISTS: Mutex<Option<HashMap<u64, Vec<u64>>>> = Mutex::new(None);
static TUPLES: Mutex<Option<HashMap<u64, Vec<u64>>>> = Mutex::new(None);
static NEXT_LIST: Mutex<u64> = Mutex::new(0x9000);
static NEXT_TUPLE: Mutex<u64> = Mutex::new(0x19_0000);
static NEXT_FOREIGN: Mutex<u64> = Mutex::new(0xF0DE_0000_0000_0010);

unsafe extern "C" fn fx_alloc_list() -> u64 {
    let mut next = NEXT_LIST.lock().unwrap();
    let addr = *next as usize;
    *next += 0x100;
    let bits = MoltObject::from_ptr(addr as *mut u8).bits();
    LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, Vec::new());
    bits
}
unsafe extern "C" fn fx_alloc_list_presized(len: usize) -> u64 {
    let mut next = NEXT_LIST.lock().unwrap();
    let addr = *next as usize;
    *next += 0x100;
    let bits = MoltObject::from_ptr(addr as *mut u8).bits();
    LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, vec![MoltObject::none().bits(); len]);
    bits
}
unsafe extern "C" fn fx_list_append(list_bits: u64, item_bits: u64, _item: *mut PyObject) -> i32 {
    if let Some(v) = LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get_mut(&list_bits)
    {
        v.push(item_bits);
    }
    0
}
unsafe extern "C" fn fx_list_len(bits: u64) -> usize {
    LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .map_or(0, |v| v.len())
}
unsafe extern "C" fn fx_list_item(
    bits: u64,
    i: usize,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    match LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .and_then(|v| v.get(i).copied())
    {
        Some(value) => molt_cpython_abi::hooks::BorrowedHandleResult::ok(value),
        None => molt_cpython_abi::hooks::BorrowedHandleResult::missing(),
    }
}
unsafe extern "C" fn fx_list_set(
    list_bits: u64,
    i: usize,
    val_bits: u64,
) -> molt_cpython_abi::hooks::OwnedHandleResult {
    let mut lists = LISTS.lock().unwrap();
    match lists.get_or_insert_default().get_mut(&list_bits) {
        Some(v) if i < v.len() => {
            let old = std::mem::replace(&mut v[i], val_bits);
            molt_cpython_abi::hooks::OwnedHandleResult::ok(old)
        }
        _ => molt_cpython_abi::hooks::OwnedHandleResult::error(),
    }
}
unsafe extern "C" fn fx_list_insert(
    list_bits: u64,
    where_: isize,
    item_bits: u64,
    _item: *mut PyObject,
) -> i32 {
    let mut lists = LISTS.lock().unwrap();
    let Some(list) = lists.get_or_insert_default().get_mut(&list_bits) else {
        return -1;
    };
    let index = if where_ < 0 {
        (where_ + list.len() as isize).max(0) as usize
    } else {
        (where_ as usize).min(list.len())
    };
    list.insert(index, item_bits);
    0
}
unsafe extern "C" fn fx_alloc_tuple(len: usize) -> u64 {
    let mut next = NEXT_TUPLE.lock().unwrap();
    let addr = *next as usize;
    *next += 0x100;
    let bits = MoltObject::from_ptr(addr as *mut u8).bits();
    TUPLES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, vec![MoltObject::none().bits(); len]);
    bits
}
unsafe extern "C" fn fx_tuple_set(
    tuple_bits: u64,
    i: usize,
    val_bits: u64,
    _item: *mut PyObject,
) -> molt_cpython_abi::hooks::OwnedHandleResult {
    let mut tuples = TUPLES.lock().unwrap();
    match tuples.get_or_insert_default().get_mut(&tuple_bits) {
        Some(tuple) if i < tuple.len() => {
            let old = std::mem::replace(&mut tuple[i], val_bits);
            molt_cpython_abi::hooks::OwnedHandleResult::ok(old)
        }
        _ => molt_cpython_abi::hooks::OwnedHandleResult::error(),
    }
}
unsafe extern "C" fn fx_tuple_len(bits: u64) -> usize {
    TUPLES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .map_or(0, Vec::len)
}
unsafe extern "C" fn fx_tuple_item(
    bits: u64,
    i: usize,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    match TUPLES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .and_then(|tuple| tuple.get(i).copied())
    {
        Some(value) => molt_cpython_abi::hooks::BorrowedHandleResult::ok(value),
        None => molt_cpython_abi::hooks::BorrowedHandleResult::missing(),
    }
}
unsafe extern "C" fn fx_ref_count(_bits: u64) -> usize {
    1
}
unsafe extern "C" fn fx_classify_heap(bits: u64) -> u8 {
    if LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        MoltTypeTag::List as u8
    } else if TUPLES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        MoltTypeTag::Tuple as u8
    } else {
        MoltTypeTag::Other as u8
    }
}
unsafe extern "C" fn fx_int_from_i64(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}
unsafe extern "C" fn fx_foreign_new(_c_ptr: usize) -> u64 {
    let mut next = NEXT_FOREIGN.lock().unwrap();
    let w = *next;
    *next += 0x10;
    w
}

fn install() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_list = fx_alloc_list;
    hooks.alloc_list_presized = fx_alloc_list_presized;
    hooks.list_append = fx_list_append;
    hooks.list_len = fx_list_len;
    hooks.list_item = fx_list_item;
    hooks.list_set = fx_list_set;
    hooks.list_insert = fx_list_insert;
    hooks.alloc_tuple = fx_alloc_tuple;
    hooks.tuple_set = fx_tuple_set;
    hooks.tuple_len = fx_tuple_len;
    hooks.tuple_item = fx_tuple_item;
    hooks.ref_count = fx_ref_count;
    hooks.classify_heap = fx_classify_heap;
    hooks.int_from_i64 = fx_int_from_i64;
    hooks.foreign_new = fx_foreign_new;
    support::prepare_abi_test_thread(hooks);
}

use molt_cpython_abi::api::{errors, numbers, sequences};

#[test]
fn cython_direct_ob_item_construction_commits_one_truthful_list() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let list = unsafe { sequences::PyList_New(3) };
    assert!(!list.is_null());
    let physical = list.cast::<PyListObject>();
    unsafe {
        assert_eq!((*list).ob_type, &raw mut PyList_Type);
        assert_eq!((*physical).ob_base.ob_size, 3);
        assert!((*physical).allocated >= 3);
        assert!(!(*physical).ob_item.is_null());
        for index in 0..3 {
            assert!(
                (*(*physical).ob_item.add(index)).is_null(),
                "PyList_New must publish NULL construction slots"
            );
        }
    }

    let source = unsafe {
        [
            numbers::PyLong_FromLong(101),
            numbers::PyLong_FromLong(202),
            numbers::PyLong_FromLong(303),
        ]
    };
    let source_bits = source.map(|pointer| {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(pointer)
            .expect("numeric carrier must retain its runtime identity")
            .bits()
    });

    // This is the unavoidable Cython `__Pyx_copy_object_array` operation:
    // each source element is first INCREF'd and then copied directly into the
    // `PyListObject.ob_item` array, bypassing every public list setter.
    unsafe {
        for (index, pointer) in source.into_iter().enumerate() {
            molt_cpython_abi::api::refcount::Py_INCREF(pointer);
            *(*physical).ob_item.add(index) = pointer;
        }
    }

    // The first checked C observation commits the complete pointer snapshot
    // into the runtime list, consumes the three stolen construction refs, and
    // republishes one owned projection of that canonical runtime state.
    for (index, expected_bits) in source_bits.into_iter().enumerate() {
        let item = unsafe { sequences::PyList_GetItem(list, index as isize) };
        assert!(
            !item.is_null(),
            "direct slot {index} must become observable"
        );
        let actual_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(item)
            .expect("committed item must retain runtime identity")
            .bits();
        assert_eq!(actual_bits, expected_bits, "direct slot {index}");
    }
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());

    // A completed list remains physically writable through the non-limited
    // PyListObject ABI. `sealed` describes completeness, not cleanliness: the
    // next semantic observation must detect and commit this direct replacement.
    let replacement = unsafe { numbers::PyLong_FromLong(909) };
    let replacement_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .molt_handle_for_pyobj(replacement)
        .expect("replacement must retain runtime identity")
        .bits();
    unsafe {
        molt_cpython_abi::api::refcount::Py_INCREF(replacement);
        *(*physical).ob_item.add(1) = replacement;
    }
    let observed = unsafe { sequences::PyList_GetItem(list, 1) };
    assert_eq!(
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(observed)
            .expect("direct replacement must commit")
            .bits(),
        replacement_bits
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn setitem_places_items_at_index_out_of_order() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    // PyList_New(3) must PRESIZE: GET_SIZE == 3 immediately (ledger row
    // sequences.rs:14 — the old body ignored size).
    let list = unsafe { sequences::PyList_New(3) };
    assert!(!list.is_null());
    assert_eq!(
        unsafe { sequences::PyList_Size(list) },
        3,
        "PyList_New(3) must pre-size to 3 slots (CPython Py_SET_SIZE)"
    );

    // Out-of-order fill — the pattern the old append-based SET_ITEM mis-placed.
    let a = unsafe { numbers::PyLong_FromLong(111) };
    let b = unsafe { numbers::PyLong_FromLong(222) };
    let c = unsafe { numbers::PyLong_FromLong(333) };
    let bridge_bits = |p: *mut PyObject| {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(p)
            .map(|value| value.bits())
            .unwrap()
    };
    // Capture the values before SetItem steals and releases the three physical
    // carriers. The list retains the runtime handles, not those carrier
    // addresses, and GetItem may materialize different borrowed carriers.
    let (ab, bb, cb) = (bridge_bits(a), bridge_bits(b), bridge_bits(c));
    assert_eq!(unsafe { sequences::PyList_SetItem(list, 2, c) }, 0);
    assert_eq!(unsafe { sequences::PyList_SetItem(list, 0, a) }, 0);
    assert_eq!(unsafe { sequences::PyList_SetItem(list, 1, b) }, 0);
    assert_eq!(
        unsafe { sequences::PyList_Size(list) },
        3,
        "indexed store must REPLACE, never append-grow"
    );
    unsafe {
        assert_eq!(
            bridge_bits(sequences::PyList_GetItem(list, 0)),
            ab,
            "slot 0"
        );
        assert_eq!(
            bridge_bits(sequences::PyList_GetItem(list, 1)),
            bb,
            "slot 1"
        );
        assert_eq!(
            bridge_bits(sequences::PyList_GetItem(list, 2)),
            cb,
            "slot 2"
        );
    }
    assert!(
        unsafe { errors::PyErr_Occurred() }.is_null(),
        "successful fills must not leave an exception"
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn setitem_accepts_a_self_cycle_during_presized_construction() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let list = unsafe { sequences::PyList_New(1) };
    assert!(!list.is_null());
    // Preserve the caller's reference because PyList_SetItem steals one.
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(list) };
    assert_eq!(unsafe { sequences::PyList_SetItem(list, 0, list) }, 0);
    assert_eq!(unsafe { sequences::PyList_GetItem(list, 0) }, list);
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());

    // Dissolve the list -> self edge before releasing the caller's retained
    // reference; the indexed replacement consumes its new reference.
    let replacement = unsafe { numbers::PyLong_FromLong(0) };
    assert_eq!(
        unsafe { sequences::PyList_SetItem(list, 0, replacement) },
        0
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn setitem_accepts_mutual_list_tuple_construction_edges() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let list = unsafe { sequences::PyList_New(1) };
    let tuple = unsafe { sequences::PyTuple_New(1) };
    assert!(!list.is_null() && !tuple.is_null());

    // Both containers are still incomplete. Reference insertion transports
    // their canonical identities; it does not semantically observe either
    // container's not-yet-readable contents.
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(list) };
    assert_eq!(unsafe { sequences::PyTuple_SetItem(tuple, 0, list) }, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(tuple) };
    assert_eq!(unsafe { sequences::PyList_SetItem(list, 0, tuple) }, 0);

    assert_eq!(unsafe { sequences::PyList_GetItem(list, 0) }, tuple);
    assert_eq!(unsafe { sequences::PyTuple_GetItem(tuple, 0) }, list);
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());

    // Break the list edge first so the exact tuple becomes uniquely owned and
    // may legally replace its construction slot, then retire both callers.
    assert_eq!(
        unsafe { sequences::PyList_SetItem(list, 0, numbers::PyLong_FromLong(1)) },
        0
    );
    assert_eq!(
        unsafe { sequences::PyTuple_SetItem(tuple, 0, numbers::PyLong_FromLong(2)) },
        0
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(tuple);
        molt_cpython_abi::api::refcount::Py_DECREF(list);
    }
}

#[test]
fn append_and_insert_accept_incomplete_list_reference_edges() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let appended = unsafe { sequences::PyList_New(1) };
    let append_target = unsafe { sequences::PyList_New(0) };
    let inserted = unsafe { sequences::PyList_New(1) };
    let insert_target = unsafe { sequences::PyList_New(0) };
    assert!(!appended.is_null() && !append_target.is_null());
    assert!(!inserted.is_null() && !insert_target.is_null());

    assert_eq!(
        unsafe { sequences::PyList_Append(append_target, appended) },
        0
    );
    assert_eq!(
        unsafe { sequences::PyList_Insert(insert_target, 0, inserted) },
        0
    );
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
    let bits = |object| {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(object)
            .expect("managed construction object must retain identity")
            .bits()
    };
    let append_target_bits = bits(append_target);
    let appended_bits = bits(appended);
    let insert_target_bits = bits(insert_target);
    let inserted_bits = bits(inserted);
    {
        let lists = LISTS.lock().unwrap();
        let lists = lists.as_ref().expect("fake list storage");
        assert_eq!(
            lists.get(&append_target_bits).map(Vec::as_slice),
            Some(&[appended_bits][..])
        );
        assert_eq!(
            lists.get(&insert_target_bits).map(Vec::as_slice),
            Some(&[inserted_bits][..])
        );
    }

    // Retaining the edge does not make an incomplete value semantically
    // readable. The ordinary observation boundary must still reject it.
    assert!(unsafe { sequences::PyList_GetItem(appended, 0) }.is_null());
    assert!(!unsafe { errors::PyErr_Occurred() }.is_null());
    unsafe { errors::PyErr_Clear() };

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(append_target);
        molt_cpython_abi::api::refcount::Py_DECREF(appended);
        molt_cpython_abi::api::refcount::Py_DECREF(insert_target);
        molt_cpython_abi::api::refcount::Py_DECREF(inserted);
    }
}

#[test]
fn setitem_oob_sets_indexerror_and_releases_stolen_ref() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let list = unsafe { sequences::PyList_New(1) };
    // Use a mortal, non-small integer. Small-int carriers are immortal by
    // design and therefore cannot prove that the stolen reference is released.
    let item = unsafe { numbers::PyLong_FromLong(1000) };
    // Proxy starts at refcount 1 (the caller's reference, which SetItem steals).
    assert_eq!(unsafe { (*item).ob_refcnt }, 1);

    let rc = unsafe { sequences::PyList_SetItem(list, 5, item) };
    assert_eq!(rc, -1, "OOB index must fail with -1, not report success");
    assert!(
        !unsafe { errors::PyErr_Occurred() }.is_null(),
        "OOB must set IndexError (list assignment index out of range)"
    );
    // Steal contract on failure: CPython Py_XDECREFs newitem — refcount 1 → 0,
    // which releases the bridge proxy, severing the pointer↔handle mapping.
    assert!(
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .pyobj_to_handle(item)
            .is_none(),
        "the stolen reference must be released on the OOB error path"
    );
    unsafe { errors::PyErr_Clear() };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(list) };
}

#[test]
fn setitem_foreign_item_gets_custody_and_is_retrievable() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let list = unsafe { sequences::PyList_New(1) };

    // A genuine C-extension object the bridge has never seen (numpy scalar
    // stand-in). Refcount 1 = the caller's reference (stolen by SetItem).
    let mut foreign = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    let fp = &raw mut foreign;

    // Before the fix: pyobj_to_handle(None) arm returned WITHOUT storing while
    // SetItem reported 0 — the silent foreign drop this locks out.
    let rc = unsafe { sequences::PyList_SetItem(list, 0, fp) };
    assert_eq!(rc, 0, "foreign item must be accepted, not dropped");

    // Retrievable: GetItem resolves the TYPE_ID_FOREIGN wrapper back to the
    // SAME C pointer (raw_py round-trip identity).
    let got = unsafe { sequences::PyList_GetItem(list, 0) };
    assert_eq!(
        got, fp,
        "stored foreign item must be retrievable as the same C pointer"
    );

    // Refcount contract: wrapper mint takes ONE strong C reference (+1) and the
    // stolen caller reference is consumed (-1) — net unchanged at 1, custody
    // now owned by the wrapper.
    assert_eq!(
        unsafe { (*fp).ob_refcnt },
        2,
        "foreign custody must represent runtime and physical projection edges"
    );
    assert!(
        unsafe { errors::PyErr_Occurred() }.is_null(),
        "the foreign path must not leave a stray exception"
    );
    // Release the list before detaching the wrapper identity so its projection
    // edge is retired while the stack-backed foreign object is still valid.
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(list);
        molt_cpython_abi::bridge::GLOBAL_BRIDGE.release_foreign(fp as usize);
    }
}
