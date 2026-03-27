// Comparison operator tests — cross-type comparison parity with CPython.

use molt_obj_model::MoltObject;
use std::sync::Once;

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    MoltObject::none().bits()
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_: u64) -> u64 {
    MoltObject::none().bits()
}

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
}
static INIT: Once = Once::new();
fn init() {
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });
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
    init();
    assert!(as_bool(molt_runtime::molt_is(int(1), int(1))));
}
#[test]
fn test_is_none_none() {
    init();
    assert!(as_bool(molt_runtime::molt_is(none(), none())));
}
#[test]
fn test_is_diff_ints() {
    init();
    assert!(!as_bool(molt_runtime::molt_is(int(1), int(2))));
}
#[test]
fn test_is_bool_true() {
    init();
    assert!(as_bool(molt_runtime::molt_is(b(true), b(true))));
}

// ── eq across types ─────────────────────────────────────
#[test]
fn test_eq_int_float() {
    init();
    assert!(as_bool(molt_runtime::molt_eq(int(1), f(1.0))));
}
#[test]
fn test_eq_bool_int() {
    init();
    assert!(as_bool(molt_runtime::molt_eq(b(true), int(1))));
}
#[test]
fn test_eq_bool_false_zero() {
    init();
    assert!(as_bool(molt_runtime::molt_eq(b(false), int(0))));
}
#[test]
fn test_ne_int_none() {
    init();
    assert!(as_bool(molt_runtime::molt_ne(int(0), none())));
}

// ── not ─────────────────────────────────────────────────
#[test]
fn test_not_true() {
    init();
    assert!(as_bool(molt_runtime::molt_not(b(false))));
}
#[test]
fn test_not_false() {
    init();
    assert!(!as_bool(molt_runtime::molt_not(b(true))));
}
#[test]
fn test_not_zero() {
    init();
    assert!(as_bool(molt_runtime::molt_not(int(0))));
}
#[test]
fn test_not_nonzero() {
    init();
    assert!(!as_bool(molt_runtime::molt_not(int(42))));
}
#[test]
fn test_not_none() {
    init();
    assert!(as_bool(molt_runtime::molt_not(none())));
}

// ── ordering ────────────────────────────────────────────
#[test]
fn test_lt_int_float() {
    init();
    assert!(as_bool(molt_runtime::molt_lt(int(1), f(1.5))));
}
#[test]
fn test_le_equal() {
    init();
    assert!(as_bool(molt_runtime::molt_le(int(5), int(5))));
}
#[test]
fn test_ge_less() {
    init();
    assert!(!as_bool(molt_runtime::molt_ge(int(3), int(5))));
}
#[test]
fn test_gt_greater() {
    init();
    assert!(as_bool(molt_runtime::molt_gt(f(3.0), f(2.0))));
}
