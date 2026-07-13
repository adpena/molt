//! Mask-proof teeth for the abstract sequence/mapping protocol fixes
//! (`abstract_sequence.rs` / `abstract_mapping.rs` ledger rows).
//!
//! Fake-model harness (own binary: the hook OnceLock is first-wins per
//! process). Locks the divergences the ledger flagged:
//! * Contains/Index compared raw handle BITS — equal-but-distinct heap strings
//!   always missed (silent wrong answer);
//! * Index returned a bare `-1` on absence (indistinguishable from an error);
//! * Tuple/List fabricated EMPTY results for non-iterables (theater);
//! * PySequence_SetItem silently MUTATED tuples (CPython: TypeError);
//! * PySequence_Size(dict) returned a silent -1 (CPython: TypeError, dict has
//!   no sq_length);
//! * PyMapping_Check answered 0 for native lists (CPython: any mp_subscript).

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{MoltTypeTag, PyObject};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::ptr;
use std::sync::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());
static LISTS: Mutex<Option<HashMap<u64, Vec<u64>>>> = Mutex::new(None);
static STRS: Mutex<Option<HashMap<u64, &'static [u8]>>> = Mutex::new(None);
static TUPLES: Mutex<Option<HashMap<u64, Vec<u64>>>> = Mutex::new(None);
static DICTS: Mutex<Option<HashMap<u64, usize>>> = Mutex::new(None);
static NEXT_HANDLE: Mutex<u64> = Mutex::new(0xA000);

fn fresh_handle() -> u64 {
    let mut next = NEXT_HANDLE.lock().unwrap();
    let addr = *next as usize;
    *next += 0x100;
    MoltObject::from_ptr(addr as *mut u8).bits()
}

unsafe extern "C" fn fx_alloc_list() -> u64 {
    let bits = fresh_handle();
    LISTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, Vec::new());
    bits
}
unsafe extern "C" fn fx_list_append(list_bits: u64, item_bits: u64) -> i32 {
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
unsafe extern "C" fn fx_alloc_tuple(n: usize) -> u64 {
    let bits = fresh_handle();
    TUPLES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, vec![MoltObject::none().bits(); n]);
    bits
}
unsafe extern "C" fn fx_tuple_set(
    bits: u64,
    i: usize,
    val: u64,
) -> molt_cpython_abi::hooks::OwnedHandleResult {
    if let Some(v) = TUPLES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get_mut(&bits)
        && i < v.len()
    {
        let old = std::mem::replace(&mut v[i], val);
        return molt_cpython_abi::hooks::OwnedHandleResult::ok(old);
    }
    molt_cpython_abi::hooks::OwnedHandleResult::error()
}
unsafe extern "C" fn fx_tuple_len(bits: u64) -> usize {
    TUPLES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .map_or(0, |v| v.len())
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
        .and_then(|v| v.get(i).copied())
    {
        Some(value) => molt_cpython_abi::hooks::BorrowedHandleResult::ok(value),
        None => molt_cpython_abi::hooks::BorrowedHandleResult::missing(),
    }
}
unsafe extern "C" fn fx_alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes: &'static [u8] = Box::leak(
        unsafe { std::slice::from_raw_parts(data, len) }
            .to_vec()
            .into_boxed_slice(),
    );
    let bits = fresh_handle();
    STRS.lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, bytes);
    bits
}
unsafe extern "C" fn fx_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    match STRS.lock().unwrap().get_or_insert_default().get(&bits) {
        Some(s) => {
            if !out_len.is_null() {
                unsafe { *out_len = s.len() };
            }
            s.as_ptr()
        }
        None => {
            if !out_len.is_null() {
                unsafe { *out_len = 0 };
            }
            ptr::null()
        }
    }
}
unsafe extern "C" fn fx_dict_len(bits: u64) -> usize {
    DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .copied()
        .unwrap_or(0)
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
    if TUPLES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        return MoltTypeTag::Tuple as u8;
    }
    if STRS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        return MoltTypeTag::Str as u8;
    }
    if DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        return MoltTypeTag::Dict as u8;
    }
    MoltTypeTag::Other as u8
}

fn install() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_list = fx_alloc_list;
    hooks.list_append = fx_list_append;
    hooks.list_len = fx_list_len;
    hooks.list_item = fx_list_item;
    hooks.alloc_tuple = fx_alloc_tuple;
    hooks.tuple_set = fx_tuple_set;
    hooks.tuple_len = fx_tuple_len;
    hooks.tuple_item = fx_tuple_item;
    hooks.alloc_str = fx_alloc_str;
    hooks.str_data = fx_str_data;
    hooks.dict_len = fx_dict_len;
    hooks.classify_heap = fx_classify_heap;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

fn register(bits: u64) -> *mut PyObject {
    unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}
fn make_str(text: &str) -> u64 {
    unsafe { fx_alloc_str(text.as_ptr(), text.len()) }
}

use molt_cpython_abi::api::{abstract_mapping, abstract_sequence, errors};

