//! Mask-proof for the builtin VALUE-type `tp_hash` + `tp_richcompare` slots
//! (CLASS1-SLOTS). Before the fix `init_static_types` set `tp_richcompare` on
//! ONLY tuple/list/dict and `tp_hash` on NOTHING, so numpy 2.4.2's `DUAL_INHERIT`
//! (`multiarraymodule.c:4827-4835`) copied NULL `tp_hash`/`tp_richcompare` off
//! molt's `PyFloat_Type`/`PyComplex_Type`/`PyBytes_Type`/`PyUnicode_Type` and its
//! Double/CDouble/String/Unicode scalar types became UNHASHABLE and
//! NON-COMPARABLE, breaking `_multiarray_umath` init.
//!
//! These tests drive the real ABI code through a fake runtime (own binary,
//! first-wins hook `OnceLock`) and assert:
//!  * every builtin value type now carries non-NULL `tp_hash` + `tp_richcompare`;
//!  * a numpy-style DUAL_INHERIT copy off `PyFloat_Type` gets non-NULL slots;
//!  * `PyObject_Hash` returns the correct value (`hash(2)==hash(2.0)==hash(2+0j)==2`);
//!  * equal values compare equal / different unequal / cross-type defers, via
//!    both the slot directly (the numpy-copy call path) and `PyObject_RichCompareBool`.
//!
//! Values verified against CPython 3.12 `Objects/{long,float,complex,unicode,
//! bytes}object.c` (op codes `Py_LT=0 … Py_GE=5`; complex is `==`/`!=`-only;
//! `_PyHASH_IMAG = 1000003`).

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    MoltTypeTag, Py_False, Py_NotImplementedSentinel, Py_True, PyBool_Type, PyBytes_Type,
    PyComplex_Type, PyFloat_Type, PyLong_Type, PyObject, PyTypeObject, PyUnicode_Type,
};
use molt_cpython_abi::api::numbers::PyComplex_FromDoubles;
use molt_cpython_abi::api::typeobj::{PyObject_Hash, PyObject_RichCompareBool};
use molt_lang_obj_model::MoltObject;
use std::collections::HashMap;
use std::os::raw::c_int;
use std::sync::Mutex;

const PY_LT: c_int = 0;
const PY_EQ: c_int = 2;
const PY_NE: c_int = 3;
const PY_GT: c_int = 4;

static TEST_LOCK: Mutex<()> = Mutex::new(());
// str/bytes stores map a fresh handle -> its leaked byte buffer (leaked so the
// `*const u8` the runtime hooks return stays valid for the whole test binary).
static STRS: Mutex<Option<HashMap<u64, &'static [u8]>>> = Mutex::new(None);
static BYTES: Mutex<Option<HashMap<u64, &'static [u8]>>> = Mutex::new(None);
static NEXT_HANDLE: Mutex<u64> = Mutex::new(0xD000);

fn fresh_handle() -> u64 {
    let mut next = NEXT_HANDLE.lock().unwrap();
    let addr = *next as usize;
    *next += 0x100;
    MoltObject::from_ptr(addr as *mut u8).bits()
}

// ── fake str/bytes runtime ─────────────────────────────────────────────────
unsafe extern "C" fn fx_classify_heap(bits: u64) -> u8 {
    if support::fake_complex::contains(bits) {
        return MoltTypeTag::Complex as u8;
    }
    if STRS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        return MoltTypeTag::Str as u8;
    }
    if BYTES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .contains_key(&bits)
    {
        return MoltTypeTag::Bytes as u8;
    }
    MoltTypeTag::Other as u8
}

fn fx_hash_bytes(bytes: &[u8]) -> i64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
    }
    let hash = hash as i64;
    if hash == -1 { -2 } else { hash }
}

