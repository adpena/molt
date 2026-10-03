//! End-to-end mask-proof for the container `tp_richcompare` slots that close the
//! zeroed-shell defect on the built-in container types (ABI-TYPEOBJECT-L4).
//!
//! Before the fix, `PyList_Type`/`PyDict_Type` were `std::mem::zeroed()` shells
//! with `tp_richcompare == NULL`, so `do_richcompare` on two *distinct* but equal
//! list/dict objects fell back to object identity and returned `False` — the same
//! class of bug that broke numpy ufunc dispatch for tuples (`get_info_no_cast`).
//! This drives real ABI code through a fake runtime (own binary, first-wins hook
//! OnceLock): builds two distinct container objects with equal contents and
//! asserts structural equality via `PyObject_RichCompareBool`. Managed operands
//! now route through the canonical runtime comparison hook; the integer-only
//! fixture records that routing, while runtime tests own comparison semantics.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{MoltTypeTag, PyObject};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::os::raw::c_int;
use std::sync::Mutex;

const PY_LT: c_int = 0;
const PY_EQ: c_int = 2;
const PY_NE: c_int = 3;

/// dict store: handle -> insertion-ordered (key_bits, val_bits) entries.
type DictStore = HashMap<u64, Vec<(u64, u64)>>;

static LISTS: Mutex<Option<HashMap<u64, Vec<u64>>>> = Mutex::new(None);
static DICTS: Mutex<Option<DictStore>> = Mutex::new(None);
static COMPARISONS: Mutex<Vec<(i32, u64, u64)>> = Mutex::new(Vec::new());

fn fresh_handle() -> u64 {
    support::fake_runtime::fresh_handle()
}

// This fixture admits only inline integer elements. Runtime tests own Python
// equality/reentrancy semantics; this hook records the ABI's canonical dispatch
// and gives its existing list/dict assertions deterministic fixture results.
unsafe extern "C" fn fx_richcompare(
    op: i32,
    left: u64,
    right: u64,
) -> molt_cpython_abi::hooks::OwnedHandleResult {
    COMPARISONS.lock().unwrap().push((op, left, right));
    let lists = LISTS.lock().unwrap();
    let ordering = lists.as_ref().and_then(|lists| {
        let left = lists.get(&left)?;
        let right = lists.get(&right)?;
        let ints = |items: &[u64]| {
            items
                .iter()
                .map(|bits| {
                    MoltObject::from_bits(*bits)
                        .as_int()
                        .expect("integer-only comparison fixture")
                })
                .collect::<Vec<_>>()
        };
        Some(ints(left).cmp(&ints(right)))
    });
    drop(lists);
    let result = if let Some(ordering) = ordering {
        match op {
            0 => ordering.is_lt(),
            1 => ordering.is_le(),
            2 => ordering.is_eq(),
            3 => !ordering.is_eq(),
            4 => ordering.is_gt(),
            5 => ordering.is_ge(),
            _ => panic!("bad comparison ordinal"),
        }
    } else {
        let dicts = DICTS.lock().unwrap();
        let dicts = dicts.as_ref().unwrap();
        let left = dicts.get(&left).expect("admitted left dictionary");
        let right = dicts.get(&right).expect("admitted right dictionary");
        let equal = left.len() == right.len() && left.iter().all(|entry| right.contains(entry));
        match op {
            PY_EQ => equal,
            PY_NE => !equal,
            _ => panic!("fixture dictionaries support equality only"),
        }
    };
    molt_cpython_abi::hooks::OwnedHandleResult::ok(MoltObject::from_bool(result).bits())
}

// ── fake list runtime ──────────────────────────────────────────────────────
unsafe extern "C" fn fx_alloc_list() -> u64 {
    let bits = fresh_handle();
    LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, Vec::new());
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

// ── fake dict runtime (insertion-ordered assoc list; int keys hash by bits) ──
unsafe extern "C" fn fx_alloc_dict() -> u64 {
    let bits = fresh_handle();
    DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, Vec::new());
    bits
}
unsafe extern "C" fn fx_dict_set(d: u64, k: u64, v: u64) -> i32 {
    if let Some(entries) = DICTS.lock().unwrap().get_or_insert_default().get_mut(&d) {
        if let Some(slot) = entries.iter_mut().find(|(ek, _)| *ek == k) {
            slot.1 = v;
        } else {
            entries.push((k, v));
        }
    }
    0
}
unsafe extern "C" fn resolve_fixture_dict(
    bits: u64,
    _: u8,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    if DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        molt_cpython_abi::hooks::BorrowedHandleResult::ok(bits)
    } else {
        molt_cpython_abi::hooks::BorrowedHandleResult::missing()
    }
}

unsafe extern "C" fn fx_dict_get(
    d: u64,
    k: u64,
    _: molt_cpython_abi::hooks::DictHashSource,
    _: i64,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    match DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&d)
        .and_then(|e| e.iter().find(|(ek, _)| *ek == k).map(|(_, v)| *v))
    {
        Some(value) => molt_cpython_abi::hooks::BorrowedHandleResult::ok(value),
        None => molt_cpython_abi::hooks::BorrowedHandleResult::missing(),
    }
}
unsafe extern "C" fn fx_dict_len(bits: u64) -> usize {
    DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .map_or(0, |e| e.len())
}
unsafe extern "C" fn fx_dict_entry(
    d: u64,
    index: usize,
    out_key: *mut u64,
    out_val: *mut u64,
) -> c_int {
    match DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&d)
        .and_then(|e| e.get(index).copied())
    {
        Some((k, v)) => {
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
    if LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        return MoltTypeTag::List as u8;
    }
    if DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        return MoltTypeTag::Dict as u8;
    }
    unsafe { support::fake_runtime::classify_heap(bits) }
}

