// Float arithmetic tests — CPython parity for float operations.

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

fn f(v: f64) -> u64 {
    MoltObject::from_float(v).bits()
}
fn int(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
}
fn as_f(bits: u64) -> f64 {
    MoltObject::from_bits(bits)
        .as_float()
        .expect("expected float")
}
fn as_i(bits: u64) -> i64 {
    MoltObject::from_bits(bits).as_int().expect("expected int")
}
fn as_bool(bits: u64) -> bool {
    MoltObject::from_bits(bits).as_bool().unwrap_or(false)
}

#[test]
fn test_add() {
    init();
    assert_eq!(as_f(molt_runtime::molt_add(f(1.5), f(2.5))), 4.0);
}
#[test]
fn test_sub() {
    init();
    assert_eq!(as_f(molt_runtime::molt_sub(f(5.0), f(1.5))), 3.5);
}
#[test]
fn test_mul() {
    init();
    assert_eq!(as_f(molt_runtime::molt_mul(f(2.5), f(4.0))), 10.0);
}
#[test]
fn test_div() {
    init();
    assert_eq!(as_f(molt_runtime::molt_div(f(7.0), f(2.0))), 3.5);
}
#[test]
fn test_floordiv() {
    init();
    assert_eq!(as_f(molt_runtime::molt_floordiv(f(7.0), f(2.0))), 3.0);
}
#[test]
fn test_mod() {
    init();
    assert_eq!(as_f(molt_runtime::molt_mod(f(7.5), f(2.0))), 1.5);
}
#[test]
fn test_pow() {
    init();
    assert_eq!(as_f(molt_runtime::molt_pow(f(2.0), f(3.0))), 8.0);
}
#[test]
fn test_neg_floordiv() {
    init();
    assert_eq!(as_f(molt_runtime::molt_floordiv(f(-7.0), f(2.0))), -4.0);
}
#[test]
fn test_int_div_returns_float() {
    init();
    assert_eq!(as_f(molt_runtime::molt_div(int(7), int(2))), 3.5);
}
#[test]
fn test_lt() {
    init();
    assert!(as_bool(molt_runtime::molt_lt(f(1.0), f(2.0))));
}
#[test]
fn test_eq() {
    init();
    assert!(as_bool(molt_runtime::molt_eq(f(3.14), f(3.14))));
}
#[test]
fn test_ne() {
    init();
    assert!(as_bool(molt_runtime::molt_ne(f(1.0), f(2.0))));
}
#[test]
fn test_abs_neg() {
    init();
    assert_eq!(as_f(molt_runtime::molt_abs_builtin(f(-3.14))), 3.14);
}
#[test]
fn test_abs_pos() {
    init();
    assert_eq!(as_f(molt_runtime::molt_abs_builtin(f(2.7))), 2.7);
}
#[test]
fn test_truthy_zero() {
    init();
    assert_eq!(molt_runtime::molt_is_truthy(f(0.0)), 0);
}
#[test]
fn test_truthy_nonzero() {
    init();
    assert_ne!(molt_runtime::molt_is_truthy(f(1.0)), 0);
}
#[test]
fn test_add_int_float() {
    init();
    assert_eq!(as_f(molt_runtime::molt_add(int(1), f(0.5))), 1.5);
}
#[test]
fn test_float_mul_int() {
    init();
    assert_eq!(as_f(molt_runtime::molt_mul(f(2.5), int(4))), 10.0);
}
