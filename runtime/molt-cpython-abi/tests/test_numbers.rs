//! Tests for PyLong_*, PyFloat_*, PyBool_*, PyNumber_Check and type checks.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    Py_False, Py_None, Py_NotImplementedSentinel, Py_True, PyNumberMethods, PyObject, PyTypeObject,
};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::OwnedHandleResult;
use molt_lang_obj_model::MoltObject;
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr;

#[derive(Clone, Copy)]
enum NumericHookOutcome {
    Value,
    Missing,
    Error,
}

thread_local! {
    static DENY_NUMERIC_BYTE_IMPORT: Cell<bool> = const { Cell::new(false) };
    static NUMERIC_BYTE_RUNTIME_FAILURE: Cell<Option<u64>> = const { Cell::new(None) };
    static INDEX_RESULT_FINALIZERS: Cell<usize> = const { Cell::new(0) };
    static NUMERIC_BYTE_IMPORTS: Cell<usize> = const { Cell::new(0) };
    static NUMERIC_ADOPTIONS: Cell<usize> = const { Cell::new(0) };
    static DENY_NUMERIC_ADOPTION: Cell<bool> = const { Cell::new(false) };
    static NUMERIC_RUNTIME_PENDING: Cell<u64> = const { Cell::new(0) };
    static NUMERIC_UNARY_OUTCOME: Cell<Option<(NumericHookOutcome, u64)>> = const { Cell::new(None) };
    static NUMERIC_UNARY_OPERATION: Cell<Option<u32>> = const { Cell::new(None) };
    static SUBTYPE_CALLBACK_ERROR: Cell<usize> = const { Cell::new(0) };
    static SUBTYPE_RUNTIME_FAILURE: Cell<bool> = const { Cell::new(false) };
    static NUMERIC_CLASS_CALLS: Cell<usize> = const { Cell::new(0) };
    static NUMERIC_SUBTYPE_CALLS: Cell<usize> = const { Cell::new(0) };
    static CLASS_CALLBACK_ERROR: Cell<Option<(u64, usize)>> = const { Cell::new(None) };
    static PROTOCOL_CLASS: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
    static TRUNC_CALLS: Cell<usize> = const { Cell::new(0) };
    static TRUNC_BAD_RESULT: Cell<bool> = const { Cell::new(false) };
    static SLOT_RESULT: Cell<usize> = const { Cell::new(0) };
    static TARGET_MINOR: Cell<i64> = const { Cell::new(12) };
    static LONG_OVERRIDE_CALLS: Cell<usize> = const { Cell::new(0) };
    static FLOAT_OVERRIDE_CALLS: Cell<usize> = const { Cell::new(0) };
    static COMPLEX_OVERRIDE_CALLS: Cell<usize> = const { Cell::new(0) };
    static INDEX_OVERRIDE_CALLS: Cell<usize> = const { Cell::new(0) };
    static NUMBER_CALL: Cell<Option<(u32, u32, u64, u64)>> = const { Cell::new(None) };
    static POWER_MODE: Cell<Option<u32>> = const { Cell::new(None) };
    static POWER_MODULUS_BITS: Cell<Option<u64>> = const { Cell::new(None) };
    static POWER_HOOK_FAILS: Cell<bool> = const { Cell::new(false) };
    static FOREIGN_POWER_CALLS: Cell<u32> = const { Cell::new(0) };
    static FOREIGN_INPLACE_POWER_CALLS: Cell<u32> = const { Cell::new(0) };
    static FOREIGN_INPLACE_NOT_IMPLEMENTED: Cell<bool> = const { Cell::new(false) };
    static FOREIGN_POWER_MODULUS: Cell<usize> = const { Cell::new(usize::MAX) };
    static FOREIGN_POWER_BASE: Cell<usize> = const { Cell::new(0) };
    static FOREIGN_POWER_EXPONENT: Cell<usize> = const { Cell::new(0) };
    static FOREIGN_BINARY_CALLS: Cell<u32> = const { Cell::new(0) };
    static FOREIGN_MATRIX_CALLS: Cell<u32> = const { Cell::new(0) };
    static FOREIGN_INPLACE_MATRIX_CALLS: Cell<u32> = const { Cell::new(0) };
}

unsafe extern "C" fn capture_number_power(
    mode: u32,
    _base_bits: u64,
    _exponent_bits: u64,
    modulus_bits: u64,
) -> OwnedHandleResult {
    POWER_MODE.with(|value| value.set(Some(mode)));
    POWER_MODULUS_BITS.with(|value| value.set(Some(modulus_bits)));
    if POWER_HOOK_FAILS.with(Cell::get) {
        OwnedHandleResult::error()
    } else {
        OwnedHandleResult::ok(MoltObject::from_int(1).bits())
    }
}

unsafe extern "C" fn capture_binary(
    op: u32,
    mode: u32,
    left: u64,
    right: u64,
) -> OwnedHandleResult {
    NUMBER_CALL.with(|value| value.set(Some((op, mode, left, right))));
    OwnedHandleResult::ok(MoltObject::from_int(if mode == 0 { 23 } else { 71 }).bits())
}

unsafe extern "C" fn foreign_power_slot(
    _base: *mut PyObject,
    _exponent: *mut PyObject,
    modulus: *mut PyObject,
) -> *mut PyObject {
    FOREIGN_POWER_CALLS.with(|calls| calls.set(calls.get() + 1));
    FOREIGN_POWER_BASE.with(|value| value.set(_base as usize));
    FOREIGN_POWER_EXPONENT.with(|value| value.set(_exponent as usize));
    FOREIGN_POWER_MODULUS.with(|value| value.set(modulus as usize));
    unsafe { molt_cpython_abi::api::object::Py_NewRef(&raw mut Py_None) }
}

unsafe extern "C" fn foreign_inplace_power_slot(
    _base: *mut PyObject,
    _exponent: *mut PyObject,
    modulus: *mut PyObject,
) -> *mut PyObject {
    FOREIGN_INPLACE_POWER_CALLS.with(|calls| calls.set(calls.get() + 1));
    FOREIGN_POWER_BASE.with(|value| value.set(_base as usize));
    FOREIGN_POWER_EXPONENT.with(|value| value.set(_exponent as usize));
    FOREIGN_POWER_MODULUS.with(|value| value.set(modulus as usize));
    if FOREIGN_INPLACE_NOT_IMPLEMENTED.with(Cell::get) {
        unsafe { molt_cpython_abi::api::object::Py_NewRef(&raw mut Py_NotImplementedSentinel) }
    } else {
        unsafe { molt_cpython_abi::api::object::Py_NewRef(&raw mut Py_None) }
    }
}

unsafe extern "C" fn foreign_binary_slot(
    _left: *mut PyObject,
    _right: *mut PyObject,
) -> *mut PyObject {
    FOREIGN_BINARY_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { molt_cpython_abi::api::object::Py_NewRef(&raw mut Py_None) }
}

unsafe extern "C" fn foreign_matrix_slot(
    _left: *mut PyObject,
    _right: *mut PyObject,
) -> *mut PyObject {
    FOREIGN_MATRIX_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { molt_cpython_abi::api::object::Py_NewRef(&raw mut Py_None) }
}

unsafe extern "C" fn foreign_inplace_matrix_slot(
    _left: *mut PyObject,
    _right: *mut PyObject,
) -> *mut PyObject {
    FOREIGN_INPLACE_MATRIX_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { molt_cpython_abi::api::object::Py_NewRef(&raw mut Py_None) }
}

unsafe extern "C" fn counted_numeric_identity_new(bits: u64) -> OwnedHandleResult {
    NUMERIC_ADOPTIONS.with(|calls| calls.set(calls.get() + 1));
    if DENY_NUMERIC_ADOPTION.with(Cell::get) {
        OwnedHandleResult::error()
    } else {
        unsafe { support::fake_runtime::numeric_identity_new(bits) }
    }
}

unsafe extern "C" fn counted_int_from_bytes(
    data: *const u8,
    len: usize,
    little: i32,
    signed: i32,
) -> u64 {
    NUMERIC_BYTE_IMPORTS.with(|calls| calls.set(calls.get() + 1));
    if let Some(pending) = NUMERIC_BYTE_RUNTIME_FAILURE.with(Cell::get) {
        NUMERIC_RUNTIME_PENDING.with(|value| value.set(pending));
        return 0;
    }
    if DENY_NUMERIC_BYTE_IMPORT.with(Cell::get) {
        unsafe { molt_cpython_abi::api::errors::PyErr_NoMemory() };
        return 0;
    }
    unsafe { support::fake_runtime::int_from_bytes(data, len, little, signed) }
}

// Reuse the pending/clear/preserve hook contract from the import-error
// fixture. The class remains observable while instance transfer is unavailable;
// that is distinct from a failed cold Type-view projection.
unsafe extern "C" fn numeric_exception_pending() -> i32 {
    i32::from(NUMERIC_RUNTIME_PENDING.with(Cell::get) != 0)
}
unsafe extern "C" fn numeric_pending_class() -> molt_cpython_abi::hooks::PendingExceptionClass {
    if NUMERIC_RUNTIME_PENDING.with(Cell::get) == 0 {
        molt_cpython_abi::hooks::PendingExceptionClass::None
    } else {
        molt_cpython_abi::hooks::PendingExceptionClass::NativeClass(
            &raw mut molt_cpython_abi::abi_types::PyExc_LookupError,
        )
    }
}
unsafe extern "C" fn numeric_clear_pending() {
    NUMERIC_RUNTIME_PENDING.with(|value| value.set(0));
}
unsafe extern "C" fn numeric_preserve_pending(
    callback: unsafe extern "C" fn(*mut c_void),
    context: *mut c_void,
) {
    let incoming = NUMERIC_RUNTIME_PENDING.with(|value| value.replace(0));
    unsafe { callback(context) };
    NUMERIC_RUNTIME_PENDING.with(|value| value.set(incoming));
}
unsafe extern "C" fn numeric_type_is_subtype(subclass: u64, class: u64) -> i32 {
    NUMERIC_SUBTYPE_CALLS.with(|value| value.set(value.get() + 1));
    let error = SUBTYPE_CALLBACK_ERROR.with(Cell::get) as *mut PyObject;
    if !error.is_null() {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetRaisedException(
                molt_cpython_abi::api::object::Py_NewRef(error),
            )
        };
        return 0;
    }
    if SUBTYPE_RUNTIME_FAILURE.with(Cell::get) {
        NUMERIC_RUNTIME_PENDING.with(|value| value.set(0x1234_5678));
        return 0;
    }
    unsafe { support::fake_runtime::type_is_subtype(subclass, class) }
}

unsafe extern "C" fn protocol_runtime_class(
    bits: u64,
) -> molt_cpython_abi::hooks::BorrowedHandleResult {
    NUMERIC_CLASS_CALLS.with(|value| value.set(value.get() + 1));
    if let Some((object, error)) = CLASS_CALLBACK_ERROR.with(Cell::get) {
        if object == bits {
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetRaisedException(
                    molt_cpython_abi::api::object::Py_NewRef(error as *mut PyObject),
                )
            };
            return molt_cpython_abi::hooks::BorrowedHandleResult::error();
        }
    }
    if let Some((object, class)) = PROTOCOL_CLASS.with(Cell::get) {
        if object == bits {
            return molt_cpython_abi::hooks::BorrowedHandleResult::ok(class);
        }
    }
    unsafe { support::fake_runtime::runtime_class_borrowed(bits) }
}

unsafe extern "C" fn trunc_call(
    _object: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    TRUNC_CALLS.with(|calls| calls.set(calls.get() + 1));
    if TRUNC_BAD_RESULT.with(Cell::get) {
        unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5) }
    } else {
        unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(41) }
    }
}

unsafe extern "C" fn borrowed_call_result(
    _object: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        molt_cpython_abi::api::object::Py_NewRef(SLOT_RESULT.with(Cell::get) as *mut PyObject)
    }
}

unsafe extern "C" fn foreign_float_override(_object: *mut PyObject) -> *mut PyObject {
    FLOAT_OVERRIDE_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(42.5) }
}

