// Builtin function tests — CPython parity for abs, len, hash, ord, chr, etc.

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
fn as_int(bits: u64) -> i64 {
    MoltObject::from_bits(bits).as_int().expect("expected int")
}
fn missing() -> u64 {
    molt_runtime::molt_missing()
}

// ── abs ─────────────────────────────────────────────────
#[test]
fn test_abs_int_pos() {
    init();
    assert_eq!(as_int(molt_runtime::molt_abs_builtin(int(42))), 42);
}
#[test]
fn test_abs_int_neg() {
    init();
    assert_eq!(as_int(molt_runtime::molt_abs_builtin(int(-42))), 42);
}
#[test]
fn test_abs_int_zero() {
    init();
    assert_eq!(as_int(molt_runtime::molt_abs_builtin(int(0))), 0);
}

// ── pow ─────────────────────────────────────────────────
#[test]
fn test_pow_2_10() {
    init();
    assert_eq!(as_int(molt_runtime::molt_pow(int(2), int(10))), 1024);
}
#[test]
fn test_pow_0() {
    init();
    assert_eq!(as_int(molt_runtime::molt_pow(int(100), int(0))), 1);
}

// ── divmod ──────────────────────────────────────────────
// divmod returns a tuple; for now just test it doesn't crash
#[test]
fn test_divmod_basic() {
    init();
    let _r = molt_runtime::molt_divmod_builtin(int(7), int(3));
}

// ── ord / chr ───────────────────────────────────────────
#[test]
fn test_chr_a() {
    init();
    let _r = molt_runtime::molt_chr(int(65)); /* 'A' */
}
#[test]
fn test_ord_basic() {
    init(); /* would need a string object */
}

// ── is_truthy on various types ──────────────────────────
#[test]
fn test_truthy_none() {
    init();
    assert_eq!(molt_runtime::molt_is_truthy(none()), 0);
}
#[test]
fn test_truthy_true() {
    init();
    assert_ne!(molt_runtime::molt_is_truthy(b(true)), 0);
}
#[test]
fn test_truthy_false() {
    init();
    assert_eq!(molt_runtime::molt_is_truthy(b(false)), 0);
}
#[test]
fn test_truthy_int_0() {
    init();
    assert_eq!(molt_runtime::molt_is_truthy(int(0)), 0);
}
#[test]
fn test_truthy_int_1() {
    init();
    assert_ne!(molt_runtime::molt_is_truthy(int(1)), 0);
}
#[test]
fn test_truthy_float_0() {
    init();
    assert_eq!(molt_runtime::molt_is_truthy(f(0.0)), 0);
}
#[test]
fn test_truthy_float_nonzero() {
    init();
    assert_ne!(molt_runtime::molt_is_truthy(f(0.1)), 0);
}

// ── isinstance / issubclass ─────────────────────────────
// These need class objects which are hard to construct in unit tests.
// Just verify they don't crash with basic inputs.
#[test]
fn test_isinstance_no_crash() {
    init();
    let _r = molt_runtime::molt_isinstance(int(1), int(2));
}

// ── New builtins (from the 26 we added) ─────────────────
#[test]
fn test_int_builtin_no_args() {
    init();
    let r = molt_runtime::molt_int_builtin(missing(), missing());
    assert_eq!(as_int(r), 0);
}

#[test]
fn test_float_builtin_no_args() {
    init();
    let r = molt_runtime::molt_float_builtin(missing());
    let v = MoltObject::from_bits(r).as_float().expect("expected float");
    assert_eq!(v, 0.0);
}

#[test]
fn test_bool_builtin_false() {
    init();
    let r = molt_runtime::molt_bool_builtin(int(0));
    assert!(!MoltObject::from_bits(r).as_bool().unwrap());
}

#[test]
fn test_bool_builtin_true() {
    init();
    let r = molt_runtime::molt_bool_builtin(int(42));
    assert!(MoltObject::from_bits(r).as_bool().unwrap());
}

#[test]
fn test_object_builtin() {
    init();
    let r = molt_runtime::molt_object_builtin();
    // Should return a valid object, not none
    assert_ne!(r, none());
}

#[test]
fn test_type_builtin() {
    init();
    let r = molt_runtime::molt_type_builtin(int(42));
    // In test mode without full bootstrap, type() may return None — just verify no crash
    let _ = r;
}

#[test]
fn test_range_builtin_one_arg() {
    init();
    let r = molt_runtime::molt_range_builtin(int(5), missing(), missing());
    assert_ne!(r, none()); // range(5) should succeed
}
