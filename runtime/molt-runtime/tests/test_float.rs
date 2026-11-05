// Float arithmetic tests — CPython parity for float operations.

use molt_obj_model::MoltObject;
use std::f64::consts::PI;
use std::sync::Once;

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/test_float"
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
fn as_bool(bits: u64) -> bool {
    MoltObject::from_bits(bits).as_bool().unwrap_or(false)
}

#[test]
fn test_add() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_add(py.attached_context(), f(1.5), f(2.5)) }),
        4.0
    );
}
#[test]
fn test_sub() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_sub(py.attached_context(), f(5.0), f(1.5)) }),
        3.5
    );
}
#[test]
fn test_mul() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_mul(py.attached_context(), f(2.5), f(4.0)) }),
        10.0
    );
}
#[test]
fn test_div() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_div(py.attached_context(), f(7.0), f(2.0)) }),
        3.5
    );
}
#[test]
fn test_floordiv() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_floordiv(py.attached_context(), f(7.0), f(2.0)) }),
        3.0
    );
}
#[test]
fn test_mod() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_mod(py.attached_context(), f(7.5), f(2.0)) }),
        1.5
    );
}
#[test]
fn test_pow() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_pow(py.attached_context(), f(2.0), f(3.0)) }),
        8.0
    );
}
#[test]
fn test_neg_floordiv() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_floordiv(py.attached_context(), f(-7.0), f(2.0)) }),
        -4.0
    );
}
#[test]
fn test_int_div_returns_float() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_div(py.attached_context(), int(7), int(2)) }),
        3.5
    );
}
#[test]
fn test_lt() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_lt(py.attached_context(), f(1.0), f(2.0))
    }));
}
#[test]
fn test_eq() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_eq(py.attached_context(), f(PI), f(PI))
    }));
}
#[test]
fn test_ne() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert!(as_bool(unsafe {
        molt_runtime::molt_ne(py.attached_context(), f(1.0), f(2.0))
    }));
}
#[test]
fn test_abs_neg() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_abs_builtin(py.attached_context(), f(-PI)) }),
        PI
    );
}
#[test]
fn test_abs_pos() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_abs_builtin(py.attached_context(), f(2.7)) }),
        2.7
    );
}
#[test]
fn test_truthy_zero() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), f(0.0)) },
        0
    );
}
#[test]
fn test_truthy_nonzero() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_ne!(
        unsafe { molt_runtime::molt_is_truthy(py.attached_context(), f(1.0)) },
        0
    );
}
#[test]
fn test_add_int_float() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_add(py.attached_context(), int(1), f(0.5)) }),
        1.5
    );
}
#[test]
fn test_float_mul_int() {
    let fixture_execution = init();
    let py = unsafe { fixture_execution.__token_unchecked() };
    assert_eq!(
        as_f(unsafe { molt_runtime::molt_mul(py.attached_context(), f(2.5), int(4)) }),
        10.0
    );
}