unsafe extern "C" fn borrowed_slot_result(_object: *mut PyObject) -> *mut PyObject {
    unsafe {
        molt_cpython_abi::api::object::Py_NewRef(SLOT_RESULT.with(Cell::get) as *mut PyObject)
    }
}

unsafe extern "C" fn numeric_target_minor() -> i64 {
    TARGET_MINOR.with(Cell::get)
}

unsafe extern "C" fn protocol_unary(operation: u32, bits: u64) -> OwnedHandleResult {
    use molt_cpython_abi::hooks::NumberUnaryOp;
    if let Some((outcome, pending)) = NUMERIC_UNARY_OUTCOME.with(Cell::get) {
        NUMERIC_UNARY_OPERATION.with(|value| value.set(Some(operation)));
        NUMERIC_RUNTIME_PENDING.with(|value| value.set(pending));
        return match outcome {
            NumericHookOutcome::Error => OwnedHandleResult::error(),
            NumericHookOutcome::Missing => OwnedHandleResult::missing(),
            NumericHookOutcome::Value => OwnedHandleResult::ok(
                if operation == NumberUnaryOp::Float as u32 {
                    MoltObject::from_float(42.5)
                } else {
                    MoltObject::from_int(42)
                }
                .bits(),
            ),
        };
    }
    if PROTOCOL_CLASS
        .with(Cell::get)
        .is_some_and(|(object, _)| object == bits)
    {
        if operation == NumberUnaryOp::FloatAsDouble as u32
            || operation == NumberUnaryOp::Float as u32
        {
            FLOAT_OVERRIDE_CALLS.with(|calls| calls.set(calls.get() + 1));
            return OwnedHandleResult::ok(MoltObject::from_float(42.5).bits());
        }
        if operation == NumberUnaryOp::Long as u32 {
            LONG_OVERRIDE_CALLS.with(|calls| calls.set(calls.get() + 1));
            return OwnedHandleResult::ok(MoltObject::from_int(777).bits());
        }
    }
    unsafe { support::fake_numbers::unary(operation, bits) }
}

unsafe extern "C" fn complex_override(
    _callable: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    COMPLEX_OVERRIDE_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { molt_cpython_abi::api::numbers::PyComplex_FromDoubles(12.5, -3.0) }
}

unsafe extern "C" fn forbidden_index_override(_object: *mut PyObject) -> *mut PyObject {
    INDEX_OVERRIDE_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) }
}

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    support::fake_runtime::wire_sequences(&mut hooks);
    hooks.object_hash = support::fake_complex::hash;
    hooks.complex_from_doubles = support::fake_complex::from_doubles;
    hooks.complex_parts = support::fake_complex::parts;
    hooks.number_power = capture_number_power;
    hooks.number_binary_op = capture_binary;
    hooks.number_unary_op = protocol_unary;
    hooks.target_python_minor = numeric_target_minor;
    hooks.import_module = support::warnings::import_module;
    hooks.numeric_identity_new = Some(counted_numeric_identity_new);
    hooks.int_from_bytes = counted_int_from_bytes;
    hooks.runtime_class_borrowed = Some(protocol_runtime_class);
    hooks.type_is_subtype = numeric_type_is_subtype;
    hooks.exception_pending = numeric_exception_pending;
    hooks.pending_exception_class = numeric_pending_class;
    hooks.clear_pending_exception = numeric_clear_pending;
    hooks.with_preserved_pending_exception = numeric_preserve_pending;
    support::prepare_abi_test_thread(hooks);
    support::fake_runtime::prepare_class_bindings();
    NUMERIC_ADOPTIONS.with(|calls| calls.set(0));
    NUMERIC_BYTE_IMPORTS.with(|calls| calls.set(0));
    DENY_NUMERIC_ADOPTION.with(|deny| deny.set(false));
    PROTOCOL_CLASS.with(|class| class.set(None));
    INDEX_OVERRIDE_CALLS.with(|calls| calls.set(0));
    FLOAT_OVERRIDE_CALLS.with(|calls| calls.set(0));
    LONG_OVERRIDE_CALLS.with(|calls| calls.set(0));
    TARGET_MINOR.with(|minor| minor.set(12));
    COMPLEX_OVERRIDE_CALLS.with(|calls| calls.set(0));
    POWER_MODULUS_BITS.with(|value| value.set(None));
    POWER_HOOK_FAILS.with(|value| value.set(false));
    FOREIGN_POWER_CALLS.with(|calls| calls.set(0));
    FOREIGN_INPLACE_POWER_CALLS.with(|calls| calls.set(0));
    FOREIGN_INPLACE_NOT_IMPLEMENTED.with(|value| value.set(false));
    FOREIGN_POWER_MODULUS.with(|value| value.set(usize::MAX));
    FOREIGN_POWER_BASE.with(|value| value.set(0));
    FOREIGN_POWER_EXPONENT.with(|value| value.set(0));
    FOREIGN_BINARY_CALLS.with(|calls| calls.set(0));
    FOREIGN_MATRIX_CALLS.with(|calls| calls.set(0));
    FOREIGN_INPLACE_MATRIX_CALLS.with(|calls| calls.set(0));
}

// ---------------------------------------------------------------------------
// PyNumber_Power
// ---------------------------------------------------------------------------

#[test]
fn test_pynumber_power_preserves_modulus_presence_and_value_bits() {
    init();
    let base = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let exponent = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(3) };
    let positive_float_zero = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(0.0) };
    let negative_float_zero = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(-0.0) };
    let integer_zero = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(0) };
    let cases = [
        (
            "omitted modulus",
            ptr::null_mut(),
            MoltObject::none().bits(),
        ),
        ("explicit None", &raw mut Py_None, MoltObject::none().bits()),
        (
            "float +0.0",
            positive_float_zero,
            MoltObject::from_float(0.0).bits(),
        ),
        (
            "float -0.0",
            negative_float_zero,
            MoltObject::from_float(-0.0).bits(),
        ),
        ("integer zero", integer_zero, MoltObject::from_int(0).bits()),
    ];

    for (label, modulus, expected_bits) in cases {
        POWER_MODULUS_BITS.with(|value| value.set(None));
        let result = unsafe {
            molt_cpython_abi::api::abstract_number::PyNumber_Power(base, exponent, modulus)
        };
        assert!(!result.is_null(), "{label} must reach the runtime hook");
        assert_eq!(POWER_MODE.with(Cell::get), Some(0));
        assert_eq!(
            POWER_MODULUS_BITS.with(Cell::get),
            Some(expected_bits),
            "{label} lost its modulus representation"
        );
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result) };

        POWER_MODULUS_BITS.with(|value| value.set(None));
        let result = unsafe {
            molt_cpython_abi::api::abstract_number::PyNumber_InPlacePower(base, exponent, modulus)
        };
        assert!(
            !result.is_null(),
            "in-place {label} must reach the runtime hook"
        );
        assert_eq!(
            POWER_MODULUS_BITS.with(Cell::get),
            Some(expected_bits),
            "in-place {label} lost its modulus representation"
        );
        assert_eq!(POWER_MODE.with(Cell::get), Some(1));
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result) };
    }

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(base);
        molt_cpython_abi::api::refcount::Py_DECREF(exponent);
        molt_cpython_abi::api::refcount::Py_DECREF(positive_float_zero);
        molt_cpython_abi::api::refcount::Py_DECREF(negative_float_zero);
        molt_cpython_abi::api::refcount::Py_DECREF(integer_zero);
    }
}

#[test]
fn test_pynumber_power_hook_failure_returns_null_with_exception() {
    init();
    let base = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let exponent = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(3) };
    POWER_HOOK_FAILS.with(|value| value.set(true));

    let result = unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_Power(base, exponent, ptr::null_mut())
    };
    assert!(result.is_null());
    assert_eq!(
        POWER_MODULUS_BITS.with(Cell::get),
        Some(MoltObject::none().bits())
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());

    POWER_HOOK_FAILS.with(|value| value.set(false));
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(base);
        molt_cpython_abi::api::refcount::Py_DECREF(exponent);
    }
}

#[test]
fn test_pynumber_power_foreign_dispatch_uses_normal_and_inplace_slots() {
    init();
    let mut methods: Box<PyNumberMethods> = Box::new(unsafe { std::mem::zeroed() });
    methods.nb_power = foreign_power_slot as *mut c_void;
    methods.nb_inplace_power = foreign_inplace_power_slot as *mut c_void;
    methods.nb_matrix_multiply = foreign_matrix_slot as *mut c_void;
    methods.nb_inplace_matrix_multiply = foreign_inplace_matrix_slot as *mut c_void;
    let methods = Box::into_raw(methods);

    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.tp_name = c"PowerProbe".as_ptr();
    ty.tp_as_number = methods.cast();
    let ty = Box::into_raw(ty);
    let base = Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ty,
    }));
    let exponent = Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ty,
    }));

    let normal = unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_Power(base, exponent, ptr::null_mut())
    };
    assert!(std::ptr::eq(normal, &raw mut Py_None));
    assert_eq!(FOREIGN_POWER_CALLS.with(Cell::get), 1);
    assert_eq!(FOREIGN_INPLACE_POWER_CALLS.with(Cell::get), 0);
    assert_eq!(
        FOREIGN_POWER_MODULUS.with(Cell::get),
        (&raw mut Py_None) as usize
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(normal) };

    let inplace = unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_InPlacePower(
            base,
            exponent,
            &raw mut Py_None,
        )
    };
    assert!(std::ptr::eq(inplace, &raw mut Py_None));
    assert_eq!(FOREIGN_POWER_CALLS.with(Cell::get), 1);
    assert_eq!(FOREIGN_INPLACE_POWER_CALLS.with(Cell::get), 1);
    assert_eq!(
        FOREIGN_POWER_MODULUS.with(Cell::get),
        (&raw mut Py_None) as usize
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(inplace) };

    FOREIGN_INPLACE_NOT_IMPLEMENTED.with(|value| value.set(true));
    let fallback = unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_InPlacePower(
            base,
            exponent,
            ptr::null_mut(),
        )
    };
    assert!(std::ptr::eq(fallback, &raw mut Py_None));
    assert_eq!(FOREIGN_POWER_CALLS.with(Cell::get), 2);
    assert_eq!(FOREIGN_INPLACE_POWER_CALLS.with(Cell::get), 2);
    assert_eq!(
        FOREIGN_POWER_MODULUS.with(Cell::get),
        (&raw mut Py_None) as usize
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(fallback) };

    let aliased =
        unsafe { molt_cpython_abi::api::abstract_number::PyNumber_Power(base, base, base) };
    assert!(std::ptr::eq(aliased, &raw mut Py_None));
    assert_eq!(FOREIGN_POWER_CALLS.with(Cell::get), 3);
    assert_eq!(FOREIGN_POWER_BASE.with(Cell::get), base as usize);
    assert_eq!(FOREIGN_POWER_EXPONENT.with(Cell::get), base as usize);
    assert_eq!(FOREIGN_POWER_MODULUS.with(Cell::get), base as usize);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(aliased) };

    let matrix =
        unsafe { molt_cpython_abi::api::abstract_number::PyNumber_MatrixMultiply(base, exponent) };
    assert!(std::ptr::eq(matrix, &raw mut Py_None));
    assert_eq!(FOREIGN_MATRIX_CALLS.with(Cell::get), 1);
    assert_eq!(FOREIGN_INPLACE_MATRIX_CALLS.with(Cell::get), 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(matrix) };

    let inplace_matrix = unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_InPlaceMatrixMultiply(base, exponent)
    };
    assert!(std::ptr::eq(inplace_matrix, &raw mut Py_None));
    assert_eq!(FOREIGN_MATRIX_CALLS.with(Cell::get), 1);
    assert_eq!(FOREIGN_INPLACE_MATRIX_CALLS.with(Cell::get), 1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(inplace_matrix);
        drop(Box::from_raw(base));
        drop(Box::from_raw(exponent));
        drop(Box::from_raw(ty));
        drop(Box::from_raw(methods));
    }
}

