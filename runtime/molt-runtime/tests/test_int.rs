// Integer arithmetic and conversion tests.
// Verifies CPython parity for int operations via the molt_* public API.

use molt_obj_model::MoltObject;
use std::sync::Once;

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/test_int"
));

// molt_runtime_init is pub extern "C" but not re-exported in the Rust API.
unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    #[link_name = molt_runtime_core::attached_symbol!("molt_exception_clear") ]
    fn molt_exception_clear(context: u64) -> u64;
}

static INIT: Once = Once::new();

fn init_runtime() -> molt_runtime_core::CoreGilGuard {
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
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_add(py.attached_context(), int(2), int(3)) }),
        5
    );
}

#[test]
fn test_add_negative() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_add(py.attached_context(), int(-10), int(7)) }),
        -3
    );
}

#[test]
fn test_add_zero() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_add(py.attached_context(), int(0), int(0)) }),
        0
    );
}

#[test]
fn test_add_large() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe {
            molt_runtime::molt_add(py.attached_context(), int(1_000_000), int(2_000_000))
        }),
        3_000_000
    );
}

// ── Subtraction ─────────────────────────────────────────

#[test]
fn test_sub_basic() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_sub(py.attached_context(), int(10), int(3)) }),
        7
    );
}

#[test]
fn test_sub_negative_result() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_sub(py.attached_context(), int(3), int(10)) }),
        -7
    );
}

// ── Multiplication ──────────────────────────────────────

#[test]
fn test_mul_basic() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_mul(py.attached_context(), int(6), int(7)) }),
        42
    );
}

#[test]
fn test_mul_zero() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_mul(py.attached_context(), int(999), int(0)) }),
        0
    );
}

#[test]
fn test_mul_negative() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_mul(py.attached_context(), int(-3), int(4)) }),
        -12
    );
}

// ── Division ────────────────────────────────────────────

#[test]
fn test_floordiv() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_floordiv(py.attached_context(), int(7), int(2)) }),
        3
    );
}

#[test]
fn test_floordiv_negative() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    // Python: -7 // 2 == -4 (rounds toward negative infinity)
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_floordiv(py.attached_context(), int(-7), int(2)) }),
        -4
    );
}

#[test]
fn test_mod_basic() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_mod(py.attached_context(), int(10), int(3)) }),
        1
    );
}

#[test]
fn test_mod_negative() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    // Python: -7 % 3 == 2 (result has sign of divisor)
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_mod(py.attached_context(), int(-7), int(3)) }),
        2
    );
}

// ── Power ───────────────────────────────────────────────

#[test]
fn test_pow() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_pow(py.attached_context(), int(2), int(10)) }),
        1024
    );
}

#[test]
fn test_pow_zero_exponent() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_pow(py.attached_context(), int(5), int(0)) }),
        1
    );
}

// ── Bitwise ─────────────────────────────────────────────

#[test]
fn test_bit_and() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe {
            molt_runtime::molt_bit_and(py.attached_context(), int(0b1100), int(0b1010))
        }),
        0b1000
    );
}

#[test]
fn test_bit_or() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe {
            molt_runtime::molt_bit_or(py.attached_context(), int(0b1100), int(0b1010))
        }),
        0b1110
    );
}

#[test]
fn test_bit_xor() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe {
            molt_runtime::molt_bit_xor(py.attached_context(), int(0b1100), int(0b1010))
        }),
        0b0110
    );
}

#[test]
fn test_lshift() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_lshift(py.attached_context(), int(1), int(10)) }),
        1024
    );
}

#[test]
fn test_rshift() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_rshift(py.attached_context(), int(1024), int(3)) }),
        128
    );
}

// ── Comparison ──────────────────────────────────────────

#[test]
fn test_lt_true() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(is_true(unsafe {
        molt_runtime::molt_lt(py.attached_context(), int(1), int(2))
    }));
}

#[test]
fn test_lt_false() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(!is_true(unsafe {
        molt_runtime::molt_lt(py.attached_context(), int(2), int(1))
    }));
}

#[test]
fn test_eq_true() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(is_true(unsafe {
        molt_runtime::molt_eq(py.attached_context(), int(42), int(42))
    }));
}

#[test]
fn test_eq_false() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(!is_true(unsafe {
        molt_runtime::molt_eq(py.attached_context(), int(42), int(43))
    }));
}

#[test]
fn test_ne_true() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(is_true(unsafe {
        molt_runtime::molt_ne(py.attached_context(), int(1), int(2))
    }));
}

#[test]
fn test_ge_equal() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(is_true(unsafe {
        molt_runtime::molt_ge(py.attached_context(), int(5), int(5))
    }));
}

#[test]
fn test_gt_false_when_equal() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(!is_true(unsafe {
        molt_runtime::molt_gt(py.attached_context(), int(5), int(5))
    }));
}

// ── Mixed int/float ─────────────────────────────────────

#[test]
fn test_add_int_float() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_float(unsafe { molt_runtime::molt_add(py.attached_context(), int(2), float(3.5)) }),
        5.5
    );
}

#[test]
fn test_mul_int_float() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_float(unsafe { molt_runtime::molt_mul(py.attached_context(), int(3), float(2.5)) }),
        7.5
    );
}

// ── Unary ───────────────────────────────────────────────

#[test]
fn test_invert_zero() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_invert(py.attached_context(), int(0)) }),
        -1
    );
}

#[test]
fn test_invert_positive() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    // Python: ~5 == -6
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_invert(py.attached_context(), int(5)) }),
        -6
    );
}

// ── Truthiness ──────────────────────────────────────────

#[test]
fn test_truthy_zero_is_false() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), int(0)) },
        0
    );
}

#[test]
fn test_truthy_nonzero_is_true() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_ne!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), int(42)) },
        0
    );
}

#[test]
fn test_truthy_negative_is_true() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_ne!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), int(-1)) },
        0
    );
}

// ── Builtins ────────────────────────────────────────────

#[test]
fn test_abs_positive() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_abs_builtin(py.attached_context(), int(42)) }),
        42
    );
}

#[test]
fn test_abs_negative() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_abs_builtin(py.attached_context(), int(-42)) }),
        42
    );
}

#[test]
fn test_abs_zero() {
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_int(unsafe { molt_runtime::molt_abs_builtin(py.attached_context(), int(0)) }),
        0
    );
}

#[test]
fn test_len_requires_sequence() {
    // len(42) should raise TypeError — we just check it doesn't crash
    let fixture_execution = init_runtime();
    let py = unsafe { fixture_execution.__token_unchecked() };
    let _r = unsafe { molt_runtime::molt_len(py.attached_context(), int(42)) };
    // Exception should be pending; value is sentinel
}
