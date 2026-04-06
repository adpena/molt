// Integer arithmetic and conversion tests.
// Verifies CPython parity for int operations via the molt_* public API.

use molt_obj_model::MoltObject;
use std::sync::Once;

// The runtime expects these symbols from the compiled Python module.
// Provide stubs so integration tests can link.
#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_bootstrap() -> u64 {
    MoltObject::none().bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_isolate_import(_name_bits: u64) -> u64 {
    MoltObject::none().bits()
}

// molt_runtime_init is pub extern "C" but not re-exported in the Rust API.
unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    fn molt_exception_clear() -> u64;
}

static INIT: Once = Once::new();

fn init_runtime() {
    INIT.call_once(|| unsafe {
        molt_runtime_init();
    });
    let _ = unsafe { molt_exception_clear() };
}

fn int(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}

fn float(v: f64) -> u64 {
    MoltObject::from_float(v).bits()
}

fn as_int(bits: u64) -> i64 {
    MoltObject::from_bits(bits).as_int().expect("expected int")
}

fn as_float(bits: u64) -> f64 {
    MoltObject::from_bits(bits)
        .as_float()
        .expect("expected float")
}

fn is_true(bits: u64) -> bool {
    MoltObject::from_bits(bits).as_bool().unwrap_or(false)
}

// ── Addition ────────────────────────────────────────────

#[test]
fn test_add_basic() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_add(int(2), int(3))), 5);
}

#[test]
fn test_add_negative() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_add(int(-10), int(7))), -3);
}

#[test]
fn test_add_zero() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_add(int(0), int(0))), 0);
}

#[test]
fn test_add_large() {
    init_runtime();
    assert_eq!(
        as_int(molt_runtime::molt_add(int(1_000_000), int(2_000_000))),
        3_000_000
    );
}

// ── Subtraction ─────────────────────────────────────────

#[test]
fn test_sub_basic() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_sub(int(10), int(3))), 7);
}

#[test]
fn test_sub_negative_result() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_sub(int(3), int(10))), -7);
}

// ── Multiplication ──────────────────────────────────────

#[test]
fn test_mul_basic() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_mul(int(6), int(7))), 42);
}

#[test]
fn test_mul_zero() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_mul(int(999), int(0))), 0);
}

#[test]
fn test_mul_negative() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_mul(int(-3), int(4))), -12);
}

// ── Division ────────────────────────────────────────────

#[test]
fn test_floordiv() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_floordiv(int(7), int(2))), 3);
}

#[test]
fn test_floordiv_negative() {
    init_runtime();
    // Python: -7 // 2 == -4 (rounds toward negative infinity)
    assert_eq!(as_int(molt_runtime::molt_floordiv(int(-7), int(2))), -4);
}

#[test]
fn test_mod_basic() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_mod(int(10), int(3))), 1);
}

#[test]
fn test_mod_negative() {
    init_runtime();
    // Python: -7 % 3 == 2 (result has sign of divisor)
    assert_eq!(as_int(molt_runtime::molt_mod(int(-7), int(3))), 2);
}

// ── Power ───────────────────────────────────────────────

#[test]
fn test_pow() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_pow(int(2), int(10))), 1024);
}

#[test]
fn test_pow_zero_exponent() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_pow(int(5), int(0))), 1);
}

// ── Bitwise ─────────────────────────────────────────────

#[test]
fn test_bit_and() {
    init_runtime();
    assert_eq!(
        as_int(molt_runtime::molt_bit_and(int(0b1100), int(0b1010))),
        0b1000
    );
}

#[test]
fn test_bit_or() {
    init_runtime();
    assert_eq!(
        as_int(molt_runtime::molt_bit_or(int(0b1100), int(0b1010))),
        0b1110
    );
}

#[test]
fn test_bit_xor() {
    init_runtime();
    assert_eq!(
        as_int(molt_runtime::molt_bit_xor(int(0b1100), int(0b1010))),
        0b0110
    );
}

#[test]
fn test_lshift() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_lshift(int(1), int(10))), 1024);
}

#[test]
fn test_rshift() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_rshift(int(1024), int(3))), 128);
}

// ── Comparison ──────────────────────────────────────────

#[test]
fn test_lt_true() {
    init_runtime();
    assert!(is_true(molt_runtime::molt_lt(int(1), int(2))));
}

#[test]
fn test_lt_false() {
    init_runtime();
    assert!(!is_true(molt_runtime::molt_lt(int(2), int(1))));
}

#[test]
fn test_eq_true() {
    init_runtime();
    assert!(is_true(molt_runtime::molt_eq(int(42), int(42))));
}

#[test]
fn test_eq_false() {
    init_runtime();
    assert!(!is_true(molt_runtime::molt_eq(int(42), int(43))));
}

#[test]
fn test_ne_true() {
    init_runtime();
    assert!(is_true(molt_runtime::molt_ne(int(1), int(2))));
}

#[test]
fn test_ge_equal() {
    init_runtime();
    assert!(is_true(molt_runtime::molt_ge(int(5), int(5))));
}

#[test]
fn test_gt_false_when_equal() {
    init_runtime();
    assert!(!is_true(molt_runtime::molt_gt(int(5), int(5))));
}

// ── Mixed int/float ─────────────────────────────────────

#[test]
fn test_add_int_float() {
    init_runtime();
    assert_eq!(as_float(molt_runtime::molt_add(int(2), float(3.5))), 5.5);
}

#[test]
fn test_mul_int_float() {
    init_runtime();
    assert_eq!(as_float(molt_runtime::molt_mul(int(3), float(2.5))), 7.5);
}

// ── Unary ───────────────────────────────────────────────

#[test]
fn test_invert_zero() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_invert(int(0))), -1);
}

#[test]
fn test_invert_positive() {
    init_runtime();
    // Python: ~5 == -6
    assert_eq!(as_int(molt_runtime::molt_invert(int(5))), -6);
}

// ── Truthiness ──────────────────────────────────────────

#[test]
fn test_truthy_zero_is_false() {
    init_runtime();
    assert_eq!(molt_runtime::molt_is_truthy(int(0)), 0);
}

#[test]
fn test_truthy_nonzero_is_true() {
    init_runtime();
    assert_ne!(molt_runtime::molt_is_truthy(int(42)), 0);
}

#[test]
fn test_truthy_negative_is_true() {
    init_runtime();
    assert_ne!(molt_runtime::molt_is_truthy(int(-1)), 0);
}

// ── Builtins ────────────────────────────────────────────

#[test]
fn test_abs_positive() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_abs_builtin(int(42))), 42);
}

#[test]
fn test_abs_negative() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_abs_builtin(int(-42))), 42);
}

#[test]
fn test_abs_zero() {
    init_runtime();
    assert_eq!(as_int(molt_runtime::molt_abs_builtin(int(0))), 0);
}

#[test]
fn test_len_requires_sequence() {
    // len(42) should raise TypeError — we just check it doesn't crash
    init_runtime();
    let _r = molt_runtime::molt_len(int(42));
    // Exception should be pending; value is sentinel
}
