//! F1 mask-proof gates for the `PyLong_As*` / `PyFloat_AsDouble` /
//! `PyComplex_*AsDouble` conversion contracts (numbers.rs ledger rows).
//!
//! CPython 3.12 primary sources: Objects/longobject.c, Objects/floatobject.c,
//! Objects/complexobject.c. The teeth:
//!   * `numbers.rs:486` PyLong_AsSsize_t — silent -1 for a non-int was numpy's
//!     reshape-"infer" sentinel; must be TypeError "an integer is required".
//!   * `numbers.rs:424` PyLong_AsLong — silent truncation + no exception.
//!   * `numbers.rs:539/:544` unsigned family — negative wrapped to huge values.
//!   * `numbers.rs:730` PyFloat_AsDouble — silent NaN for non-numbers.
//!   * `numbers.rs:895` PyComplex_ImagAsDouble — stray TypeError on 0.0 path.
//!   * `numbers.rs:936` PyLong_Check(True) must be 1 (bool is an int subtype).
//!
//! The mock hook table models a heap BigInt in the (i64::MAX, u64::MAX] band
//! plus one beyond-u64 bignum, so the OverflowError band logic is proven
//! without the full runtime.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{Py_None, Py_True, PyObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

// Handles the mock hooks answer for (0 until installed).
static BIG_U64_BITS: AtomicU64 = AtomicU64::new(0); // value: u64::MAX - 1
static HUGE_BITS: AtomicU64 = AtomicU64::new(0); // beyond ±2^64
static TEST_LOCK: Mutex<()> = Mutex::new(());
static NEG_HUGE_BITS: AtomicU64 = AtomicU64::new(0);
static MIN_I64_BITS: AtomicU64 = AtomicU64::new(0);
static MAX_I64_BITS: AtomicU64 = AtomicU64::new(0);

const BIG_U64_VALUE: u64 = u64::MAX - 1;
const HUGE_LOW_U64: u64 = 0x1234_5678_9abc_def0;
const NEG_HUGE_LOW_U64: u64 = 7;
/// The f64 the mock TrueDivide authority reports for HUGE (exact-authority path).
const HUGE_AS_F64: f64 = 1.0e25;