#[test]
fn test_failed_managed_projection_never_reaches_foreign_numeric_slots() {
    fn assert_commit_failure(result: *mut PyObject) {
        assert!(result.is_null());
        assert!(std::ptr::eq(
            unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() },
            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>()
        ));
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    }

    init();
    let list = unsafe { molt_cpython_abi::api::sequences::PyList_New(1) };
    assert!(!list.is_null());

    let mut methods: Box<PyNumberMethods> = Box::new(unsafe { std::mem::zeroed() });
    methods.nb_add = foreign_binary_slot as *mut c_void;
    methods.nb_power = foreign_power_slot as *mut c_void;
    methods.nb_inplace_power = foreign_inplace_power_slot as *mut c_void;
    methods.nb_matrix_multiply = foreign_matrix_slot as *mut c_void;
    methods.nb_inplace_matrix_multiply = foreign_inplace_matrix_slot as *mut c_void;
    let methods = Box::into_raw(methods);
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.tp_name = c"ForeignNumericProbe".as_ptr();
    ty.tp_as_number = methods.cast();
    let ty = Box::into_raw(ty);
    let foreign = Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ty,
    }));

    assert_commit_failure(unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_Add(list, foreign)
    });
    assert_commit_failure(unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_InPlaceAdd(list, foreign)
    });
    assert_commit_failure(unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_Power(list, foreign, ptr::null_mut())
    });
    assert_commit_failure(unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_InPlacePower(
            list,
            foreign,
            ptr::null_mut(),
        )
    });
    assert_commit_failure(unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_MatrixMultiply(list, foreign)
    });
    assert_commit_failure(unsafe {
        molt_cpython_abi::api::abstract_number::PyNumber_InPlaceMatrixMultiply(list, foreign)
    });
    assert_commit_failure(unsafe { molt_cpython_abi::api::abstract_number::PyNumber_Long(list) });
    assert_commit_failure(unsafe { molt_cpython_abi::api::abstract_number::PyNumber_Float(list) });
    assert_commit_failure(unsafe { molt_cpython_abi::api::abstract_number::PyNumber_Index(list) });

    assert_eq!(FOREIGN_BINARY_CALLS.with(Cell::get), 0);
    assert_eq!(FOREIGN_POWER_CALLS.with(Cell::get), 0);
    assert_eq!(FOREIGN_INPLACE_POWER_CALLS.with(Cell::get), 0);
    assert_eq!(FOREIGN_MATRIX_CALLS.with(Cell::get), 0);
    assert_eq!(FOREIGN_INPLACE_MATRIX_CALLS.with(Cell::get), 0);

    let list_bits = GLOBAL_BRIDGE.managed_handle_for_pyobj(list).unwrap();
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(list);
        drop(Box::from_raw(foreign));
        drop(Box::from_raw(ty));
        drop(Box::from_raw(methods));
    }
    assert!(
        !support::fake_runtime::contains(list_bits),
        "unreadied list owner must retire"
    );
}

// ---------------------------------------------------------------------------
// PyLong
// ---------------------------------------------------------------------------

#[test]
fn test_pylong_from_long_returns_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_roundtrip_positive() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(12345) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py) };
    assert_eq!(val, 12345);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_roundtrip_negative() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-999) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py) };
    assert_eq!(val, -999);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_roundtrip_zero() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(0) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py) };
    assert_eq!(val, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_aslong_null_returns_minus_one() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(ptr::null_mut()) };
    assert_eq!(val, -1);
}

#[test]
fn test_pylong_aslonglong_and_overflow_reports_inline_value() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(12345) };
    let mut overflow = 99;
    let val =
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLongLongAndOverflow(py, &mut overflow) };
    assert_eq!(val, 12345);
    assert_eq!(overflow, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_aslonglong_and_overflow_null_sets_error() {
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut overflow = 99;
    let val = unsafe {
        molt_cpython_abi::api::numbers::PyLong_AsLongLongAndOverflow(ptr::null_mut(), &mut overflow)
    };
    assert_eq!(val, -1);
    assert_eq!(overflow, 0);
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_pylong_from_ssize_t() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromSsize_t(77) };
    assert!(!py.is_null());
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsSsize_t(py) };
    assert_eq!(val, 77);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_from_size_t_and_number_index() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromSize_t(55) };
    assert!(!py.is_null());
    let indexed = unsafe { molt_cpython_abi::api::abstract_number::PyNumber_Index(py) };
    assert!(std::ptr::eq(indexed, py));
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLong(indexed) },
        55
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(indexed);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_physical_integer_admission_never_requests_runtime_identity() {
    init();
    DENY_NUMERIC_ADOPTION.with(|deny| deny.set(true));
    unsafe {
        for value in [i64::MIN, -257, 257, i64::MAX] {
            let object = molt_cpython_abi::api::numbers::PyLong_FromLongLong(value);
            assert!(!object.is_null());
            assert!(
                GLOBAL_BRIDGE.molt_handle_for_pyobj(object).is_none()
                    || MoltObject::try_from_int(value).is_some()
            );
            let refs = (*object).ob_refcnt;
            assert_eq!(
                molt_cpython_abi::api::abstract_number::PyIndex_Check(object),
                1
            );
            assert_eq!(molt_cpython_abi::api::numbers::PyNumber_Check(object), 1);
            let indexed = molt_cpython_abi::api::abstract_number::PyNumber_Index(object);
            assert_eq!(indexed, object);
            assert_eq!((*object).ob_refcnt, refs + 1);
            assert_eq!(
                molt_cpython_abi::api::numbers::PyLong_AsLongLong(indexed),
                value
            );
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::refcount::Py_DECREF(indexed);
            assert_eq!((*object).ob_refcnt, refs);
            molt_cpython_abi::api::refcount::Py_DECREF(object);
        }
        for (boolean, expected) in [
            ((&raw mut Py_False).cast(), 0),
            ((&raw mut Py_True).cast(), 1),
        ] {
            let indexed = molt_cpython_abi::api::abstract_number::PyNumber_Index(boolean);
            assert!(!indexed.is_null());
            assert_ne!(indexed, boolean);
            assert_eq!(
                (*indexed).ob_type,
                &raw mut molt_cpython_abi::abi_types::PyLong_Type
            );
            assert_eq!(
                molt_cpython_abi::api::numbers::PyLong_AsLong(indexed),
                expected
            );
            molt_cpython_abi::api::refcount::Py_DECREF(indexed);
        }
    }
    unsafe {
        let object = molt_cpython_abi::api::numbers::PyFloat_FromDouble(19.25);
        let refs = (*object).ob_refcnt;
        let converted = molt_cpython_abi::api::abstract_number::PyNumber_Float(object);
        assert_eq!(converted, object);
        assert_eq!((*object).ob_refcnt, refs + 1);
        molt_cpython_abi::api::refcount::Py_DECREF(converted);
        molt_cpython_abi::api::refcount::Py_DECREF(object);
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    }
    assert_eq!(NUMERIC_ADOPTIONS.with(Cell::get), 0);
    assert_eq!(NUMERIC_BYTE_IMPORTS.with(Cell::get), 0);
}

#[test]
fn test_pylong_from_unsigned_long() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromUnsignedLong(100) };
    assert!(!py.is_null());
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLong(py) };
    assert_eq!(val, 100);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_as_unsigned_longlong_and_byte_array() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromUnsignedLong(0x1234) };
    assert!(!py.is_null());
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLong(py) };
    assert_eq!(val, 0x1234);

    let mut little = [0u8; 4];
    let little_rc = unsafe {
        molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
            py.cast(),
            little.as_mut_ptr(),
            little.len(),
            1,
            0,
        )
    };
    assert_eq!(little_rc, 0);
    assert_eq!(little, [0x34, 0x12, 0x00, 0x00]);

    let mut big = [0u8; 4];
    let big_rc = unsafe {
        molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
            py.cast(),
            big.as_mut_ptr(),
            big.len(),
            0,
            0,
        )
    };
    assert_eq!(big_rc, 0);
    assert_eq!(big, [0x00, 0x00, 0x12, 0x34]);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_as_byte_array_rejects_unsigned_negative() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-1) };
    let mut bytes = [0u8; 1];
    let rc = unsafe {
        molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
            py.cast(),
            bytes.as_mut_ptr(),
            bytes.len(),
            1,
            0,
        )
    };
    assert_eq!(rc, -1);
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_pylong_from_unsigned_longlong_non_inline_requires_runtime_hook() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromUnsignedLongLong(u64::MAX) };
    assert!(
        py.is_null(),
        "heap unsigned BigInt construction requires registered runtime hooks"
    );
}

#[test]
fn test_pylong_void_ptr_roundtrip_inline_pointer_value() {
    init();
    let raw = 0x1234usize as *mut c_void;
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromVoidPtr(raw) };
    assert!(!py.is_null());
    let roundtrip = unsafe { molt_cpython_abi::api::numbers::PyLong_AsVoidPtr(py) };
    assert_eq!(roundtrip, raw);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_as_void_ptr_preserves_negative_signed_cast() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-1) };
    let roundtrip = unsafe { molt_cpython_abi::api::numbers::PyLong_AsVoidPtr(py) };
    assert_eq!(roundtrip as usize, usize::MAX);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_as_void_ptr_null_returns_null() {
    init();
    let roundtrip = unsafe { molt_cpython_abi::api::numbers::PyLong_AsVoidPtr(ptr::null_mut()) };
    assert!(roundtrip.is_null());
}

#[test]
fn test_pylong_from_double_truncates_toward_zero() {
    init();
    let positive = unsafe { molt_cpython_abi::api::numbers::PyLong_FromDouble(12.75) };
    let negative = unsafe { molt_cpython_abi::api::numbers::PyLong_FromDouble(-12.75) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(positive) },
        12
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(negative) },
        -12
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(positive);
        molt_cpython_abi::api::refcount::Py_DECREF(negative);
    }
}

#[test]
fn test_pylong_from_double_rejects_nan_and_infinity() {
    init();
    let nan = unsafe { molt_cpython_abi::api::numbers::PyLong_FromDouble(f64::NAN) };
    assert!(nan.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let inf = unsafe { molt_cpython_abi::api::numbers::PyLong_FromDouble(f64::INFINITY) };
    assert!(inf.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ---------------------------------------------------------------------------
// PyFloat
// ---------------------------------------------------------------------------

#[test]
fn test_pyfloat_from_double_returns_non_null() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(PI) };
    assert!(!py.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_roundtrip() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(E) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py) };
    assert!((val - E).abs() < 1e-10);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_negative() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(-1.5) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py) };
    assert!((val - (-1.5)).abs() < 1e-10);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_zero() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(0.0) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py) };
    assert_eq!(val, 0.0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_asdouble_null_returns_minus_one() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(ptr::null_mut()) };
    assert_eq!(val, -1.0);
}

#[test]
fn test_pyfloat_asdouble_from_int_coerces() {
    init();
    // PyFloat_AsDouble on an int object should coerce to double
    let py_int = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(7) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyFloat_AsDouble(py_int) };
    assert_eq!(val, 7.0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py_int) };
}

#[test]
fn test_py_hash_double_matches_integer_hash_for_integral_values() {
    init();
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), 0.0) },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), 1.0) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), -1.0) },
        -2
    );
}

#[test]
fn test_py_hash_double_handles_infinity_and_nan() {
    init();
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), f64::INFINITY) },
        314159
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), f64::NEG_INFINITY)
        },
        -314159
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::_Py_HashDouble(ptr::null_mut(), f64::NAN) },
        0
    );
}

// ---------------------------------------------------------------------------
// PyComplex
// ---------------------------------------------------------------------------

#[test]
fn test_pycomplex_roundtrip() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyComplex_FromDoubles(1.25, -2.5) };
    assert!(!py.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyComplex_Check(py) },
        1
    );

    let value = unsafe { molt_cpython_abi::api::numbers::PyComplex_AsCComplex(py) };
    assert_eq!(value.real, 1.25);
    assert_eq!(value.imag, -2.5);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pycomplex_as_c_complex_from_int() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(9) };
    let value = unsafe { molt_cpython_abi::api::numbers::PyComplex_AsCComplex(py) };
    assert_eq!(value.real, 9.0);
    assert_eq!(value.imag, 0.0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

// ---------------------------------------------------------------------------
// PyBool
// ---------------------------------------------------------------------------

#[test]
fn test_pybool_from_long_true() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(1) };
    assert!(std::ptr::eq(py, (&raw mut Py_True).cast()));
}