unsafe extern "C" fn fx_object_hash(bits: u64) -> i64 {
    if support::fake_complex::contains(bits) {
        return unsafe { support::fake_complex::hash(bits) };
    }
    if let Some(bytes) = STRS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .copied()
    {
        return fx_hash_bytes(bytes);
    }
    if let Some(bytes) = BYTES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .copied()
    {
        return fx_hash_bytes(bytes);
    }
    -1
}
unsafe extern "C" fn fx_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let data = STRS
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .copied();
    match data {
        Some(b) => {
            unsafe {
                if !out_len.is_null() {
                    *out_len = b.len();
                }
            }
            b.as_ptr()
        }
        None => {
            unsafe {
                if !out_len.is_null() {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
    }
}
unsafe extern "C" fn fx_bytes_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let data = BYTES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .get(&bits)
        .copied();
    match data {
        Some(b) => {
            unsafe {
                if !out_len.is_null() {
                    *out_len = b.len();
                }
            }
            b.as_ptr()
        }
        None => {
            unsafe {
                if !out_len.is_null() {
                    *out_len = 0;
                }
            }
            std::ptr::null()
        }
    }
}

fn install() {
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.classify_heap = fx_classify_heap;
    hooks.object_hash = fx_object_hash;
    hooks.complex_from_doubles = support::fake_complex::from_doubles;
    hooks.complex_parts = support::fake_complex::parts;
    hooks.str_data = fx_str_data;
    hooks.bytes_data = fx_bytes_data;
    support::prepare_abi_test_thread(hooks);
}

// ── minting molt-native operands ───────────────────────────────────────────
fn register(bits: u64) -> *mut PyObject {
    unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}
fn mk_int(v: i64) -> *mut PyObject {
    register(MoltObject::from_int(v).bits())
}
fn mk_float(v: f64) -> *mut PyObject {
    register(MoltObject::from_float(v).bits())
}
fn mk_str(s: &str) -> *mut PyObject {
    let leaked: &'static [u8] = Box::leak(s.as_bytes().to_vec().into_boxed_slice());
    let bits = fresh_handle();
    STRS.lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, leaked);
    register(bits)
}
fn mk_bytes(b: &[u8]) -> *mut PyObject {
    let leaked: &'static [u8] = Box::leak(b.to_vec().into_boxed_slice());
    let bits = fresh_handle();
    BYTES
        .lock()
        .unwrap()
        .get_or_insert_default()
        .insert(bits, leaked);
    register(bits)
}

// ── result classifiers ─────────────────────────────────────────────────────
fn is_true(p: *mut PyObject) -> bool {
    p as *const u8 == std::ptr::addr_of!(Py_True) as *const u8
}
fn is_false(p: *mut PyObject) -> bool {
    p as *const u8 == std::ptr::addr_of!(Py_False) as *const u8
}
fn is_not_implemented(p: *mut PyObject) -> bool {
    p as *const u8 == std::ptr::addr_of!(Py_NotImplementedSentinel) as *const u8
}
fn has_hash(ty: *const PyTypeObject) -> bool {
    unsafe { (*ty).tp_hash.is_some() }
}
fn has_cmp(ty: *const PyTypeObject) -> bool {
    unsafe { (*ty).tp_richcompare.is_some() }
}
type RichCmp = unsafe extern "C" fn(*mut PyObject, *mut PyObject, c_int) -> *mut PyObject;
fn cmp_slot(ty: *const PyTypeObject) -> RichCmp {
    unsafe { (*ty).tp_richcompare.expect("tp_richcompare must be wired") }
}
fn clear_err() {
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn every_builtin_value_type_has_hash_and_richcompare_slots() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    // The core zeroed-shell mask-proof: pre-fix these were NULL.
    for ty in [
        std::ptr::addr_of!(PyLong_Type),
        std::ptr::addr_of!(PyBool_Type),
        std::ptr::addr_of!(PyFloat_Type),
        std::ptr::addr_of!(PyComplex_Type),
        std::ptr::addr_of!(PyUnicode_Type),
        std::ptr::addr_of!(PyBytes_Type),
    ] {
        assert!(has_hash(ty), "tp_hash must be non-NULL");
        assert!(has_cmp(ty), "tp_richcompare must be non-NULL");
    }
}

#[test]
fn numpy_dual_inherit_copy_off_float_type_gets_nonnull_slots() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    // Simulate numpy `DUAL_INHERIT(Double, Float, Floating)`:
    //   PyDoubleArrType_Type.tp_hash        = PyFloat_Type.tp_hash;
    //   PyDoubleArrType_Type.tp_richcompare = PyFloat_Type.tp_richcompare;
    let parent = std::ptr::addr_of!(PyFloat_Type);
    let mut scalar: PyTypeObject = unsafe { std::mem::zeroed() };
    scalar.tp_hash = unsafe { (*parent).tp_hash };
    scalar.tp_richcompare = unsafe { (*parent).tp_richcompare };
    assert!(scalar.tp_hash.is_some(), "copied tp_hash must be non-NULL");
    assert!(
        scalar.tp_richcompare.is_some(),
        "copied tp_richcompare must be non-NULL"
    );
    // And the same for the DUAL_INHERIT2 String/Unicode sources.
    for parent in [
        std::ptr::addr_of!(PyBytes_Type),
        std::ptr::addr_of!(PyUnicode_Type),
    ] {
        let mut s: PyTypeObject = unsafe { std::mem::zeroed() };
        s.tp_hash = unsafe { (*parent).tp_hash };
        s.tp_richcompare = unsafe { (*parent).tp_richcompare };
        assert!(s.tp_hash.is_some());
        assert!(s.tp_richcompare.is_some());
    }
}