#[test]
fn contains_and_index_use_value_equality_for_heap_strings() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    // Two DISTINCT heap str handles with EQUAL bytes — the exact case the
    // pre-fix raw-bits compare missed.
    let s_in_list = make_str("dtype");
    let s_probe = make_str("dtype");
    assert_ne!(
        s_in_list, s_probe,
        "handles must be distinct for this proof"
    );

    let list_bits = unsafe { fx_alloc_list() };
    unsafe { fx_list_append(list_bits, s_in_list) };
    let list = register(list_bits);
    let probe = register(s_probe);

    assert_eq!(
        unsafe { abstract_sequence::PySequence_Contains(list, probe) },
        1,
        "equal-but-distinct heap strings must match by VALUE (Py_EQ), not bits"
    );
    assert_eq!(
        unsafe { abstract_sequence::PySequence_Index(list, probe) },
        0,
        "PySequence_Index must find the value-equal string at index 0"
    );
    assert_eq!(
        unsafe { abstract_sequence::PySequence_Count(list, probe) },
        1,
        "PySequence_Count must count the value-equal string"
    );
}

#[test]
fn index_absent_raises_valueerror() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let list_bits = unsafe { fx_alloc_list() };
    unsafe { fx_list_append(list_bits, make_str("present")) };
    let list = register(list_bits);
    let probe = register(make_str("absent"));

    let rc = unsafe { abstract_sequence::PySequence_Index(list, probe) };
    assert_eq!(rc, -1);
    assert!(
        !unsafe { errors::PyErr_Occurred() }.is_null(),
        "CPython raises ValueError 'sequence.index(x): x not in sequence' — a \
         bare -1 is the pre-fix silent sentinel"
    );
    unsafe { errors::PyErr_Clear() };
}

#[test]
fn tuple_and_list_of_non_iterable_raise_typeerror_not_empty() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    // An int is not iterable: CPython raises TypeError. The pre-fix theater
    // fabricated an EMPTY tuple / empty list with no exception.
    let n = register(MoltObject::from_int(42).bits());

    let t = unsafe { abstract_sequence::PySequence_Tuple(n) };
    assert!(
        t.is_null(),
        "PySequence_Tuple(non-iterable) must be NULL, not an empty tuple"
    );
    assert!(
        !unsafe { errors::PyErr_Occurred() }.is_null(),
        "TypeError must be pending"
    );
    unsafe { errors::PyErr_Clear() };

    let l = unsafe { abstract_sequence::PySequence_List(n) };
    assert!(
        l.is_null(),
        "PySequence_List(non-iterable) must be NULL, not an empty list"
    );
    assert!(
        !unsafe { errors::PyErr_Occurred() }.is_null(),
        "TypeError must be pending"
    );
    unsafe { errors::PyErr_Clear() };
}

#[test]
fn str_materializes_into_code_point_tuple() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    // tuple('ab') == ('a', 'b') — CPython drains the str iterator; the
    // pre-fix code raised/fabricated for every non-list/tuple.
    let s = register(make_str("ab"));
    let t = unsafe { abstract_sequence::PySequence_Tuple(s) };
    assert!(
        !t.is_null(),
        "PySequence_Tuple(str) must materialize the code points"
    );
    assert_eq!(unsafe { abstract_sequence::PySequence_Fast_GET_SIZE(t) }, 2);
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

#[test]
fn sequence_setitem_on_tuple_raises_typeerror() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let tuple_bits = unsafe { fx_alloc_tuple(1) };
    let tuple = register(tuple_bits);
    let v = register(make_str("x"));

    // CPython: tuple has no sq_ass_item → TypeError. The pre-fix code
    // DELEGATED TO PyTuple_SetItem and silently mutated the tuple.
    let rc = unsafe { abstract_sequence::PySequence_SetItem(tuple, 0, v) };
    assert_eq!(rc, -1, "immutable tuple must reject item assignment");
    assert!(
        !unsafe { errors::PyErr_Occurred() }.is_null(),
        "TypeError 'object does not support item assignment' must be pending"
    );
    unsafe { errors::PyErr_Clear() };
    // And the tuple's slot must be untouched (still the None placeholder).
    assert!(
        matches!(
            unsafe { fx_tuple_item(tuple_bits, 0) }.decode(),
            molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits)
                if bits == MoltObject::none().bits()
        ),
        "the pre-fix silent tuple mutation must be locked out"
    );
}

#[test]
fn sequence_size_of_dict_raises_typeerror() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe { errors::PyErr_Clear() };

    let dict_bits = fresh_handle();
    DICTS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(dict_bits, 3);
    let dict = register(dict_bits);

    // CPython: dict has mp_length but NO sq_length → TypeError "%s is not a
    // sequence" (never the length, never a silent -1).
    let rc = unsafe { abstract_sequence::PySequence_Size(dict) };
    assert_eq!(rc, -1);
    assert!(!unsafe { errors::PyErr_Occurred() }.is_null());
    unsafe { errors::PyErr_Clear() };

    // The MAPPING protocol is where dict length lives.
    assert_eq!(unsafe { abstract_mapping::PyMapping_Size(dict) }, 3);
    assert!(unsafe { errors::PyErr_Occurred() }.is_null());
}

#[test]
fn mapping_check_accepts_subscriptable_natives() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();

    // CPython PyMapping_Check(list) == 1 (list has mp_subscript). The pre-fix
    // body answered 1 only for dicts.
    let list = register(unsafe { fx_alloc_list() });
    assert_eq!(unsafe { abstract_mapping::PyMapping_Check(list) }, 1);
    let s = register(make_str("m"));
    assert_eq!(unsafe { abstract_mapping::PyMapping_Check(s) }, 1);
}
