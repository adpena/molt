// Comparison operator tests — cross-type comparison parity with CPython.

use molt_obj_model::MoltObject;
use std::sync::Once;

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/test_comparisons"
));

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_exception_clear") ]
    fn molt_exception_clear(context: u64) -> u64;
}
static INIT: Once = Once::new();
fn init() -> molt_runtime_core::CoreGilGuard {
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });

    let guard = molt_runtime_core::CoreGilGuard::new();
    let py = unsafe { guard.__token_unchecked() };
    unsafe { molt_exception_clear(py.attached_context()) };
    guard
}

fn int(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}
fn f(v: f64) -> u64 {
    MoltObject::from_float(v).bits()
}
fn none() -> u64 {
    MoltObject::none().bits()
}
fn b(v: bool) -> u64 {
    MoltObject::from_bool(v).bits()
}
fn as_bool(bits: u64) -> bool {
    MoltObject::from_bits(bits).as_bool().unwrap_or(false)
}

// ── is operator ─────────────────────────────────────────
#[test]
fn test_is_same_int() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_is(py.attached_context(), int(1), int(1))
    }));
}
#[test]
fn test_is_none_none() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_is(py.attached_context(), none(), none())
    }));
}
#[test]
fn test_is_diff_ints() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(!as_bool(unsafe {
        molt_runtime::molt_is(py.attached_context(), int(1), int(2))
    }));
}
#[test]
fn test_is_bool_true() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_is(py.attached_context(), b(true), b(true))
    }));
}

// ── eq across types ─────────────────────────────────────
#[test]
fn test_eq_int_float() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_eq(py.attached_context(), int(1), f(1.0))
    }));
}
#[test]
fn test_eq_bool_int() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_eq(py.attached_context(), b(true), int(1))
    }));
}
#[test]
fn test_eq_bool_false_zero() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_eq(py.attached_context(), b(false), int(0))
    }));
}
#[test]
fn test_ne_int_none() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_ne(py.attached_context(), int(0), none())
    }));
}

// ── not ─────────────────────────────────────────────────
#[test]
fn test_not_true() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_not(py.attached_context(), b(false))
    }));
}
#[test]
fn test_not_false() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(!as_bool(unsafe {
        molt_runtime::molt_not(py.attached_context(), b(true))
    }));
}
#[test]
fn test_not_zero() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_not(py.attached_context(), int(0))
    }));
}
#[test]
fn test_not_nonzero() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(!as_bool(unsafe {
        molt_runtime::molt_not(py.attached_context(), int(42))
    }));
}
#[test]
fn test_not_none() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_not(py.attached_context(), none())
    }));
}

// ── ordering ────────────────────────────────────────────
#[test]
fn test_lt_int_float() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_lt(py.attached_context(), int(1), f(1.5))
    }));
}
#[test]
fn test_le_equal() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_le(py.attached_context(), int(5), int(5))
    }));
}
#[test]
fn test_ge_less() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(!as_bool(unsafe {
        molt_runtime::molt_ge(py.attached_context(), int(3), int(5))
    }));
}
#[test]
fn test_gt_greater() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_gt(py.attached_context(), f(3.0), f(2.0))
    }));
}