unsafe extern "C" fn mock_classify_heap(bits: u64) -> u8 {
    if support::fake_strings::contains(bits) {
        molt_cpython_abi::abi_types::MoltTypeTag::Str as u8
    } else if bits == BIG_U64_BITS.load(Ordering::SeqCst)
        || bits == HUGE_BITS.load(Ordering::SeqCst)
        || bits == NEG_HUGE_BITS.load(Ordering::SeqCst)
        || bits == MIN_I64_BITS.load(Ordering::SeqCst)
        || bits == MAX_I64_BITS.load(Ordering::SeqCst)
    {
        molt_cpython_abi::abi_types::MoltTypeTag::Int as u8
    } else {
        molt_cpython_abi::abi_types::MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn mock_int_as_i64_checked(bits: u64, out: *mut i64) -> std::os::raw::c_int {
    if bits == MIN_I64_BITS.load(Ordering::SeqCst) {
        unsafe { *out = i64::MIN };
        0
    } else if bits == MAX_I64_BITS.load(Ordering::SeqCst) {
        unsafe { *out = i64::MAX };
        0
    } else {
        -1
    }
}

unsafe extern "C" fn mock_int_as_u64_checked(bits: u64, out: *mut u64) -> std::os::raw::c_int {
    if bits == BIG_U64_BITS.load(Ordering::SeqCst) {
        unsafe { *out = BIG_U64_VALUE };
        0
    } else {
        -1 // HUGE exceeds u64 too
    }
}

unsafe extern "C" fn mock_int_as_u64_mask(
    bits: u64,
    width: u32,
    out: *mut u64,
) -> std::os::raw::c_int {
    let value = if bits == BIG_U64_BITS.load(Ordering::SeqCst) {
        BIG_U64_VALUE
    } else if bits == HUGE_BITS.load(Ordering::SeqCst) {
        HUGE_LOW_U64
    } else if bits == NEG_HUGE_BITS.load(Ordering::SeqCst) {
        NEG_HUGE_LOW_U64
    } else {
        return -1;
    };
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    unsafe { *out = value & mask };
    0
}

unsafe extern "C" fn mock_int_sign(bits: u64) -> std::os::raw::c_int {
    if bits == NEG_HUGE_BITS.load(Ordering::SeqCst) {
        -1
    } else if bits == HUGE_BITS.load(Ordering::SeqCst) {
        1
    } else {
        0
    }
}

/// The runtime numeric authority stand-in: `HUGE / 1` (TrueDivide) yields the
/// exact float; everything else fails closed.
unsafe extern "C" fn mock_number_binary_op(
    op: u32,
    a: u64,
    _b: u64,
) -> molt_cpython_abi::hooks::OwnedHandleResult {
    if op == molt_cpython_abi::hooks::NumberBinaryOp::TrueDivide as u32
        && a == HUGE_BITS.load(Ordering::SeqCst)
    {
        return molt_cpython_abi::hooks::OwnedHandleResult::ok(
            MoltObject::from_float(HUGE_AS_F64).bits(),
        );
    }
    if op == molt_cpython_abi::hooks::NumberBinaryOp::Rshift as u32 {
        if a == HUGE_BITS.load(Ordering::SeqCst) {
            return molt_cpython_abi::hooks::OwnedHandleResult::ok(MoltObject::from_int(0).bits());
        }
        if a == NEG_HUGE_BITS.load(Ordering::SeqCst) {
            return molt_cpython_abi::hooks::OwnedHandleResult::ok(MoltObject::from_int(-1).bits());
        }
    }
    molt_cpython_abi::hooks::OwnedHandleResult::error()
}

fn install_hooks() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    if BIG_U64_BITS.load(Ordering::SeqCst) == 0 {
        let a: *mut u8 = Box::into_raw(Box::new(0u8));
        let b: *mut u8 = Box::into_raw(Box::new(0u8));
        let c: *mut u8 = Box::into_raw(Box::new(0u8));
        let d: *mut u8 = Box::into_raw(Box::new(0u8));
        let e: *mut u8 = Box::into_raw(Box::new(0u8));
        BIG_U64_BITS.store(MoltObject::from_ptr(a).bits(), Ordering::SeqCst);
        HUGE_BITS.store(MoltObject::from_ptr(b).bits(), Ordering::SeqCst);
        NEG_HUGE_BITS.store(MoltObject::from_ptr(c).bits(), Ordering::SeqCst);
        MIN_I64_BITS.store(MoltObject::from_ptr(d).bits(), Ordering::SeqCst);
        MAX_I64_BITS.store(MoltObject::from_ptr(e).bits(), Ordering::SeqCst);
    }
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.classify_heap = mock_classify_heap;
    hooks.int_as_i64_checked = mock_int_as_i64_checked;
    hooks.int_as_u64_checked = mock_int_as_u64_checked;
    hooks.int_as_u64_mask = mock_int_as_u64_mask;
    hooks.int_sign = mock_int_sign;
    hooks.number_binary_op = mock_number_binary_op;
    support::fake_strings::wire(&mut hooks);
    support::prepare_abi_test_thread(hooks);
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
fn clear_err() {
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
fn err_pending() -> bool {
    !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null()
}
fn err_is(exc: *mut PyObject) -> bool {
    unsafe { molt_cpython_abi::api::errors::PyErr_ExceptionMatches(exc) == 1 }
}

// ── numbers.rs:486 — PyLong_AsSsize_t: never a bare -1 ──────────────────────

#[test]
fn as_ssize_t_non_int_raises_typeerror_not_silent_minus_one() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let f = float_obj(1.5);
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsSsize_t(f) };
    assert_eq!(v, -1);
    assert!(
        err_is((&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()),
        "PyLong_AsSsize_t(float) must raise TypeError 'an integer is required' \
         — a bare -1 is numpy's reshape-infer sentinel (silent wrong shapes)"
    );
    let msg = support::take_current_error_text();
    assert_eq!(msg.as_deref(), Some("an integer is required"));
    clear_err();

    // None is also a non-int (strict PyLong_Check contract, no __index__).
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsSsize_t(&raw mut Py_None) };
    assert_eq!(v, -1);
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()
    ));
    clear_err();
}

#[test]
fn as_ssize_t_beyond_range_raises_overflow() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    // BIG (u64::MAX-1) exceeds isize on every host.
    let big = proxy(BIG_U64_BITS.load(Ordering::SeqCst));
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsSsize_t(big) };
    assert_eq!(v, -1);
    assert!(
        err_is((&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>()),
        "an int beyond ssize_t must raise OverflowError, never truncate"
    );
    clear_err();
}

// ── numbers.rs:424 — PyLong_AsLong: __index__ contract + honest failures ────

#[test]
fn as_long_non_int_raises_via_index_dispatch() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let f = float_obj(2.5);
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(f) };
    assert_eq!(v, -1);
    assert!(
        err_is((&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()),
        "PyLong_AsLong(float) must raise TypeError (float has no __index__)"
    );
    clear_err();
}

