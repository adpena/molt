//! Tests for PyLong_*, PyFloat_*, PyBool_*, PyNumber_Check and type checks.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    MoltTypeTag, Py_False, Py_None, Py_NotImplementedSentinel, Py_True, PyNumberMethods, PyObject,
    PyTypeObject,
};
use molt_cpython_abi::hooks::{BorrowedHandleResult, OwnedHandleResult};
use molt_lang_obj_model::MoltObject;
use std::cell::Cell;
use std::ffi::c_void;
use std::ptr;

thread_local! {
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
    static UNINITIALIZED_LIST: Cell<Option<(u64, usize, usize)>> = const { Cell::new(None) };
}

unsafe extern "C" fn classify_heap(bits: u64) -> u8 {
    if UNINITIALIZED_LIST
        .with(Cell::get)
        .is_some_and(|value| value.0 == bits)
    {
        MoltTypeTag::List as u8
    } else if support::fake_complex::contains(bits) {
        MoltTypeTag::Complex as u8
    } else {
        MoltTypeTag::Other as u8
    }
}

unsafe extern "C" fn capture_number_power(
    _base_bits: u64,
    _exponent_bits: u64,
    modulus_bits: u64,
) -> OwnedHandleResult {
    POWER_MODULUS_BITS.with(|value| value.set(Some(modulus_bits)));
    if POWER_HOOK_FAILS.with(Cell::get) {
        OwnedHandleResult::error()
    } else {
        OwnedHandleResult::ok(MoltObject::from_int(1).bits())
    }
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

unsafe extern "C" fn alloc_uninitialized_list(size: usize) -> u64 {
    let token = Box::into_raw(Box::new(0u64));
    let bits = MoltObject::from_ptr(token.cast::<u8>()).bits();
    UNINITIALIZED_LIST.with(|value| value.set(Some((bits, size, token as usize))));
    bits
}

unsafe extern "C" fn uninitialized_list_len(bits: u64) -> usize {
    UNINITIALIZED_LIST
        .with(Cell::get)
        .map_or(0, |value| if value.0 == bits { value.1 } else { 0 })
}

unsafe extern "C" fn uninitialized_list_item(bits: u64, index: usize) -> BorrowedHandleResult {
    match UNINITIALIZED_LIST.with(Cell::get) {
        Some((expected, len, _)) if expected == bits && index < len => {
            BorrowedHandleResult::ok(MoltObject::none().bits())
        }
        _ => BorrowedHandleResult::missing(),
    }
}

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.classify_heap = classify_heap;
    hooks.object_hash = support::fake_complex::hash;
    hooks.complex_from_doubles = support::fake_complex::from_doubles;
    hooks.complex_parts = support::fake_complex::parts;
    hooks.number_power = capture_number_power;
    hooks.alloc_list_presized = alloc_uninitialized_list;
    hooks.list_len = uninitialized_list_len;
    hooks.list_item = uninitialized_list_item;
    support::prepare_abi_test_thread(hooks);
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
    UNINITIALIZED_LIST.with(|value| value.set(None));
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

    let (_, _, token) = UNINITIALIZED_LIST
        .with(Cell::get)
        .expect("PyList_New must retain its unique test handle");
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(list);
        drop(Box::from_raw(foreign));
        drop(Box::from_raw(ty));
        drop(Box::from_raw(methods));
    }
    UNINITIALIZED_LIST.with(|value| value.set(None));
    unsafe { drop(Box::from_raw(token as *mut u64)) };
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
fn test_pylong_from_longlong_non_inline_requires_runtime_hook() {
    init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(i64::MAX) };
    assert!(
        py.is_null(),
        "heap BigInt construction requires registered runtime hooks"
    );
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