#[test]
fn hash_values_match_cpython_and_are_cross_type_consistent() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe {
        // hash(2) == 2, hash(2.0) == 2 (integer-valued float), hash(2+0j) == 2.
        // The complex case is the sharp mask-proof: a NULL tp_hash made it raise
        // "unhashable type: 'complex'" and return -1.
        let hi = PyObject_Hash(mk_int(2));
        clear_err();
        let hf = PyObject_Hash(mk_float(2.0));
        clear_err();
        let hc = PyObject_Hash(PyComplex_FromDoubles(2.0, 0.0));
        clear_err();
        assert_eq!(hi, 2, "hash(2) == 2");
        assert_eq!(hf, 2, "hash(2.0) == 2");
        assert_eq!(hc, 2, "hash(2+0j) == 2 (== hash(2.0), imag 0 drops out)");

        // A non-integral / non-zero-imag complex is hashable (no error, not -1).
        let h = PyObject_Hash(PyComplex_FromDoubles(3.0, 4.0));
        assert!(
            molt_cpython_abi::api::errors::PyErr_Occurred().is_null(),
            "no pending error"
        );
        assert_ne!(h, -1, "complex(3,4) is hashable");
        clear_err();

        // str / bytes hashable via PyObject_Hash (non-error).
        let hs = PyObject_Hash(mk_str("abc"));
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        assert_ne!(hs, -1, "'abc' is hashable");
        clear_err();
        let hb = PyObject_Hash(mk_bytes(b"abc"));
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        assert_ne!(hb, -1, "b'abc' is hashable");
        clear_err();
    }
}