#[test]
fn test_pybool_from_long_false() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(0) };
    assert!(std::ptr::eq(py, (&raw mut Py_False).cast()));
}

#[test]
fn test_pybool_from_long_nonzero_is_true() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(42) };
    assert!(std::ptr::eq(py, (&raw mut Py_True).cast()));
}

#[test]
fn test_pybool_from_long_negative_is_true() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyBool_FromLong(-1) };
    assert!(std::ptr::eq(py, (&raw mut Py_True).cast()));
}

// ---------------------------------------------------------------------------
// Type checks
// ---------------------------------------------------------------------------

#[test]
fn test_pylong_check_on_int() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyLong_Check(py) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_check_on_float_returns_false() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.0) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyLong_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_check_on_float() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.0) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyFloat_Check(py) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pyfloat_check_on_int_returns_false() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyFloat_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pylong_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::numbers::PyLong_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_pyfloat_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::numbers::PyFloat_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_pybool_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::numbers::PyBool_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

// ---------------------------------------------------------------------------
// PyNumber_Check
// ---------------------------------------------------------------------------

#[test]
fn test_pynumber_check_on_int() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyNumber_Check(py) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pynumber_check_on_float() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5) };
    let result = unsafe { molt_cpython_abi::api::numbers::PyNumber_Check(py) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_pynumber_check_null_returns_zero() {
    init();
    let result = unsafe { molt_cpython_abi::api::numbers::PyNumber_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_pyindex_check_matches_integer_index_contract() {
    init();
    let py_int = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let py_float = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5) };

    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_number::PyIndex_Check(py_int) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_number::PyIndex_Check((&raw mut Py_True).cast()) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_number::PyIndex_Check(py_float) },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::abstract_number::PyIndex_Check(ptr::null_mut()) },
        0
    );

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py_int);
        molt_cpython_abi::api::refcount::Py_DECREF(py_float);
    }
}

// ---------------------------------------------------------------------------
// PyLong_AsLong on a bool (should coerce to 0/1)
// ---------------------------------------------------------------------------

#[test]
fn test_pylong_aslong_on_true_returns_one() {
    init();
    let py_true = (&raw mut Py_True).cast();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py_true) };
    assert_eq!(val, 1);
}

#[test]
fn test_pylong_aslong_on_false_returns_zero() {
    init();
    let py_false = (&raw mut Py_False).cast();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_AsLong(py_false) };
    assert_eq!(val, 0);
}
use std::f64::consts::{E, PI};

#[test]
fn every_managed_binary_entry_preserves_normal_or_inplace_mode() {
    use molt_cpython_abi::api::abstract_number::*;
    type Binary = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject;
    // Literal public-contract inventory, independent of the implementation match.
    let cases: [(u32, Binary, Binary); 12] = [
        (0, PyNumber_Add, PyNumber_InPlaceAdd),
        (1, PyNumber_Subtract, PyNumber_InPlaceSubtract),
        (2, PyNumber_Multiply, PyNumber_InPlaceMultiply),
        (3, PyNumber_TrueDivide, PyNumber_InPlaceTrueDivide),
        (4, PyNumber_FloorDivide, PyNumber_InPlaceFloorDivide),
        (5, PyNumber_Remainder, PyNumber_InPlaceRemainder),
        (6, PyNumber_Lshift, PyNumber_InPlaceLshift),
        (7, PyNumber_Rshift, PyNumber_InPlaceRshift),
        (8, PyNumber_And, PyNumber_InPlaceAnd),
        (9, PyNumber_Or, PyNumber_InPlaceOr),
        (10, PyNumber_Xor, PyNumber_InPlaceXor),
        (11, PyNumber_MatrixMultiply, PyNumber_InPlaceMatrixMultiply),
    ];
    init();
    unsafe {
        let left = molt_cpython_abi::api::numbers::PyLong_FromLong(8);
        let right = molt_cpython_abi::api::numbers::PyLong_FromLong(3);
        for (op, normal, inplace) in cases {
            for (mode, call, expected) in [(0, normal, 23), (1, inplace, 71)] {
                NUMBER_CALL.with(|value| value.set(None));
                let result = call(left, right);
                assert!(!result.is_null());
                assert_eq!(
                    NUMBER_CALL.with(Cell::get),
                    Some((
                        op,
                        mode,
                        MoltObject::from_int(8).bits(),
                        MoltObject::from_int(3).bits()
                    ))
                );
                assert_eq!(
                    molt_cpython_abi::api::numbers::PyLong_AsLong(result),
                    expected
                );
                molt_cpython_abi::api::refcount::Py_DECREF(result);
            }
        }
        molt_cpython_abi::api::refcount::Py_DECREF(left);
        molt_cpython_abi::api::refcount::Py_DECREF(right);
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    }
}

// Physical digits are an independent input oracle, not a bridge-produced value.
// CPython uses base-2^30 limbs on every supported Molt target.
#[repr(C)]
struct PhysicalLongFixture {
    ob_base: PyObject,
    tag: usize,
    digits: [u32; 4],
}

fn physical_long_fixture(
    tp: *mut PyTypeObject,
    negative: bool,
    magnitude: u128,
) -> PhysicalLongFixture {
    let count = ((128 - magnitude.leading_zeros()) as usize).div_ceil(30);
    PhysicalLongFixture {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: tp,
        },
        tag: (count << 3)
            | if count == 0 {
                1
            } else if negative {
                2
            } else {
                0
            },
        digits: std::array::from_fn(|index| ((magnitude >> (index * 30)) & ((1 << 30) - 1)) as u32),
    }
}

#[test]
fn test_physical_integer_subtype_index_ignores_override_and_returns_exact_int() {
    init();
    DENY_NUMERIC_ADOPTION.with(|deny| deny.set(true));
    let mut slots: PyNumberMethods = unsafe { std::mem::zeroed() };
    slots.nb_index = forbidden_index_override as *const () as *mut c_void;
    let mut subtype = support::StaticType::new();
    subtype.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    subtype.tp_base = &raw mut molt_cpython_abi::abi_types::PyLong_Type;
    subtype.tp_name = c"IntSubtype".as_ptr();
    subtype.tp_as_number = (&raw mut slots).cast();
    unsafe {
        for (negative, magnitude) in [
            (false, 257),
            (true, 257),
            (false, u64::MAX as u128),
            (true, 1u128 << 63),
        ] {
            let mut input = physical_long_fixture(subtype.as_ptr(), negative, magnitude);
            let object = (&raw mut input).cast::<PyObject>();
            assert_eq!(
                molt_cpython_abi::api::abstract_number::PyIndex_Check(object),
                1
            );
            assert_eq!(molt_cpython_abi::api::numbers::PyNumber_Check(object), 1);
            let indexed = molt_cpython_abi::api::abstract_number::PyNumber_Index(object);
            assert!(!indexed.is_null());
            assert_ne!(indexed, object);
            assert_eq!(
                (*indexed).ob_type,
                &raw mut molt_cpython_abi::abi_types::PyLong_Type
            );
            if negative {
                assert_eq!(
                    molt_cpython_abi::api::numbers::PyLong_AsLongLong(indexed),
                    -(magnitude as i128) as i64
                );
            } else {
                assert_eq!(
                    molt_cpython_abi::api::numbers::PyLong_AsUnsignedLongLong(indexed),
                    magnitude as u64
                );
            }
            assert_eq!(input.ob_base.ob_refcnt, 1);
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::refcount::Py_DECREF(indexed);
        }
    }
    assert_eq!(INDEX_OVERRIDE_CALLS.with(Cell::get), 0);
    assert_eq!(NUMERIC_ADOPTIONS.with(Cell::get), 0);
    assert_eq!(NUMERIC_BYTE_IMPORTS.with(Cell::get), 0);
}

#[test]
fn test_as_long_long_overflow_has_cpython_exact_error_and_recovers() {
    init();
    unsafe {
        for (negative, magnitude) in [
            (false, 1u128 << 63),
            (true, (1u128 << 63) + 1),
            (false, 1u128 << 100),
            (true, 1u128 << 100),
        ] {
            let mut input = physical_long_fixture(
                &raw mut molt_cpython_abi::abi_types::PyLong_Type,
                negative,
                magnitude,
            );
            assert_eq!(
                molt_cpython_abi::api::numbers::PyLong_AsLongLong((&raw mut input).cast()),
                -1
            );
            assert_ne!(
                molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast()
                ),
                0
            );
            assert_eq!(
                support::take_current_error_text().as_deref(),
                Some("int too big to convert")
            );
            assert_eq!(input.ob_base.ob_refcnt, 1);
            let recovered = molt_cpython_abi::api::numbers::PyLong_FromLongLong(-1);
            assert_eq!(
                molt_cpython_abi::api::numbers::PyLong_AsLongLong(recovered),
                -1
            );
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::refcount::Py_DECREF(recovered);
        }
    }
}

#[test]
fn test_managed_numeric_predicates_use_inherited_type_protocol_presence() {
    init();
    // The instance really is an opaque managed heap object. Its class is a
    // separate native type with a real dictionary/base edge; there is no fake
    // numeric tag, payload, special lookup, or conversion-result hook.
    for (name, number, index) in [
        (c"__index__", 1, 1),
        (c"__int__", 1, 0),
        (c"__float__", 1, 0),
        (c"__complex__", 0, 0),
        (c"__add__", 0, 0),
        (c"ordinary", 0, 0),
    ] {
        unsafe {
            let mut base = support::StaticType::new();
            base.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
            base.tp_name = c"ProtocolBase".as_ptr();
            base.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
            base.tp_dict = molt_cpython_abi::api::mapping::PyDict_New();
            assert!(!base.tp_dict.is_null());
            // Even a noncallable declared method occupies a numeric slot on
            // CPython: the predicate must not bind or call it.
            assert_eq!(
                molt_cpython_abi::api::mapping::PyDict_SetItemString(
                    base.tp_dict,
                    name.as_ptr(),
                    &raw mut Py_None
                ),
                0
            );
            let mut class = support::StaticType::new();
            class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
            class.tp_name = c"ProtocolChild".as_ptr();
            class.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
            class.tp_base = base.as_ptr();
            class.tp_dict = molt_cpython_abi::api::mapping::PyDict_New();
            assert!(!class.tp_dict.is_null());
            let class_bits = support::fake_runtime::fresh_handle();
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(class.as_ptr().cast(), class_bits, true)
                .unwrap();
            let object_bits = support::fake_runtime::fresh_handle();
            PROTOCOL_CLASS.with(|value| value.set(Some((object_bits, class_bits))));
            let object = GLOBAL_BRIDGE.owned_handle_to_pyobj(object_bits);
            assert!(!object.is_null());
            assert_eq!(
                (*object).ob_type,
                &raw mut molt_cpython_abi::abi_types::MoltManaged_Type
            );
            let object_refs = (*object).ob_refcnt;
            let class_refs = class.ob_base.ob_base.ob_refcnt;
            assert_eq!(
                molt_cpython_abi::api::numbers::PyNumber_Check(object),
                number,
                "{name:?}"
            );
            assert_eq!(
                molt_cpython_abi::api::abstract_number::PyIndex_Check(object),
                index,
                "{name:?}"
            );
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            assert_eq!((*object).ob_refcnt, object_refs);
            assert_eq!(class.ob_base.ob_base.ob_refcnt, class_refs);
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                c"retained predicate error".as_ptr(),
            );
            assert_eq!(
                molt_cpython_abi::api::numbers::PyNumber_Check(object),
                number
            );
            assert_eq!(
                molt_cpython_abi::api::abstract_number::PyIndex_Check(object),
                index
            );
            assert_ne!(
                molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                ),
                0
            );
            assert_eq!(
                support::take_current_error_text().as_deref(),
                Some("retained predicate error")
            );
            assert_eq!((*object).ob_refcnt, object_refs);
            assert_eq!(class.ob_base.ob_base.ob_refcnt, class_refs);
            molt_cpython_abi::api::refcount::Py_DECREF(object);
            PROTOCOL_CLASS.with(|value| value.set(None));
            assert!(
                GLOBAL_BRIDGE
                    .unbind_static_pyobj_from_runtime_handle(class.as_ptr().cast(), class_bits)
            );
            support::fake_runtime::dec_ref(class_bits);
        }
    }
}

