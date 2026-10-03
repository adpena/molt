//! F1 mask-proof gates for `PySlice_Unpack` (`slice.rs:117`, CONFIRMED High)
//! and the legacy `PySlice_GetIndices` (`slice.rs:209`, Low).
//!
//! CPython 3.12 primary sources: Objects/sliceobject.c (PySlice_Unpack,
//! PySlice_GetIndices) + Python/ceval.c (_PyEval_SliceIndex) +
//! Objects/abstract.c (PyNumber_AsSsize_t err==NULL clamp).
//!
//! Divergences under test:
//!   * a non-index bound (`slice(1.5)`, `slice('a')`) silently produced a -1
//!     bound and SUCCESS — must be TypeError "slice indices must be integers
//!     or None or have an __index__ method" with -1;
//!   * `slice(0, 10**30)` truncated — must CLAMP to PY_SSIZE_T_MAX (and a
//!     negative overflow to PY_SSIZE_T_MIN), reporting success;
//!   * legacy `PySlice_GetIndices` clamped out-of-range indices it must
//!     reject with -1 (`*stop > length` / `*start >= length`).
//!
//! The mock hooks model the unsigned-64 band and a beyond-±2^64 bignum whose
//! sign resolves through the runtime's direct integer-sign authority.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{Py_None, PyObject, PySliceObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

static BIG_U64_BITS: AtomicU64 = AtomicU64::new(0); // u64::MAX - 3 (Big band)
static HUGE_NEG_BITS: AtomicU64 = AtomicU64::new(0); // < -2^64
static TEST_LOCK: Mutex<()> = Mutex::new(());
static CLASS_ANCHORS: [u64; 5] = [0; 5];
fn class_bits(index: usize) -> u64 {
    MoltObject::from_ptr((&raw const CLASS_ANCHORS[index]).cast_mut().cast()).bits()
}
unsafe extern "C" fn mock_runtime_class(
    bits: u64,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    let value = MoltObject::from_bits(bits);
    let index = if support::fake_strings::contains(bits) {
        1
    } else if value.is_bool() {
        4
    } else if value.is_int()
        || bits == BIG_U64_BITS.load(Ordering::SeqCst)
        || bits == HUGE_NEG_BITS.load(Ordering::SeqCst)
    {
        2
    } else if value.is_float() {
        3
    } else {
        0
    };
    molt_cpython_abi::hooks::BorrowedHandleResult::ok(class_bits(index))
}
// These normalization fixtures define no custom type slots. Real descriptor
// and callback admission is tested with the production runtime provider.
unsafe extern "C" fn mock_type_lookup(
    _class: u64,
    _name: u64,
    _mro: u8,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    molt_cpython_abi::hooks::BorrowedHandleResult::missing()
}

fn test_guard() -> MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

const BIG_U64_VALUE: u64 = u64::MAX - 3;