#[test]
fn richcompare_slots_invoked_directly_compare_by_value() {
    // Exercises the slot fn pointer directly — the numpy-copy call path, which
    // bypasses do_richcompare's native fast lane.
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe {
        // float slot: 1.5 == 1.5 (distinct objects) True; 1.5 < 2.5 True; vs str NI.
        // (molt-native floats are NaN-boxed values — the bridge returns one
        // canonical handle per value, so equal floats share a pointer; distinctness
        // is exercised below via the freshly-allocated complex/bytes C objects.)
        let ff = cmp_slot(std::ptr::addr_of!(PyFloat_Type));
        let a = mk_float(1.5);
        let b = mk_float(1.5);
        assert!(is_true(ff(a, b, PY_EQ)), "1.5 == 1.5");
        assert!(is_false(ff(a, b, PY_NE)), "not (1.5 != 1.5)");
        assert!(is_true(ff(a, mk_float(2.5), PY_LT)), "1.5 < 2.5");
        assert!(
            is_not_implemented(ff(a, mk_str("x"), PY_EQ)),
            "float vs str -> NotImplemented"
        );
        // float vs int (equal value) compares numerically.
        assert!(is_true(ff(mk_float(1.0), mk_int(1), PY_EQ)), "1.0 == 1");

        // int slot: 3 == 3 True; 3 > 2 True; int vs float -> NotImplemented
        // (long_richcompare CHECK_BINOP requires PyLong_Check(other)).
        let lf = cmp_slot(std::ptr::addr_of!(PyLong_Type));
        assert!(is_true(lf(mk_int(3), mk_int(3), PY_EQ)), "3 == 3");
        assert!(is_true(lf(mk_int(3), mk_int(2), PY_GT)), "3 > 2");
        assert!(
            is_not_implemented(lf(mk_int(3), mk_float(3.0), PY_EQ)),
            "int vs float -> NI"
        );

        // complex slot: only ==/!=; ordering -> NotImplemented.
        let cf = cmp_slot(std::ptr::addr_of!(PyComplex_Type));
        let c1 = PyComplex_FromDoubles(1.0, 2.0);
        let c2 = PyComplex_FromDoubles(1.0, 2.0);
        let c3 = PyComplex_FromDoubles(1.0, 3.0);
        assert!(is_true(cf(c1, c2, PY_EQ)), "(1+2j) == (1+2j)");
        assert!(is_false(cf(c1, c3, PY_EQ)), "(1+2j) != (1+3j)");
        assert!(is_true(cf(c1, c3, PY_NE)), "(1+2j) != (1+3j) is True");
        assert!(
            is_not_implemented(cf(c1, c2, PY_LT)),
            "complex ordering -> NotImplemented"
        );
        // complex vs float: (2+0j) == 2.0 True; (2+1j) == 2.0 False.
        assert!(is_true(cf(
            PyComplex_FromDoubles(2.0, 0.0),
            mk_float(2.0),
            PY_EQ
        )));
        assert!(is_false(cf(
            PyComplex_FromDoubles(2.0, 1.0),
            mk_float(2.0),
            PY_EQ
        )));

        // str slot: "abc" == "abc" True; "abc" < "abd" True; vs bytes NI.
        let sf = cmp_slot(std::ptr::addr_of!(PyUnicode_Type));
        assert!(
            is_true(sf(mk_str("abc"), mk_str("abc"), PY_EQ)),
            "'abc' == 'abc'"
        );
        assert!(
            is_true(sf(mk_str("abc"), mk_str("abd"), PY_LT)),
            "'abc' < 'abd'"
        );
        assert!(
            is_not_implemented(sf(mk_str("abc"), mk_bytes(b"abc"), PY_EQ)),
            "str vs bytes -> NI"
        );

        // bytes slot: b"abc" == b"abc" True; b"abc" < b"abd" True; vs str NI.
        let bf = cmp_slot(std::ptr::addr_of!(PyBytes_Type));
        assert!(
            is_true(bf(mk_bytes(b"abc"), mk_bytes(b"abc"), PY_EQ)),
            "b'abc' == b'abc'"
        );
        assert!(
            is_true(bf(mk_bytes(b"abc"), mk_bytes(b"abd"), PY_LT)),
            "b'abc' < b'abd'"
        );
        assert!(
            is_not_implemented(bf(mk_bytes(b"abc"), mk_str("abc"), PY_EQ)),
            "bytes vs str -> NI"
        );
    }
}

#[test]
fn richcompare_via_public_api_over_distinct_objects() {
    let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install();
    unsafe {
        // bytes: distinct-but-equal compare EQUAL. Pre-fix native_value_richcompare
        // did not handle bytes and the slot was NULL, so this fell to identity -> 0.
        let b1 = mk_bytes(b"abc");
        let b2 = mk_bytes(b"abc");
        assert_ne!(b1, b2, "distinct bytes objects");
        assert_eq!(
            PyObject_RichCompareBool(b1, b2, PY_EQ),
            1,
            "b'abc' == b'abc'"
        );
        assert_eq!(
            PyObject_RichCompareBool(b1, mk_bytes(b"abd"), PY_LT),
            1,
            "b'abc' < b'abd'"
        );
        assert_eq!(
            PyObject_RichCompareBool(b1, mk_bytes(b"abd"), PY_NE),
            1,
            "!= across distinct"
        );

        // complex via the public compare: distinct-but-equal -> EQUAL (mask-proof
        // for the complex slot: NULL slot fell to identity -> 0).
        let c1 = PyComplex_FromDoubles(1.0, 2.0);
        let c2 = PyComplex_FromDoubles(1.0, 2.0);
        assert_ne!(c1, c2, "distinct complex objects");
        assert_eq!(
            PyObject_RichCompareBool(c1, c2, PY_EQ),
            1,
            "(1+2j) == (1+2j)"
        );
        assert_eq!(
            PyObject_RichCompareBool(c1, PyComplex_FromDoubles(9.0, 9.0), PY_EQ),
            0
        );

        // float / str end-to-end.
        assert_eq!(
            PyObject_RichCompareBool(mk_float(1.0), mk_int(1), PY_EQ),
            1,
            "1.0 == 1"
        );
        assert_eq!(
            PyObject_RichCompareBool(mk_str("abc"), mk_str("abc"), PY_EQ),
            1
        );
        assert_eq!(
            PyObject_RichCompareBool(mk_str("abc"), mk_str("abc"), PY_NE),
            0
        );
    }
}