#[test]
fn test_foreign_numeric_predicates_use_slots_without_calling_them() {
    init();
    for (number, index, kind) in [(1, 1, 0), (1, 0, 1), (1, 0, 2), (0, 0, 3), (0, 0, 4)] {
        let mut slots: PyNumberMethods = unsafe { std::mem::zeroed() };
        let slot = forbidden_index_override as *const () as *mut c_void;
        match kind {
            0 => slots.nb_index = slot,
            1 => slots.nb_int = slot,
            2 => slots.nb_float = slot,
            3 => slots.nb_add = foreign_binary_slot as *const () as *mut c_void,
            _ => {}
        }
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        class.tp_as_number = (&raw mut slots).cast();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        unsafe {
            assert_eq!(
                molt_cpython_abi::api::numbers::PyNumber_Check(&raw mut object),
                number
            );
            assert_eq!(
                molt_cpython_abi::api::abstract_number::PyIndex_Check(&raw mut object),
                index
            );
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        }
        assert_eq!(object.ob_refcnt, 1);
    }
    assert_eq!(INDEX_OVERRIDE_CALLS.with(Cell::get), 0);
    assert_eq!(FOREIGN_BINARY_CALLS.with(Cell::get), 0);
}

#[test]
fn test_wide_physical_integer_subtype_index_preserves_limbs_and_ownership() {
    init();
    let mut slots: PyNumberMethods = unsafe { std::mem::zeroed() };
    slots.nb_index = forbidden_index_override as *const () as *mut c_void;
    let mut subtype = support::StaticType::new();
    subtype.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    subtype.tp_base = &raw mut molt_cpython_abi::abi_types::PyLong_Type;
    subtype.tp_name = c"WideIntSubtype".as_ptr();
    subtype.tp_as_number = (&raw mut slots).cast();
    unsafe {
        for value in [(1i128 << 100) + 13, -((1i128 << 100) + 13)] {
            let mut input =
                physical_long_fixture(subtype.as_ptr(), value < 0, value.unsigned_abs());
            let original_digits = input.digits;
            let object = (&raw mut input).cast::<PyObject>();
            assert_eq!(
                molt_cpython_abi::api::abstract_number::PyIndex_Check(object),
                1
            );
            let indexed = molt_cpython_abi::api::abstract_number::PyNumber_Index(object);
            assert!(!indexed.is_null());
            assert_ne!(indexed, object);
            assert_eq!(
                (*indexed).ob_type,
                &raw mut molt_cpython_abi::abi_types::PyLong_Type
            );
            let mut bytes = [0u8; 16];
            assert_eq!(
                molt_cpython_abi::api::numbers::_PyLong_AsByteArray(
                    indexed.cast(),
                    bytes.as_mut_ptr(),
                    bytes.len(),
                    1,
                    1
                ),
                0
            );
            assert_eq!(bytes, value.to_le_bytes());
            assert_eq!(input.digits, original_digits);
            assert_eq!(input.ob_base.ob_refcnt, 1);
            molt_cpython_abi::api::refcount::Py_DECREF(indexed);
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        }
    }
    assert_eq!(INDEX_OVERRIDE_CALLS.with(Cell::get), 0);
}

#[test]
fn test_managed_numeric_carrier_keeps_semantic_subtype_protocols() {
    init();
    unsafe {
        for is_float in [false, true] {
            let mut callable_type = support::StaticType::new();
            callable_type.ob_base.ob_base.ob_type =
                &raw mut molt_cpython_abi::abi_types::PyType_Type;
            callable_type.tp_name = c"ComplexConverter".as_ptr();
            callable_type.tp_call = Some(complex_override);
            let mut callable = PyObject {
                ob_refcnt: 1,
                ob_type: callable_type.as_ptr(),
            };
            let mut class = support::StaticType::new();
            class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
            class.tp_name = c"ManagedNumericSubtype".as_ptr();
            class.tp_base = if is_float {
                &raw mut molt_cpython_abi::abi_types::PyFloat_Type
            } else {
                &raw mut molt_cpython_abi::abi_types::PyLong_Type
            };
            class.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
            class.tp_dict = molt_cpython_abi::api::mapping::PyDict_New();
            assert!(!class.tp_dict.is_null());
            assert_eq!(
                molt_cpython_abi::api::mapping::PyDict_SetItemString(
                    class.tp_dict,
                    c"__complex__".as_ptr(),
                    &raw mut callable
                ),
                0
            );
            let class_bits = support::fake_runtime::fresh_handle();
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(class.as_ptr().cast(), class_bits, true)
                .unwrap();
            let bits = if is_float {
                support::fake_runtime::heap_float(19.25)
            } else {
                support::fake_runtime::heap_integer(319)
            };
            PROTOCOL_CLASS.with(|value| value.set(Some((bits, class_bits))));
            let object = GLOBAL_BRIDGE.owned_handle_to_pyobj(bits);
            assert!(!object.is_null());
            assert_eq!((*object).ob_type, class.tp_base);
            assert_eq!(
                molt_cpython_abi::api::typeobj::_Py_TYPE(object),
                class.as_ptr()
            );
            let refs = (*object).ob_refcnt;
            FLOAT_OVERRIDE_CALLS.with(|calls| calls.set(0));
            COMPLEX_OVERRIDE_CALLS.with(|calls| calls.set(0));
            if !is_float {
                let index = molt_cpython_abi::api::abstract_number::PyNumber_Index(object);
                assert!(!index.is_null());
                assert_ne!(index, object);
                assert_eq!(molt_cpython_abi::api::numbers::PyLong_CheckExact(index), 1);
                assert_eq!(molt_cpython_abi::api::numbers::PyLong_AsLong(index), 319);
                molt_cpython_abi::api::refcount::Py_DECREF(index);
            }
            assert_eq!(
                molt_cpython_abi::api::numbers::PyFloat_AsDouble(object),
                if is_float { 19.25 } else { 42.5 }
            );
            assert_eq!(FLOAT_OVERRIDE_CALLS.with(Cell::get), usize::from(!is_float));
            let integer = molt_cpython_abi::api::abstract_number::PyNumber_Long(object);
            assert!(!integer.is_null());
            assert_eq!(
                molt_cpython_abi::api::numbers::PyLong_CheckExact(integer),
                1
            );
            assert_eq!(molt_cpython_abi::api::numbers::PyLong_AsLong(integer), 777);
            molt_cpython_abi::api::refcount::Py_DECREF(integer);
            let floated = molt_cpython_abi::api::abstract_number::PyNumber_Float(object);
            assert!(!floated.is_null());
            assert_ne!(floated, object);
            assert_eq!(
                molt_cpython_abi::api::numbers::PyFloat_CheckExact(floated),
                1
            );
            assert_eq!(
                molt_cpython_abi::api::numbers::PyFloat_AsDouble(floated),
                42.5
            );
            molt_cpython_abi::api::refcount::Py_DECREF(floated);
            let complex = molt_cpython_abi::api::numbers::PyComplex_AsCComplex(object);
            assert_eq!((complex.real, complex.imag), (12.5, -3.0));
            assert_eq!(COMPLEX_OVERRIDE_CALLS.with(Cell::get), 1);
            for minor in [12, 13, 14] {
                TARGET_MINOR.with(|target| target.set(minor));
                COMPLEX_OVERRIDE_CALLS.with(|calls| calls.set(0));
                let real = molt_cpython_abi::api::numbers::PyComplex_RealAsDouble(object);
                let imag = molt_cpython_abi::api::numbers::PyComplex_ImagAsDouble(object);
                assert_eq!(
                    real,
                    if minor >= 13 {
                        12.5
                    } else if is_float {
                        19.25
                    } else {
                        42.5
                    }
                );
                assert_eq!(imag, if minor >= 13 { -3.0 } else { 0.0 });
                assert_eq!(
                    COMPLEX_OVERRIDE_CALLS.with(Cell::get),
                    if minor >= 13 { 2 } else { 0 }
                );
            }
            assert_eq!((*object).ob_refcnt, refs);
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::refcount::Py_DECREF(object);
            PROTOCOL_CLASS.with(|value| value.set(None));
            assert!(
                GLOBAL_BRIDGE
                    .unbind_static_pyobj_from_runtime_handle(class.as_ptr().cast(), class_bits)
            );
            support::fake_runtime::dec_ref(class_bits);
            // The dict owns the foreign callable; retire it before stack data.
            molt_cpython_abi::api::refcount::Py_CLEAR(&raw mut class.tp_dict);
            assert_eq!(callable.ob_refcnt, 1);
        }
    }
}

#[test]
fn test_native_integer_subtype_int_override_is_distinct_from_index_admission() {
    init();
    let mut slots: PyNumberMethods = unsafe { std::mem::zeroed() };
    slots.nb_int = forbidden_index_override as *const () as *mut c_void;
    slots.nb_index = forbidden_index_override as *const () as *mut c_void;
    let mut subtype = support::StaticType::new();
    subtype.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    subtype.tp_base = &raw mut molt_cpython_abi::abi_types::PyLong_Type;
    subtype.tp_name = c"OverridingInt".as_ptr();
    subtype.tp_as_number = (&raw mut slots).cast();
    let mut input = physical_long_fixture(subtype.as_ptr(), false, 319);
    unsafe {
        let object = (&raw mut input).cast::<PyObject>();
        let index = molt_cpython_abi::api::abstract_number::PyNumber_Index(object);
        assert_eq!(molt_cpython_abi::api::numbers::PyLong_AsLong(index), 319);
        assert_eq!(INDEX_OVERRIDE_CALLS.with(Cell::get), 0);
        let integer = molt_cpython_abi::api::abstract_number::PyNumber_Long(object);
        assert_eq!(molt_cpython_abi::api::numbers::PyLong_AsLong(integer), 7);
        assert_eq!(INDEX_OVERRIDE_CALLS.with(Cell::get), 1);
        assert_eq!(molt_cpython_abi::api::numbers::PyLong_CheckExact(index), 1);
        assert_eq!(
            molt_cpython_abi::api::numbers::PyLong_CheckExact(integer),
            1
        );
        molt_cpython_abi::api::refcount::Py_DECREF(index);
        molt_cpython_abi::api::refcount::Py_DECREF(integer);
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    }
    assert_eq!(input.ob_base.ob_refcnt, 1);
}

#[test]
fn test_complex_parts_versioned_non_numeric_refusal_and_recovery() {
    init();
    let mut class = support::StaticType::new();
    class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    class.tp_name = c"Plain".as_ptr();
    class.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
    class.tp_dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    assert!(!class.tp_dict.is_null());
    let mut object = PyObject {
        ob_refcnt: 1,
        ob_type: class.as_ptr(),
    };
    unsafe {
        for minor in [12, 13, 14] {
            TARGET_MINOR.with(|target| target.set(minor));
            let imag = molt_cpython_abi::api::numbers::PyComplex_ImagAsDouble(&raw mut object);
            if minor == 12 {
                assert_eq!(imag, 0.0);
                assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            } else {
                assert_eq!(imag, -1.0);
                assert_ne!(
                    molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                    ),
                    0
                );
                assert_eq!(
                    support::take_current_error_text().as_deref(),
                    Some("must be real number, not Plain")
                );
            }
            let real = molt_cpython_abi::api::numbers::PyComplex_RealAsDouble(&raw mut object);
            assert_eq!(real, -1.0);
            assert_eq!(
                support::take_current_error_text().as_deref(),
                Some("must be real number, not Plain")
            );
            let recovered = molt_cpython_abi::api::numbers::PyFloat_FromDouble(-1.0);
            assert_eq!(
                molt_cpython_abi::api::numbers::PyComplex_RealAsDouble(recovered),
                -1.0
            );
            assert_eq!(
                molt_cpython_abi::api::numbers::PyComplex_ImagAsDouble(recovered),
                0.0
            );
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            molt_cpython_abi::api::refcount::Py_DECREF(recovered);
        }
    }
    assert_eq!(object.ob_refcnt, 1);
}