unsafe extern "C" fn mock_classify_heap(bits: u64) -> u8 {
    if (0..5).any(|index| bits == class_bits(index)) {
        molt_cpython_abi::abi_types::MoltTypeTag::Type as u8
    } else if support::fake_strings::contains(bits) {
        molt_cpython_abi::abi_types::MoltTypeTag::Str as u8
    } else if bits == BIG_U64_BITS.load(Ordering::SeqCst)
        || bits == HUGE_NEG_BITS.load(Ordering::SeqCst)
    {
        molt_cpython_abi::abi_types::MoltTypeTag::Int as u8
    } else {
        molt_cpython_abi::abi_types::MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn mock_int_as_i64_checked(_bits: u64, _out: *mut i64) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn mock_int_as_u64_checked(bits: u64, out: *mut u64) -> std::os::raw::c_int {
    if bits == BIG_U64_BITS.load(Ordering::SeqCst) {
        unsafe { *out = BIG_U64_VALUE };
        0
    } else {
        -1
    }
}

unsafe extern "C" fn mock_int_sign(bits: u64) -> i32 {
    if bits == HUGE_NEG_BITS.load(Ordering::SeqCst) {
        -1
    } else if bits == BIG_U64_BITS.load(Ordering::SeqCst) {
        1
    } else {
        2
    }
}

fn install_hooks() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    if BIG_U64_BITS.load(Ordering::SeqCst) == 0 {
        let a: *mut u8 = Box::into_raw(Box::new(0u8));
        let b: *mut u8 = Box::into_raw(Box::new(0u8));
        BIG_U64_BITS.store(MoltObject::from_ptr(a).bits(), Ordering::SeqCst);
        HUGE_NEG_BITS.store(MoltObject::from_ptr(b).bits(), Ordering::SeqCst);
    }
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.classify_heap = mock_classify_heap;
    hooks.int_as_i64_checked = mock_int_as_i64_checked;
    hooks.int_as_u64_checked = mock_int_as_u64_checked;
    hooks.int_sign = mock_int_sign;
    hooks.runtime_class_borrowed = mock_runtime_class;
    hooks.type_lookup_borrowed = mock_type_lookup;
    support::fake_strings::wire(&mut hooks);
    support::prepare_abi_test_thread(hooks);
    unsafe {
        for (index, class) in [
            &raw mut molt_cpython_abi::abi_types::PyType_Type,
            &raw mut molt_cpython_abi::abi_types::PyUnicode_Type,
            &raw mut molt_cpython_abi::abi_types::PyLong_Type,
            &raw mut molt_cpython_abi::abi_types::PyFloat_Type,
            &raw mut molt_cpython_abi::abi_types::PyBool_Type,
        ]
        .into_iter()
        .enumerate()
        {
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(class.cast(), class_bits(index), true)
                .expect("bind normalization fixture text classes");
        }
    }
}

fn proxy(bits: u64) -> *mut PyObject {
    unsafe { GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) }
}
fn int_obj(v: i64) -> *mut PyObject {
    proxy(MoltObject::from_int(v).bits())
}
fn float_obj(v: f64) -> *mut PyObject {
    proxy(MoltObject::from_float(v).bits())
}
fn none() -> *mut PyObject {
    &raw mut Py_None
}
fn clear_err() {
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
fn err_pending() -> bool {
    !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null()
}

// These are physical inputs to the standalone normalization API, not a
// replacement slice constructor. Real ownership/crossing tests live with the
// runtime provider in cpython_abi_hooks::slice_semantics_tests.
struct PhysicalSlice(Box<PySliceObject>);
impl PhysicalSlice {
    fn as_ptr(&self) -> *mut PyObject {
        (&raw const *self.0).cast_mut().cast()
    }
}
impl Drop for PhysicalSlice {
    fn drop(&mut self) {
        molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
            for pointer in [self.0.start, self.0.stop, self.0.step] {
                molt_cpython_abi::api::refcount::Py_DECREF(pointer);
            }
        });
    }
}
// Each test supplies owned arguments; this fixture adopts those references.
fn new_slice(start: *mut PyObject, stop: *mut PyObject, step: *mut PyObject) -> PhysicalSlice {
    PhysicalSlice(Box::new(PySliceObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut molt_cpython_abi::abi_types::PySlice_Type,
        },
        start,
        stop,
        step,
    }))
}

fn unpack(slice: &PhysicalSlice) -> (i32, isize, isize, isize) {
    let (mut start, mut stop, mut step) = (0isize, 0isize, 0isize);
    let rc = unsafe {
        molt_cpython_abi::api::slice::PySlice_Unpack(
            slice.as_ptr(),
            &mut start,
            &mut stop,
            &mut step,
        )
    };
    (rc, start, stop, step)
}

// ── slice.rs:117 — non-index bounds raise TypeError, never silent -1 ────────

#[test]
fn unpack_float_bound_raises_typeerror() {
    let _g = test_guard();
    install_hooks();
    clear_err();
    // slice(1.5) — CPython: TypeError from _PyEval_SliceIndex.
    let s = new_slice(float_obj(1.5), none(), none());
    let (rc, ..) = unpack(&s);
    assert_eq!(
        rc, -1,
        "slice(1.5) must FAIL PySlice_Unpack — the pre-fix silently stored a \
         -1 bound and reported success"
    );
    assert!(err_pending(), "the -1 must carry the exception");
    let msg = support::take_current_error_text();
    assert_eq!(
        msg.as_deref(),
        Some("slice indices must be integers or None or have an __index__ method"),
        "exact _PyEval_SliceIndex TypeError text"
    );
    clear_err();
    drop(s);
}

#[test]
fn unpack_float_step_raises_typeerror_not_reverse_direction() {
    let _g = test_guard();
    install_hooks();
    clear_err();
    // The ledger case: a non-index STEP flipped iteration direction via the
    // silent -1. Must fail loud instead.
    let s = new_slice(int_obj(0), int_obj(10), float_obj(2.5));
    let (rc, ..) = unpack(&s);
    assert_eq!(rc, -1);
    assert!(err_pending());
    clear_err();
    drop(s);
}

// ── slice(0, 10**30) clamps to PY_SSIZE_T_MAX (success, not error) ──────────