#[test]
fn as_long_beyond_c_long_raises_overflow() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let big = proxy(BIG_U64_BITS.load(Ordering::SeqCst));
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(big) };
    assert_eq!(v, -1);
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>()
    ));
    let msg = support::take_current_error_text();
    assert_eq!(
        msg.as_deref(),
        Some("Python int too large to convert to C long")
    );
    clear_err();
}

// ── _PyLong_AsInt: 32-bit target on every platform (portable teeth) ─────────

#[test]
fn as_int_2_pow_40_raises_overflow_everywhere() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let big = int_obj(1 << 40); // inline int, > i32 on all hosts
    let v = unsafe { molt_cpython_abi::api::numbers::_PyLong_AsInt(big) };
    assert_eq!(v, -1);
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>()
    ));
    let msg = support::take_current_error_text();
    assert_eq!(
        msg.as_deref(),
        Some("Python int too large to convert to C int")
    );
    clear_err();

    // In-range still converts.
    let small = int_obj(-77);
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_PyLong_AsInt(small) },
        -77
    );
    assert!(!err_pending());
}

// ── numbers.rs:539/:544 — unsigned family: negatives raise, never wrap ──────

#[test]
fn as_unsigned_long_negative_raises_overflow_not_wrap() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let neg = int_obj(-1);
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLong(neg) };
    assert_eq!(
        v,
        std::os::raw::c_ulong::MAX,
        "error sentinel is (unsigned long)-1"
    );
    assert!(
        err_is((&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>()),
        "PyLong_AsUnsignedLong(-1) must raise OverflowError, not wrap to ULONG_MAX"
    );
    let msg = support::take_current_error_text();
    assert_eq!(
        msg.as_deref(),
        Some("can't convert negative value to unsigned int")
    );
    clear_err();
}

#[test]
fn as_unsigned_long_long_contracts() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();

    // Non-int → TypeError (strict; no __index__ per longobject.c).
    let f = float_obj(3.5);
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLong(f) };
    assert_eq!(v, u64::MAX);
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()
    ));
    clear_err();

    // The (i64::MAX, u64::MAX] band converts exactly.
    let big = proxy(BIG_U64_BITS.load(Ordering::SeqCst));
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLong(big) };
    assert_eq!(v, BIG_U64_VALUE, "the unsigned 64-bit band must round-trip");
    assert!(!err_pending());

    // Beyond ±2^64 → OverflowError.
    let huge = proxy(HUGE_BITS.load(Ordering::SeqCst));
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLong(huge) };
    assert_eq!(v, u64::MAX);
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>()
    ));
    clear_err();
}

// ── AndOverflow contract: -1 + *overflow, never a clamp ─────────────────────

#[test]
fn as_long_long_and_overflow_returns_minus_one_not_clamp() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let big = proxy(BIG_U64_BITS.load(Ordering::SeqCst));
    let mut overflow = 0;
    let v =
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLongLongAndOverflow(big, &mut overflow) };
    assert_eq!(
        v, -1,
        "CPython returns -1 on overflow (the pre-fix clamp to LLONG_MAX was divergent)"
    );
    assert_eq!(overflow, 1, "positive overflow must set *overflow = 1");
    assert!(!err_pending(), "overflow via *overflow sets NO exception");

    // Non-int: TypeError with *overflow = 0.
    let f = float_obj(0.5);
    let mut overflow = 99;
    let v =
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLongLongAndOverflow(f, &mut overflow) };
    assert_eq!(v, -1);
    assert_eq!(overflow, 0);
    assert!(err_pending());
    clear_err();
}

#[test]
fn signed_boundaries_and_negative_overflow_match_cpython() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();

    for (bits, value) in [
        (MIN_I64_BITS.load(Ordering::SeqCst), i64::MIN),
        (MAX_I64_BITS.load(Ordering::SeqCst), i64::MAX),
    ] {
        let py = proxy(bits);
        assert_eq!(
            unsafe { molt_cpython_abi::api::numbers::PyLong_AsLongLong(py) },
            value
        );
        assert!(!err_pending());
    }

    let negative_huge = proxy(NEG_HUGE_BITS.load(Ordering::SeqCst));
    let mut overflow = 0;
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::PyLong_AsLongLongAndOverflow(
                negative_huge,
                &mut overflow,
            )
        },
        -1
    );
    assert_eq!(overflow, -1);
    assert!(!err_pending());

    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLongLong(negative_huge) },
        -1
    );
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>()
    ));
    clear_err();
}