// Warning transport is shared with member tests; this scope also retires
// numeric-specific protocol controls when a case unwinds.
fn with_numeric_warning_provider(run: impl FnOnce()) {
    struct ResetNumericCallbacks;
    impl Drop for ResetNumericCallbacks {
        fn drop(&mut self) {
            DENY_NUMERIC_BYTE_IMPORT.with(|value| value.set(false));
            SLOT_RESULT.with(|value| value.set(0));
        }
    }
    let _reset = ResetNumericCallbacks;
    support::warnings::with_provider(run);
}

unsafe extern "C" fn owned_index_slot_result(_object: *mut PyObject) -> *mut PyObject {
    SLOT_RESULT.with(|value| value.replace(0)) as *mut PyObject
}

unsafe extern "C" fn index_result_finalizer(_object: *mut PyObject) {
    INDEX_RESULT_FINALIZERS.with(|count| count.set(count.get() + 1));
    // A C finalizer may leave an error. Extraction must preserve its own error.
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast(),
            c"index result finalizer".as_ptr(),
        )
    };
}

#[test]
fn private_index_consumers_extract_wide_subtypes_without_exact_materialization() {
    use molt_cpython_abi::api::{abstract_number, errors, numbers, refcount, sequences, slice};
    unsafe extern "C" {
        fn PyArg_ParseTuple(args: *mut PyObject, format: *const std::ffi::c_char, ...) -> i32;
    }
    init();
    with_numeric_warning_provider(|| unsafe {
        let mut slots: PyNumberMethods = std::mem::zeroed();
        slots.nb_index = borrowed_slot_result as *const () as *mut c_void;
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        class.tp_name = c"IndexProducer".as_ptr();
        class.tp_as_number = (&raw mut slots).cast();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        let object_ptr = &raw mut object;
        let mut subtype = support::StaticType::new();
        subtype.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        subtype.tp_base = &raw mut molt_cpython_abi::abi_types::PyLong_Type;
        subtype.tp_name = c"IndexResult".as_ptr();
        let magnitude = (1u128 << 100) + 13;
        let mut result = physical_long_fixture(subtype.as_ptr(), false, magnitude);
        SLOT_RESULT.with(|value| value.set((&raw mut result) as usize));
        DENY_NUMERIC_BYTE_IMPORT.with(|value| value.set(true));
        let imports = NUMERIC_BYTE_IMPORTS.with(Cell::get);
        assert_eq!(numbers::PyLong_AsUnsignedLongLongMask(object_ptr), 13);
        let mut overflow = 0;
        assert_eq!(
            numbers::PyLong_AsLongLongAndOverflow(object_ptr, &raw mut overflow),
            -1
        );
        assert_eq!(overflow, 1);
        assert!(errors::PyErr_Occurred().is_null());
        let mut bytes = [0u8; 16];
        assert_eq!(
            numbers::PyLong_AsNativeBytes(object_ptr, bytes.as_mut_ptr().cast(), 16, 17),
            13
        );
        assert_eq!(bytes, magnitude.to_le_bytes());
        assert_eq!(
            abstract_number::PyNumber_AsSsize_t(object_ptr, ptr::null_mut()),
            isize::MAX
        );
        assert!(errors::PyErr_Occurred().is_null());
        let args = refcount::OwnedPyObject::from_owned(sequences::PyTuple_FromArray(
            &raw const object_ptr,
            1,
        ));
        assert!(!args.as_ptr().is_null());
        let mut output = 77isize;
        assert_eq!(
            PyArg_ParseTuple(args.as_ptr(), c"n".as_ptr(), &raw mut output),
            0
        );
        assert_eq!(output, 77);
        assert_eq!(
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast()
            ),
            1
        );
        errors::PyErr_Clear();
        let bound = refcount::OwnedPyObject::from_owned(slice::PySlice_New(
            ptr::null_mut(),
            object_ptr,
            ptr::null_mut(),
        ));
        assert!(!bound.as_ptr().is_null());
        let (mut start, mut stop, mut step) = (77, 77, 77);
        assert_eq!(
            slice::PySlice_Unpack(bound.as_ptr(), &raw mut start, &raw mut stop, &raw mut step),
            0
        );
        assert_eq!((start, stop, step), (0, isize::MAX, 1));
        assert_eq!(NUMERIC_BYTE_IMPORTS.with(Cell::get), imports);
        assert_eq!(result.ob_base.ob_refcnt, 1);
        let original_name = class.tp_name;
        let long_name = std::ffi::CString::new("X".repeat(205)).unwrap();
        for (name, expected) in [
            (original_name, "IndexProducer".to_string()),
            (long_name.as_ptr(), "X".repeat(200)),
        ] {
            class.tp_name = name;
            assert_eq!(
                abstract_number::PyNumber_AsSsize_t(
                    object_ptr,
                    (&raw mut molt_cpython_abi::abi_types::PyExc_IndexError).cast(),
                ),
                -1
            );
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_IndexError).cast()
                ),
                1
            );
            assert_eq!(
                support::take_current_error_text(),
                Some(format!(
                    "cannot fit '{expected}' into an index-sized integer"
                ))
            );
        }
        class.tp_name = original_name;
        assert!(
            support::warnings::last_message()
                .unwrap()
                .starts_with("__index__ returned non-int (type IndexResult)")
        );
        // A real public projection must attempt the wide allocation. The same
        // denial therefore distinguishes private extraction from exact copying.
        assert!(abstract_number::PyNumber_Index(object_ptr).is_null());
        assert_eq!(NUMERIC_BYTE_IMPORTS.with(Cell::get), imports + 1);
        assert_eq!(
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast()
            ),
            1
        );
        errors::PyErr_Clear();
        assert_eq!(result.ob_base.ob_refcnt, 1);
        DENY_NUMERIC_BYTE_IMPORT.with(|value| value.set(false));
        let public =
            refcount::OwnedPyObject::from_owned(abstract_number::PyNumber_Index(object_ptr));
        assert!(!public.as_ptr().is_null());
        assert_eq!(numbers::PyLong_CheckExact(public.as_ptr()), 1);
        assert_ne!(public.as_ptr(), (&raw mut result).cast());
        drop(public);
        support::warnings::set_as_error(true);
        output = 77;
        assert_eq!(
            PyArg_ParseTuple(args.as_ptr(), c"n".as_ptr(), &raw mut output),
            0
        );
        assert_eq!(output, 77);
        assert_eq!(
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_DeprecationWarning).cast()
            ),
            1
        );
        errors::PyErr_Clear();
        support::warnings::set_as_error(false);
        // Transfer the slot's sole reference; its destructor must run once and
        // must not replace the overflow found while that result was alive.
        subtype.tp_dealloc = Some(index_result_finalizer);
        slots.nb_index = owned_index_slot_result as *const () as *mut c_void;
        INDEX_RESULT_FINALIZERS.with(|count| count.set(0));
        assert_eq!(numbers::PyLong_AsLongLong(object_ptr), -1);
        assert_eq!(INDEX_RESULT_FINALIZERS.with(Cell::get), 1);
        assert_eq!(
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast()
            ),
            1
        );
        assert_eq!(result.ob_base.ob_refcnt, 0);
        errors::PyErr_Clear();
        drop(bound);
        drop(args);
        assert_eq!(object.ob_refcnt, 1);
    });
}

#[test]
fn native_integer_byte_constructors_share_cpython_flags_and_error_ownership() {
    use molt_cpython_abi::api::{errors, numbers, refcount};
    init();
    struct ResetFailure;
    impl Drop for ResetFailure {
        fn drop(&mut self) {
            DENY_NUMERIC_BYTE_IMPORT.with(|value| value.set(false));
            NUMERIC_BYTE_RUNTIME_FAILURE.with(|value| value.set(None));
            NUMERIC_RUNTIME_PENDING.with(|value| value.set(0));
        }
    }
    let _reset = ResetFailure;
    unsafe {
        let bytes = [0x80u8, 0x01];
        let native_signed = i16::from_ne_bytes(bytes) as i64;
        let native_unsigned = u16::from_ne_bytes(bytes) as i64;
        // Pinned CPython 3.13/3.14 longobject.c resolves bit 1 as native,
        // honors UNSIGNED_BUFFER for the signed entry, and ignores high bits.
        // Explicit expected results keep this independent of Molt's resolver.
        let cases = [
            (0, -32767, 32769, [0x01, 0x80]),
            (1, 384, 384, [0x80, 0x01]),
            (2, native_signed, native_unsigned, 384u16.to_ne_bytes()),
            (3, native_signed, native_unsigned, 384u16.to_ne_bytes()),
            (4, 32769, 32769, [0x01, 0x80]),
            (5, 384, 384, [0x80, 0x01]),
            (6, native_unsigned, native_unsigned, 384u16.to_ne_bytes()),
            (7, native_unsigned, native_unsigned, 384u16.to_ne_bytes()),
            (-1, native_signed, native_unsigned, 384u16.to_ne_bytes()),
            (-2, native_unsigned, native_unsigned, 384u16.to_ne_bytes()),
            (-3, 384, 384, [0x80, 0x01]),
            (-4, 32769, 32769, [0x01, 0x80]),
            (256, -32767, 32769, [0x01, 0x80]),
            (257, 384, 384, [0x80, 0x01]),
        ];
        let export = refcount::OwnedPyObject::from_owned(numbers::PyLong_FromLong(384));
        type Constructor = unsafe extern "C" fn(*const c_void, usize, i32) -> *mut PyObject;
        let constructors: [Constructor; 2] = [
            numbers::PyLong_FromNativeBytes,
            numbers::PyLong_FromUnsignedNativeBytes,
        ];
        for (flags, signed, unsigned, encoded) in cases {
            for (constructor, expected) in constructors.into_iter().zip([signed, unsigned]) {
                let value = refcount::OwnedPyObject::from_owned(constructor(
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    flags,
                ));
                assert!(!value.as_ptr().is_null(), "flags {flags}");
                assert_eq!(
                    numbers::PyLong_AsLongLong(value.as_ptr()),
                    expected,
                    "flags {flags}"
                );
                let empty = refcount::OwnedPyObject::from_owned(constructor(
                    bytes.as_ptr().cast(),
                    0,
                    flags,
                ));
                assert_eq!(numbers::PyLong_AsLong(empty.as_ptr()), 0);
            }
            let mut observed = [0xa5u8; 2];
            assert_eq!(
                numbers::PyLong_AsNativeBytes(
                    export.as_ptr(),
                    observed.as_mut_ptr().cast(),
                    2,
                    flags
                ),
                2
            );
            assert_eq!(observed, encoded, "export flags {flags}");
            assert!(errors::PyErr_Occurred().is_null());
        }
        for constructor in constructors {
            for n in [0, 2] {
                let calls = NUMERIC_BYTE_IMPORTS.with(Cell::get);
                assert!(constructor(ptr::null(), n, 0).is_null());
                assert_eq!(NUMERIC_BYTE_IMPORTS.with(Cell::get), calls);
                assert_eq!(
                    errors::PyErr_ExceptionMatches(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
                    ),
                    1
                );
                errors::PyErr_Clear();
            }
            // Producer failures survive both public wrappers; recovery proves
            // that neither failure commits a numeric view or poisons the next call.
            DENY_NUMERIC_BYTE_IMPORT.with(|value| value.set(true));
            assert!(constructor(bytes.as_ptr().cast(), 2, 0).is_null());
            assert_eq!(
                errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast()
                ),
                1
            );
            errors::PyErr_Clear();
            DENY_NUMERIC_BYTE_IMPORT.with(|value| value.set(false));
            NUMERIC_BYTE_RUNTIME_FAILURE.with(|value| value.set(Some(0x1234_5678)));
            assert!(constructor(bytes.as_ptr().cast(), 2, 0).is_null());
            assert_eq!(NUMERIC_RUNTIME_PENDING.with(Cell::get), 0x1234_5678);
            assert_eq!(
                errors::PyErr_Occurred(),
                (&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast()
            );
            NUMERIC_RUNTIME_PENDING.with(|value| value.set(0));
            NUMERIC_BYTE_RUNTIME_FAILURE.with(|value| value.set(None));
            let recovered =
                refcount::OwnedPyObject::from_owned(constructor(bytes.as_ptr().cast(), 2, 1));
            assert_eq!(numbers::PyLong_AsLong(recovered.as_ptr()), 384);
            assert!(errors::PyErr_Occurred().is_null());
        }
    }
}