#[test]
fn unpack_big_positive_stop_clamps_to_ssize_max() {
    let _g = test_guard();
    install_hooks();
    clear_err();
    // The (i64::MAX, u64::MAX] band: > isize on every host → clamp MAX.
    let s = new_slice(
        int_obj(0),
        proxy(BIG_U64_BITS.load(Ordering::SeqCst)),
        none(),
    );
    let (rc, start, stop, step) = unpack(&s);
    assert_eq!(
        rc, 0,
        "an out-of-range stop CLAMPS (sliceobject.c), no error"
    );
    assert!(!err_pending());
    assert_eq!(start, 0);
    assert_eq!(
        stop,
        isize::MAX,
        "slice(0, 10**30)-class stop must clamp to PY_SSIZE_T_MAX, not truncate"
    );
    assert_eq!(step, 1);
    drop(s);
}

#[test]
fn unpack_huge_negative_start_clamps_to_ssize_min() {
    let _g = test_guard();
    install_hooks();
    clear_err();
    // Beyond -2^64: sign resolves through the direct runtime authority.
    let s = new_slice(
        proxy(HUGE_NEG_BITS.load(Ordering::SeqCst)),
        int_obj(3),
        none(),
    );
    let (rc, start, stop, step) = unpack(&s);
    assert_eq!(rc, 0);
    assert!(!err_pending());
    assert_eq!(
        start,
        isize::MIN,
        "a negative overflow bound must clamp to PY_SSIZE_T_MIN by SIGN"
    );
    assert_eq!(stop, 3);
    assert_eq!(step, 1);
    drop(s);
}

// ── step contracts survive ───────────────────────────────────────────────────

#[test]
fn unpack_zero_step_still_valueerror_and_defaults_hold() {
    let _g = test_guard();
    install_hooks();
    clear_err();
    let s = new_slice(none(), none(), int_obj(0));
    let (rc, ..) = unpack(&s);
    assert_eq!(rc, -1, "slice step cannot be zero");
    assert!(err_pending());
    clear_err();
    drop(s);

    // Negative-step None defaults (sliceobject.c).
    let s = new_slice(none(), none(), int_obj(-2));
    let (rc, start, stop, step) = unpack(&s);
    assert_eq!(rc, 0);
    assert_eq!((start, stop, step), (isize::MAX, isize::MIN, -2));
    drop(s);
}

// ── numpy's route: PySlice_GetIndicesEx inherits the loud failure ────────────

#[test]
fn get_indices_ex_propagates_typeerror_for_bad_bound() {
    let _g = test_guard();
    install_hooks();
    clear_err();
    let s = new_slice(float_obj(0.5), none(), none());
    let (mut start, mut stop, mut step, mut len) = (0isize, 0isize, 0isize, 0isize);
    let rc = unsafe {
        molt_cpython_abi::api::slice::PySlice_GetIndicesEx(
            s.as_ptr(),
            10,
            &mut start,
            &mut stop,
            &mut step,
            &mut len,
        )
    };
    assert_eq!(rc, -1, "ndarray basic-indexing path must see the failure");
    assert!(err_pending());
    clear_err();
    drop(s);
}

// ── slice.rs:209 — legacy PySlice_GetIndices rejects, never clamps ───────────

#[test]
fn legacy_get_indices_rejects_out_of_range_and_non_long() {
    let _g = test_guard();
    install_hooks();
    clear_err();

    // stop > length must return -1 (the pre-fix GetIndicesEx delegation
    // CLAMPED it to length and reported success).
    let s = new_slice(int_obj(0), int_obj(11), none());
    let (mut start, mut stop, mut step) = (0isize, 0isize, 0isize);
    let rc = unsafe {
        molt_cpython_abi::api::slice::PySlice_GetIndices(
            s.as_ptr(),
            10,
            &mut start,
            &mut stop,
            &mut step,
        )
    };
    assert_eq!(rc, -1, "legacy GetIndices must REJECT stop > length");
    drop(s);

    // A float field is not a PyLong: -1 (legacy PyLong_Check gate).
    let s = new_slice(float_obj(1.0), none(), none());
    let rc = unsafe {
        molt_cpython_abi::api::slice::PySlice_GetIndices(
            s.as_ptr(),
            10,
            &mut start,
            &mut stop,
            &mut step,
        )
    };
    assert_eq!(rc, -1, "legacy GetIndices requires exact ints per field");
    drop(s);
    clear_err();

    // The happy path with a negative index adjusts once by length.
    let s = new_slice(int_obj(-3), int_obj(9), none());
    let rc = unsafe {
        molt_cpython_abi::api::slice::PySlice_GetIndices(
            s.as_ptr(),
            10,
            &mut start,
            &mut stop,
            &mut step,
        )
    };
    assert_eq!(rc, 0);
    assert_eq!((start, stop, step), (7, 9, 1));
    drop(s);
}