fn install() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire(&mut hooks);
    hooks.object_richcompare = fx_richcompare;
    hooks.alloc_list = fx_alloc_list;
    hooks.list_append = fx_list_append;
    hooks.list_len = fx_list_len;
    hooks.list_item = fx_list_item;
    hooks.alloc_dict = fx_alloc_dict;
    hooks.dict_set = fx_dict_set;
    hooks.dict_resolve = resolve_fixture_dict;
    hooks.dict_get = fx_dict_get;
    hooks.dict_len = fx_dict_len;
    hooks.dict_entry = fx_dict_entry;
    hooks.classify_heap = fx_classify_heap;
    support::prepare_abi_test_thread(hooks);
}

/// Mint a `*mut PyObject` for a runtime handle (ob_type set from classify_heap).
fn register(bits: u64) -> *mut PyObject {
    unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}
fn handle_of(object: *mut PyObject) -> u64 {
    molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .molt_handle_for_pyobj(object)
        .expect("fixture managed container")
        .bits()
}
fn int_bits(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}
fn mk_list(items: &[i64]) -> *mut PyObject {
    let lb = unsafe { fx_alloc_list() };
    for &v in items {
        unsafe { fx_list_append(lb, int_bits(v), std::ptr::null_mut()) };
    }
    register(lb)
}
fn mk_dict(pairs: &[(i64, i64)]) -> *mut PyObject {
    let db = unsafe { fx_alloc_dict() };
    for &(k, v) in pairs {
        unsafe { fx_dict_set(db, int_bits(k), int_bits(v)) };
    }
    register(db)
}

#[test]
fn list_structural_richcompare_over_distinct_objects() {
    install();
    COMPARISONS.lock().unwrap().clear();
    use molt_cpython_abi::api::typeobj::PyObject_RichCompareBool;
    unsafe {
        let a = mk_list(&[7, 7, 7]);
        let b = mk_list(&[7, 7, 7]);
        assert!(!a.is_null() && !b.is_null(), "list minting failed");
        assert_ne!(a, b, "must be two distinct list objects");
        // The core proof: distinct-but-equal lists compare equal (pre-fix: NULL
        // slot -> identity fallback -> 0).
        assert_eq!(PyObject_RichCompareBool(a, b, PY_EQ), 1, "[7,7,7]==[7,7,7]");
        let c = mk_list(&[7, 7, 8]);
        assert_eq!(
            PyObject_RichCompareBool(a, c, PY_EQ),
            0,
            "differing lists unequal"
        );
        assert_eq!(
            PyObject_RichCompareBool(a, c, PY_NE),
            1,
            "differing lists != True"
        );
        assert_eq!(
            PyObject_RichCompareBool(a, c, PY_LT),
            1,
            "[7,7,7] < [7,7,8]"
        );
        let short = mk_list(&[7, 7]);
        assert_eq!(
            PyObject_RichCompareBool(a, short, PY_EQ),
            0,
            "different length unequal"
        );
        assert_eq!(
            COMPARISONS.lock().unwrap().clone(),
            vec![
                (PY_EQ, handle_of(a), handle_of(b)),
                (PY_EQ, handle_of(a), handle_of(c)),
                (PY_NE, handle_of(a), handle_of(c)),
                (PY_LT, handle_of(a), handle_of(c)),
                (PY_EQ, handle_of(a), handle_of(short)),
            ],
            "every distinct managed pair reaches the runtime comparison authority"
        );
        for object in [a, b, c, short] {
            molt_cpython_abi::api::refcount::Py_DECREF(object);
        }
    }
}

#[test]
fn dict_structural_richcompare_over_distinct_objects() {
    install();
    COMPARISONS.lock().unwrap().clear();
    use molt_cpython_abi::api::typeobj::PyObject_RichCompareBool;
    unsafe {
        // Same contents, different insertion order AND distinct objects.
        let a = mk_dict(&[(1, 10), (2, 20)]);
        let b = mk_dict(&[(2, 20), (1, 10)]);
        assert!(!a.is_null() && !b.is_null(), "dict minting failed");
        assert_ne!(a, b, "must be two distinct dict objects");
        assert_eq!(
            PyObject_RichCompareBool(a, b, PY_EQ),
            1,
            "equal dicts (order-independent)"
        );
        // Differing value at an equal key.
        let c = mk_dict(&[(1, 10), (2, 99)]);
        assert_eq!(
            PyObject_RichCompareBool(a, c, PY_EQ),
            0,
            "differing value -> unequal"
        );
        assert_eq!(
            PyObject_RichCompareBool(a, c, PY_NE),
            1,
            "differing dicts != True"
        );
        // Different length.
        let d2 = mk_dict(&[(1, 10)]);
        assert_eq!(
            PyObject_RichCompareBool(a, d2, PY_EQ),
            0,
            "different length -> unequal"
        );
        // A key present in `a` but absent in `e` -> unequal (dict_equal key miss).
        let e = mk_dict(&[(1, 10), (3, 20)]);
        assert_eq!(
            PyObject_RichCompareBool(a, e, PY_EQ),
            0,
            "disjoint key -> unequal"
        );
        assert_eq!(
            COMPARISONS.lock().unwrap().clone(),
            vec![
                (PY_EQ, handle_of(a), handle_of(b)),
                (PY_EQ, handle_of(a), handle_of(c)),
                (PY_NE, handle_of(a), handle_of(c)),
                (PY_EQ, handle_of(a), handle_of(d2)),
                (PY_EQ, handle_of(a), handle_of(e)),
            ],
            "every distinct managed pair reaches the runtime comparison authority"
        );
        for object in [a, b, c, d2, e] {
            molt_cpython_abi::api::refcount::Py_DECREF(object);
        }
    }
}