#[test]
fn managed_numeric_hook_failures_preserve_runtime_error_before_projection() {
    use molt_cpython_abi::api::{abstract_number, errors, numbers, refcount};
    use molt_cpython_abi::hooks::NumberUnaryOp;
    init();
    struct ResetFailure;
    impl Drop for ResetFailure {
        fn drop(&mut self) {
            NUMERIC_UNARY_OUTCOME.with(|value| value.set(None));
            NUMERIC_UNARY_OPERATION.with(|value| value.set(None));
            NUMERIC_RUNTIME_PENDING.with(|value| value.set(0));
        }
    }
    unsafe {
        // A managed complex object reaches all three conversion hooks without
        // taking a physical integer or float identity shortcut.
        let bits = support::fake_runtime::heap_complex(3.5, 2.0);
        let object = refcount::OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(bits));
        assert!(!object.as_ptr().is_null());
        let _reset = ResetFailure;
        let count = (*object.as_ptr()).ob_refcnt;
        type Conversion = unsafe extern "C" fn(*mut PyObject) -> *mut PyObject;
        let conversions: [(NumberUnaryOp, Conversion); 3] = [
            (NumberUnaryOp::Index, abstract_number::PyNumber_Index),
            (NumberUnaryOp::Long, abstract_number::PyNumber_Long),
            (NumberUnaryOp::Float, abstract_number::PyNumber_Float),
        ];
        for (operation, convert) in conversions {
            for outcome in [NumericHookOutcome::Error, NumericHookOutcome::Missing] {
                for pending in [0, 0x1234_5678] {
                    NUMERIC_UNARY_OPERATION.with(|value| value.set(None));
                    NUMERIC_UNARY_OUTCOME.with(|value| value.set(Some((outcome, pending))));
                    assert!(convert(object.as_ptr()).is_null());
                    assert_eq!(
                        NUMERIC_UNARY_OPERATION.with(Cell::get),
                        Some(operation as u32)
                    );
                    assert_eq!((*object.as_ptr()).ob_refcnt, count);
                    assert_eq!(NUMERIC_RUNTIME_PENDING.with(Cell::get), pending);
                    if pending != 0 {
                        assert_eq!(
                            errors::PyErr_Occurred(),
                            (&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast()
                        );
                    } else {
                        assert_eq!(
                            errors::PyErr_ExceptionMatches(
                                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast()
                            ),
                            1
                        );
                        errors::PyErr_Clear();
                    }
                    NUMERIC_RUNTIME_PENDING.with(|value| value.set(0));
                    NUMERIC_UNARY_OUTCOME
                        .with(|value| value.set(Some((NumericHookOutcome::Value, 0))));
                    let recovered = refcount::OwnedPyObject::from_owned(convert(object.as_ptr()));
                    assert!(!recovered.as_ptr().is_null());
                    if operation as u32 == NumberUnaryOp::Float as u32 {
                        assert_eq!(numbers::PyFloat_AsDouble(recovered.as_ptr()), 42.5);
                    } else {
                        assert_eq!(numbers::PyLong_AsLong(recovered.as_ptr()), 42);
                    }
                    assert!(errors::PyErr_Occurred().is_null());
                    assert_eq!(NUMERIC_RUNTIME_PENDING.with(Cell::get), 0);
                }
            }
        }
    }
}

#[test]
fn numeric_classification_preserves_runtime_only_and_subtype_provider_failures() {
    use molt_cpython_abi::api::{abstract_number, errors, refcount};
    init();
    unsafe {
        errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast(),
            c"subtype provider failure".as_ptr(),
        );
        let failure = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        struct ResetFailure;
        impl Drop for ResetFailure {
            fn drop(&mut self) {
                NUMERIC_RUNTIME_PENDING.with(|value| value.set(0));
                SUBTYPE_CALLBACK_ERROR.with(|value| value.set(0));
                SUBTYPE_RUNTIME_FAILURE.with(|value| value.set(false));
            }
        }
        let _reset = ResetFailure;
        for value in [42, (1i128 << 100) + 13] {
            let bits = support::fake_runtime::heap_integer(value);
            let object =
                refcount::OwnedPyObject::from_owned(GLOBAL_BRIDGE.owned_handle_to_pyobj(bits));
            assert!(!object.as_ptr().is_null());
            let count = (*object.as_ptr()).ob_refcnt;
            let calls = NUMERIC_CLASS_CALLS.with(Cell::get);
            let imports = NUMERIC_BYTE_IMPORTS.with(Cell::get);
            NUMERIC_RUNTIME_PENDING.with(|value| value.set(0x1234_5678));
            assert!(abstract_number::PyNumber_Index(object.as_ptr()).is_null());
            assert_eq!(NUMERIC_RUNTIME_PENDING.with(Cell::get), 0x1234_5678);
            assert_eq!(NUMERIC_CLASS_CALLS.with(Cell::get), calls);
            assert_eq!(NUMERIC_BYTE_IMPORTS.with(Cell::get), imports);
            assert_eq!((*object.as_ptr()).ob_refcnt, count);
            assert_eq!(
                errors::PyErr_Occurred(),
                (&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast()
            );
            NUMERIC_RUNTIME_PENDING.with(|value| value.set(0));
            let recovered = refcount::OwnedPyObject::from_owned(abstract_number::PyNumber_Index(
                object.as_ptr(),
            ));
            assert_eq!(recovered.as_ptr(), object.as_ptr());
        }
        for runtime_only in [false, true] {
            if runtime_only {
                SUBTYPE_RUNTIME_FAILURE.with(|value| value.set(true));
            } else {
                SUBTYPE_CALLBACK_ERROR.with(|value| value.set(failure.as_ptr() as usize));
            }
            let before = NUMERIC_SUBTYPE_CALLS.with(Cell::get);
            assert!(
                abstract_number::PyNumber_Index((&raw mut Py_True).cast::<PyObject>()).is_null()
            );
            assert_eq!(NUMERIC_SUBTYPE_CALLS.with(Cell::get), before + 1);
            if runtime_only {
                assert_eq!(NUMERIC_RUNTIME_PENDING.with(Cell::get), 0x1234_5678);
                assert_eq!(
                    errors::PyErr_Occurred(),
                    (&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast()
                );
            } else {
                let observed =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(observed.as_ptr(), failure.as_ptr());
            }
            SUBTYPE_CALLBACK_ERROR.with(|value| value.set(0));
            SUBTYPE_RUNTIME_FAILURE.with(|value| value.set(false));
            NUMERIC_RUNTIME_PENDING.with(|value| value.set(0));
            let recovered = refcount::OwnedPyObject::from_owned(abstract_number::PyNumber_Index(
                (&raw mut Py_True).cast::<PyObject>(),
            ));
            assert!(!recovered.as_ptr().is_null());
            assert_eq!(
                molt_cpython_abi::api::numbers::PyLong_AsLong(recovered.as_ptr()),
                1
            );
        }
    }
}

#[test]
fn float_slot_diagnostics_bound_source_and_result_names_to_fifty_bytes() {
    use molt_cpython_abi::api::{abstract_number, errors};
    init();
    with_numeric_warning_provider(|| unsafe {
        let source_name = std::ffi::CString::new("S".repeat(70)).unwrap();
        let result_name = std::ffi::CString::new("R".repeat(70)).unwrap();
        let mut methods: PyNumberMethods = std::mem::zeroed();
        methods.nb_float = borrowed_slot_result as *const () as *mut c_void;
        let mut source_type = support::StaticType::new();
        source_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        source_type.tp_name = source_name.as_ptr();
        source_type.tp_as_number = (&raw mut methods).cast();
        let mut source = PyObject {
            ob_refcnt: 1,
            ob_type: source_type.as_ptr(),
        };
        let mut result_type = support::StaticType::new();
        result_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        result_type.tp_name = result_name.as_ptr();
        for valid_subtype in [false, true] {
            result_type.tp_base = if valid_subtype {
                &raw mut molt_cpython_abi::abi_types::PyFloat_Type
            } else {
                &raw mut molt_cpython_abi::abi_types::PyBaseObject_Type
            };
            let mut result = molt_cpython_abi::abi_types::PyFloatObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: result_type.as_ptr(),
                },
                ob_fval: 3.5,
            };
            SLOT_RESULT.with(|value| value.set((&raw mut result) as usize));
            support::warnings::set_as_error(valid_subtype);
            assert!(abstract_number::PyNumber_Float(&raw mut source).is_null());
            let suffix = if valid_subtype {
                ".  The ability to return an instance of a strict subclass of float is deprecated, and may be removed in a future version of Python."
            } else {
                ""
            };
            let expected = format!(
                "{}.__float__ returned non-float (type {}){suffix}",
                "S".repeat(50),
                "R".repeat(50)
            );
            let category = if valid_subtype {
                (&raw mut molt_cpython_abi::abi_types::PyExc_DeprecationWarning).cast()
            } else {
                (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
            };
            assert_eq!(errors::PyErr_ExceptionMatches(category), 1);
            assert_eq!(support::take_current_error_text(), Some(expected));
            assert_eq!(result.ob_base.ob_refcnt, 1);
            SLOT_RESULT.with(|value| value.set(0));
        }
        assert_eq!(source.ob_refcnt, 1);
    });
}

#[test]
fn numeric_result_class_failure_preserves_exact_error_and_owned_results() {
    use molt_cpython_abi::api::{abstract_number, errors, mapping, numbers, refcount};
    init();
    with_numeric_warning_provider(|| unsafe {
        let mut slots: PyNumberMethods = std::mem::zeroed();
        slots.nb_index = borrowed_slot_result as *const () as *mut c_void;
        slots.nb_int = borrowed_slot_result as *const () as *mut c_void;
        slots.nb_float = borrowed_slot_result as *const () as *mut c_void;
        let mut callable_type = support::StaticType::new();
        callable_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        callable_type.tp_call = Some(borrowed_call_result);
        let mut callable = PyObject {
            ob_refcnt: 1,
            ob_type: callable_type.as_ptr(),
        };
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        class.tp_name = c"NumericResultProducer".as_ptr();
        class.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
        class.tp_as_number = (&raw mut slots).cast();
        class.tp_dict = mapping::PyDict_New();
        assert_eq!(
            mapping::PyDict_SetItemString(
                class.tp_dict,
                c"__complex__".as_ptr(),
                &raw mut callable
            ),
            0
        );
        let mut producer = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast(),
            c"numeric class failure".as_ptr(),
        );
        let failure = refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
        assert!(!failure.as_ptr().is_null());
        struct ClearFailure;
        impl Drop for ClearFailure {
            fn drop(&mut self) {
                CLASS_CALLBACK_ERROR.with(|value| value.set(None));
                SLOT_RESULT.with(|value| value.set(0));
            }
        }
        for (kind, bits) in [
            (0, support::fake_runtime::heap_integer(42)),
            (0, support::fake_runtime::heap_integer((1i128 << 100) + 13)),
            (1, support::fake_runtime::heap_float(3.5)),
            (2, support::fake_runtime::heap_complex(3.5, 2.0)),
        ] {
            let result = refcount::OwnedPyObject::from_owned(
                GLOBAL_BRIDGE.owned_result_to_pyobj(OwnedHandleResult::ok(bits)),
            );
            assert!(!result.as_ptr().is_null());
            let count = (*result.as_ptr()).ob_refcnt;
            let imports = NUMERIC_BYTE_IMPORTS.with(Cell::get);
            let _failure_slot = ClearFailure;
            SLOT_RESULT.with(|value| value.set(result.as_ptr() as usize));
            CLASS_CALLBACK_ERROR.with(|value| value.set(Some((bits, failure.as_ptr() as usize))));
            let check_error = || {
                let observed =
                    refcount::OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(observed.as_ptr(), failure.as_ptr());
                assert_eq!((*result.as_ptr()).ob_refcnt, count);
                assert_eq!(NUMERIC_BYTE_IMPORTS.with(Cell::get), imports);
            };
            match kind {
                0 => {
                    assert!(abstract_number::PyNumber_Index(result.as_ptr()).is_null());
                    check_error();
                    for convert in [
                        abstract_number::PyNumber_Index,
                        abstract_number::PyNumber_Long,
                    ] {
                        assert!(convert(&raw mut producer).is_null());
                        check_error();
                    }
                }
                1 => {
                    assert!(abstract_number::PyNumber_Float(&raw mut producer).is_null());
                    check_error();
                }
                _ => {
                    let failed = numbers::PyComplex_AsCComplex(&raw mut producer);
                    assert_eq!((failed.real, failed.imag), (-1.0, 0.0));
                    check_error();
                }
            }
            CLASS_CALLBACK_ERROR.with(|value| value.set(None));
            match kind {
                0 => {
                    let recovered = refcount::OwnedPyObject::from_owned(
                        abstract_number::PyNumber_Index(&raw mut producer),
                    );
                    assert_eq!(recovered.as_ptr(), result.as_ptr());
                }
                1 => {
                    let recovered = refcount::OwnedPyObject::from_owned(
                        abstract_number::PyNumber_Float(&raw mut producer),
                    );
                    assert_eq!(recovered.as_ptr(), result.as_ptr());
                }
                _ => {
                    let recovered = numbers::PyComplex_AsCComplex(&raw mut producer);
                    assert_eq!((recovered.real, recovered.imag), (3.5, 2.0));
                }
            }
            assert!(errors::PyErr_Occurred().is_null());
            assert_eq!((*result.as_ptr()).ob_refcnt, count);
        }
        refcount::Py_CLEAR(&raw mut class.tp_dict);
        assert_eq!(callable.ob_refcnt, 1);
        assert_eq!(producer.ob_refcnt, 1);
    });
}