#[test]
fn unsigned_masks_truncate_without_setting_overflow() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();

    let negative_one = int_obj(-1);
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongMask(negative_one) },
        std::os::raw::c_ulong::MAX
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLongMask(negative_one) },
        u64::MAX
    );
    assert!(!err_pending());

    let huge = proxy(HUGE_BITS.load(Ordering::SeqCst));
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLongMask(huge) },
        HUGE_LOW_U64
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongMask(huge) },
        (HUGE_LOW_U64 & std::os::raw::c_ulong::MAX as u64) as std::os::raw::c_ulong
    );
    assert!(
        !err_pending(),
        "mask variants never raise for integer overflow"
    );
}

// ── PyLong_AsNativeBytes: true size query ────────────────────────────────────

#[test]
fn as_native_bytes_reports_true_minimal_width() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let five = int_obj(5);
    let mut buf = [0xAAu8; 8];
    let required = unsafe {
        molt_cpython_abi::api::numbers::PyLong_AsNativeBytes(
            five,
            buf.as_mut_ptr().cast(),
            buf.len() as isize,
            1, // Py_ASNATIVEBYTES_LITTLE_ENDIAN
        )
    };
    assert_eq!(
        required, 1,
        "the value 5 needs ONE byte — the pre-fix body always reported 8"
    );
    assert_eq!(buf[0], 5);
    assert!(!err_pending());

    // Failure sets an exception (never a bare -1).
    let f = float_obj(1.5);
    let required = unsafe {
        molt_cpython_abi::api::numbers::PyLong_AsNativeBytes(
            f,
            buf.as_mut_ptr().cast(),
            buf.len() as isize,
            1,
        )
    };
    assert_eq!(required, -1);
    assert!(err_pending());
    clear_err();
}

// ── numbers.rs:730 — PyFloat_AsDouble: no silent NaN ────────────────────────

#[test]
fn float_as_double_non_number_raises_typeerror_not_nan() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let v = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(&raw mut Py_None) };
    assert_eq!(
        v, -1.0,
        "CPython returns -1.0 with TypeError — the pre-fix silent NaN poisoned \
         numpy compute paths with fake values"
    );
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()
    ));
    clear_err();

    // The bignum band converts exactly through the checked hooks.
    let big = proxy(BIG_U64_BITS.load(Ordering::SeqCst));
    let v = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(big) };
    assert_eq!(v, BIG_U64_VALUE as f64);
    assert!(!err_pending());
}

// ── PyLong_AsDouble: exact conversion via the runtime numeric authority ─────

#[test]
fn long_as_double_routes_beyond_u64_through_the_authority() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let huge = proxy(HUGE_BITS.load(Ordering::SeqCst));
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsDouble(huge) };
    assert_eq!(
        v, HUGE_AS_F64,
        "a bignum beyond ±2^64 must convert through the runtime TrueDivide \
         authority (exact), not raise or truncate"
    );
    assert!(!err_pending());

    // Non-int → TypeError 'an integer is required'.
    let f = float_obj(2.0);
    let v = unsafe { molt_cpython_abi::api::numbers::PyLong_AsDouble(f) };
    assert_eq!(v, -1.0);
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()
    ));
    clear_err();
}

// ── numbers.rs:895 — PyComplex_ImagAsDouble: 0.0 with NO stray error ────────

#[test]
fn complex_imag_as_double_non_complex_is_clean_zero() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let nine = int_obj(9);
    let imag = unsafe { molt_cpython_abi::api::numbers::PyComplex_ImagAsDouble(nine) };
    assert_eq!(imag, 0.0);
    assert!(
        !err_pending(),
        "ImagAsDouble(non-complex) returns 0.0 with NO exception \
         (complexobject.c) — the pre-fix left a live TypeError"
    );
    let real = unsafe { molt_cpython_abi::api::numbers::PyComplex_RealAsDouble(nine) };
    assert_eq!(real, 9.0);
    assert!(!err_pending());
}

// ── numbers.rs:936 — PyLong_Check(True) == 1 (bool is an int subtype) ───────

#[test]
fn pylong_check_true_is_one_and_bignum_is_int() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::PyLong_Check((&raw mut Py_True).cast::<PyObject>())
        },
        1,
        "PyLong_Check(True) is 1 in CPython (Py_TPFLAGS_LONG_SUBCLASS)"
    );
    // A heap bignum is an int too (the pre-fix is_int() said 0).
    let big = proxy(BIG_U64_BITS.load(Ordering::SeqCst));
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_Check(big) },
        1
    );
    // And a float still is not.
    let f = float_obj(1.0);
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_Check(f) },
        0
    );
}