#[test]
fn test_foreign_integer_slot_results_validate_normalize_and_preserve_warning_failure() {
    init();
    with_numeric_warning_provider(|| unsafe {
        let mut slots: PyNumberMethods = std::mem::zeroed();
        slots.nb_int = borrowed_slot_result as *const () as *mut c_void;
        slots.nb_index = borrowed_slot_result as *const () as *mut c_void;
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        class.tp_as_number = (&raw mut slots).cast();
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        let mut subtype = support::StaticType::new();
        subtype.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        subtype.tp_base = &raw mut molt_cpython_abi::abi_types::PyLong_Type;
        subtype.tp_name = c"ReturnedInt".as_ptr();
        let mut result = physical_long_fixture(subtype.as_ptr(), false, 319);
        for (convert, method) in [
            (
                molt_cpython_abi::api::abstract_number::PyNumber_Long
                    as unsafe extern "C" fn(*mut PyObject) -> *mut PyObject,
                "__int__",
            ),
            (
                molt_cpython_abi::api::abstract_number::PyNumber_Index,
                "__index__",
            ),
        ] {
            SLOT_RESULT.with(|value| value.set((&raw mut result) as usize));
            let exact = convert(&raw mut object);
            assert!(!exact.is_null());
            assert_eq!(molt_cpython_abi::api::numbers::PyLong_CheckExact(exact), 1);
            assert_eq!(molt_cpython_abi::api::numbers::PyLong_AsLong(exact), 319);
            molt_cpython_abi::api::refcount::Py_DECREF(exact);
            let expected = format!(
                "{method} returned non-int (type ReturnedInt).  The ability to return an instance of a strict subclass of int is deprecated, and may be removed in a future version of Python."
            );
            assert_eq!(
                support::warnings::last_message().as_deref(),
                Some(expected.as_str())
            );
            assert_eq!(result.ob_base.ob_refcnt, 1);
            support::warnings::set_as_error(true);
            assert!(convert(&raw mut object).is_null());
            assert_eq!(
                support::take_current_error_text().as_deref(),
                Some(expected.as_str())
            );
            assert_eq!(result.ob_base.ob_refcnt, 1);
            support::warnings::set_as_error(false);
            let wrong = molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5);
            SLOT_RESULT.with(|value| value.set(wrong as usize));
            assert!(convert(&raw mut object).is_null());
            assert_eq!(
                support::take_current_error_text().as_deref(),
                Some(format!("{method} returned non-int (type float)").as_str())
            );
            molt_cpython_abi::api::refcount::Py_DECREF(wrong);
        }
        SLOT_RESULT.with(|value| value.set(0));
        assert_eq!(object.ob_refcnt, 1);
    });
}

#[test]
fn test_foreign_trunc_delegation_version_warning_order_and_result_validation() {
    init();
    with_numeric_warning_provider(|| unsafe {
        let mut callable_type = support::StaticType::new();
        callable_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        callable_type.tp_call = Some(trunc_call);
        let mut callable = PyObject {
            ob_refcnt: 1,
            ob_type: callable_type.as_ptr(),
        };
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        class.tp_name = c"TruncOnly".as_ptr();
        class.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
        class.tp_dict = molt_cpython_abi::api::mapping::PyDict_New();
        assert_eq!(
            molt_cpython_abi::api::mapping::PyDict_SetItemString(
                class.tp_dict,
                c"__trunc__".as_ptr(),
                &raw mut callable
            ),
            0
        );
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        for minor in [12, 13, 14] {
            TARGET_MINOR.with(|target| target.set(minor));
            TRUNC_CALLS.with(|value| value.set(0));
            TRUNC_BAD_RESULT.with(|value| value.set(false));
            support::warnings::clear();
            let result = molt_cpython_abi::api::abstract_number::PyNumber_Long(&raw mut object);
            if minor < 14 {
                assert!(!result.is_null());
                assert_eq!(molt_cpython_abi::api::numbers::PyLong_AsLong(result), 41);
                molt_cpython_abi::api::refcount::Py_DECREF(result);
                assert_eq!(TRUNC_CALLS.with(Cell::get), 1);
                assert_eq!(
                    support::warnings::last_message().as_deref(),
                    Some("The delegation of int() to __trunc__ is deprecated.")
                );
                support::warnings::set_as_error(true);
                assert!(
                    molt_cpython_abi::api::abstract_number::PyNumber_Long(&raw mut object)
                        .is_null()
                );
                assert_eq!(
                    TRUNC_CALLS.with(Cell::get),
                    1,
                    "warning failure precedes callback"
                );
                assert_eq!(
                    support::take_current_error_text().as_deref(),
                    Some("The delegation of int() to __trunc__ is deprecated.")
                );
                support::warnings::set_as_error(false);
                TRUNC_BAD_RESULT.with(|value| value.set(true));
                assert!(
                    molt_cpython_abi::api::abstract_number::PyNumber_Long(&raw mut object)
                        .is_null()
                );
                assert_eq!(
                    support::take_current_error_text().as_deref(),
                    Some("__trunc__ returned non-Integral (type float)")
                );
            } else {
                assert!(result.is_null());
                assert_eq!(TRUNC_CALLS.with(Cell::get), 0);
                assert!(support::warnings::last_message().is_none());
                assert_eq!(
                    support::take_current_error_text().as_deref(),
                    Some(
                        "int() argument must be a string, a bytes-like object or a real number, not 'TruncOnly'"
                    )
                );
            }
            assert_eq!(object.ob_refcnt, 1);
        }
        molt_cpython_abi::api::refcount::Py_CLEAR(&raw mut class.tp_dict);
        assert_eq!(callable.ob_refcnt, 1);
    });
}

#[test]
fn test_foreign_number_long_without_conversion_reports_exact_type_error() {
    init();
    let mut class = support::StaticType::new();
    class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    class.tp_name = c"Opaque".as_ptr();
    class.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
    class.tp_dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    assert!(!class.tp_dict.is_null());
    let mut object = PyObject {
        ob_refcnt: 1,
        ob_type: class.as_ptr(),
    };
    unsafe {
        for minor in [12, 13, 14] {
            TARGET_MINOR.with(|target| target.set(minor));
            assert!(
                molt_cpython_abi::api::abstract_number::PyNumber_Long(&raw mut object).is_null()
            );
            assert_ne!(
                molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                ),
                0
            );
            assert_eq!(
                support::take_current_error_text().as_deref(),
                Some(
                    "int() argument must be a string, a bytes-like object or a real number, not 'Opaque'"
                )
            );
            assert_eq!(object.ob_refcnt, 1);
        }
    }
}

#[test]
fn test_foreign_float_storage_read_and_float_constructor_have_distinct_protocols() {
    init();
    let mut methods: PyNumberMethods = unsafe { std::mem::zeroed() };
    methods.nb_float = foreign_float_override as *const () as *mut c_void;
    let mut subtype = support::StaticType::new();
    subtype.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    subtype.tp_base = &raw mut molt_cpython_abi::abi_types::PyFloat_Type;
    subtype.tp_name = c"ForeignFloat".as_ptr();
    subtype.tp_as_number = (&raw mut methods).cast();
    let mut object = molt_cpython_abi::abi_types::PyFloatObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: subtype.as_ptr(),
        },
        ob_fval: 19.25,
    };
    unsafe {
        let pointer = (&raw mut object).cast();
        assert_eq!(
            molt_cpython_abi::api::numbers::PyFloat_AsDouble(pointer),
            19.25
        );
        assert_eq!(FLOAT_OVERRIDE_CALLS.with(Cell::get), 0);
        let converted = molt_cpython_abi::api::abstract_number::PyNumber_Float(pointer);
        assert!(!converted.is_null());
        assert_eq!(
            molt_cpython_abi::api::numbers::PyFloat_AsDouble(converted),
            42.5
        );
        assert_eq!(FLOAT_OVERRIDE_CALLS.with(Cell::get), 1);
        molt_cpython_abi::api::refcount::Py_DECREF(converted);
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    }
    assert_eq!(object.ob_base.ob_refcnt, 1);
}

#[test]
fn test_complex_subtype_result_warning_and_failure_preserve_owners() {
    init();
    with_numeric_warning_provider(|| unsafe {
        let mut callable_type = support::StaticType::new();
        callable_type.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        callable_type.tp_call = Some(borrowed_call_result);
        let mut callable = PyObject {
            ob_refcnt: 1,
            ob_type: callable_type.as_ptr(),
        };
        let mut class = support::StaticType::new();
        class.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        class.tp_flags = molt_cpython_abi::abi_types::Py_TPFLAGS_READY;
        class.tp_dict = molt_cpython_abi::api::mapping::PyDict_New();
        assert_eq!(
            molt_cpython_abi::api::mapping::PyDict_SetItemString(
                class.tp_dict,
                c"__complex__".as_ptr(),
                &raw mut callable
            ),
            0
        );
        let mut object = PyObject {
            ob_refcnt: 1,
            ob_type: class.as_ptr(),
        };
        let mut subtype = support::StaticType::new();
        subtype.ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
        subtype.tp_base = &raw mut molt_cpython_abi::abi_types::PyComplex_Type;
        subtype.tp_name = c"ReturnedComplex".as_ptr();
        let mut result = molt_cpython_abi::abi_types::PyComplexObject {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: subtype.as_ptr(),
            },
            cval: molt_cpython_abi::abi_types::Py_complex {
                real: 2.5,
                imag: -3.0,
            },
        };
        SLOT_RESULT.with(|value| value.set((&raw mut result) as usize));
        let converted = molt_cpython_abi::api::numbers::PyComplex_AsCComplex(&raw mut object);
        assert_eq!((converted.real, converted.imag), (2.5, -3.0));
        let message = "__complex__ returned non-complex (type ReturnedComplex).  The ability to return an instance of a strict subclass of complex is deprecated, and may be removed in a future version of Python.";
        assert_eq!(support::warnings::last_message().as_deref(), Some(message));
        assert_eq!(result.ob_base.ob_refcnt, 1);
        support::warnings::set_as_error(true);
        let failed = molt_cpython_abi::api::numbers::PyComplex_AsCComplex(&raw mut object);
        assert_eq!((failed.real, failed.imag), (-1.0, 0.0));
        assert_eq!(support::take_current_error_text().as_deref(), Some(message));
        assert_eq!(result.ob_base.ob_refcnt, 1);
        assert_eq!(object.ob_refcnt, 1);
        support::warnings::set_as_error(false);
        SLOT_RESULT.with(|value| value.set(0));
        molt_cpython_abi::api::refcount::Py_CLEAR(&raw mut class.tp_dict);
        assert_eq!(callable.ob_refcnt, 1);
    });
}
