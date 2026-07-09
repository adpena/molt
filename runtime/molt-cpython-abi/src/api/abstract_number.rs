//! Number abstract protocol — PyNumber_* operations.
//!
//! The single numeric authority lives in `molt-lang-runtime`: arbitrary-precision
//! int promotion, float coercion, operator-overload dispatch, and CPython-shaped
//! exception raising. This ABI layer MUST NOT reimplement arithmetic — an i64
//! reimplementation silently wraps Python's unbounded ints at 64 bits and masks
//! the exceptions CPython raises. Every `PyNumber_*` here resolves its operands
//! to runtime handle bits and delegates to that authority through the numeric
//! hooks (`number_binary_op` / `number_unary_op` / `number_power`).

use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::{NumberBinaryOp, NumberUnaryOp};
use molt_lang_obj_model::MoltObject;
use std::os::raw::c_int;
use std::ptr;

/// Helper: resolve a PyObject to its Molt bits.
fn resolve_bits(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    GLOBAL_BRIDGE.lock().pyobj_to_handle(op)
}

/// Ensure a NULL return carries a set exception, as the CPython ABI requires.
///
/// The runtime numeric authority sets a pending exception on failure. If a
/// numeric hook returned 0 (error) but no runtime exception is pending — e.g.
/// the runtime hooks were never registered (pre-init/test) — we fail closed with
/// a SystemError rather than returning a bare NULL. A NULL PyObject* without an
/// exception is an ABI violation that corrupts the caller's error handling.
unsafe fn ensure_exception_set() {
    let pending = crate::hooks::hooks()
        .map(|h| unsafe { (h.exception_pending)() } != 0)
        .unwrap_or(false);
    if !pending {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"PyNumber operation failed: runtime numeric authority unavailable".as_ptr(),
            );
        }
    }
}

/// Convert a numeric-result handle from the runtime authority into a PyObject*.
/// `result_bits == 0` signals an error (pending runtime exception); we surface
/// it as NULL, guaranteeing an exception is set.
unsafe fn pyobj_from_result(result_bits: u64) -> *mut PyObject {
    if result_bits == 0 {
        unsafe { ensure_exception_set() };
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(result_bits) }
}

/// Dispatch a binary numeric op through the runtime authority.
unsafe fn binary_op(op: NumberBinaryOp, o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => {
            unsafe { ensure_exception_set() };
            return ptr::null_mut();
        }
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => {
            unsafe { ensure_exception_set() };
            return ptr::null_mut();
        }
    };
    let h = crate::hooks::hooks_or_stubs();
    let result = unsafe { (h.number_binary_op)(op as u32, a, b) };
    unsafe { pyobj_from_result(result) }
}

/// Dispatch a unary numeric op through the runtime authority.
unsafe fn unary_op(op: NumberUnaryOp, o: *mut PyObject) -> *mut PyObject {
    let a = match resolve_bits(o) {
        Some(b) => b,
        None => {
            unsafe { ensure_exception_set() };
            return ptr::null_mut();
        }
    };
    let h = crate::hooks::hooks_or_stubs();
    let result = unsafe { (h.number_unary_op)(op as u32, a) };
    unsafe { pyobj_from_result(result) }
}

/// Helper: extract a numeric value as f64 from Molt bits.
fn as_f64(bits: u64) -> Option<f64> {
    let obj = MoltObject::from_bits(bits);
    if obj.is_float() {
        obj.as_float()
    } else if obj.is_int() {
        obj.as_int().map(|i| i as f64)
    } else if obj.is_bool() {
        obj.as_bool().map(|b| if b { 1.0 } else { 0.0 })
    } else {
        None
    }
}

/// Helper: extract a numeric value as i64 from Molt bits.
fn as_i64(bits: u64) -> Option<i64> {
    let obj = MoltObject::from_bits(bits);
    if obj.is_int() {
        obj.as_int()
    } else if obj.is_bool() {
        obj.as_bool().map(|b| b as i64)
    } else {
        None
    }
}

/// Helper: build a PyObject from a float result.
fn pyobj_from_float(v: f64) -> *mut PyObject {
    let bits = MoltObject::from_float(v).bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

/// Helper: build a PyObject from an int result.
///
/// Routes through the runtime `int_from_i64` hook, which dispatches
/// inline-vs-BigInt correctly. `MoltObject::from_int` alone truncates any value
/// outside the 47-bit inline window (mod 2^47) — the silent-integer-miscompile
/// class — so it must never be used to box an arbitrary i64 here.
fn pyobj_from_int(v: i64) -> *mut PyObject {
    let h = crate::hooks::hooks_or_stubs();
    let bits = unsafe { (h.int_from_i64)(v) };
    if bits == 0 {
        // Hooks unregistered (pre-init/test) — fall back to the inline boxer,
        // valid only for the inline window. Out-of-window values fail closed as
        // a null with a set exception rather than a truncated wrong answer.
        if let Some(inline) = MoltObject::try_from_int(v) {
            return unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(inline.bits()) };
        }
        unsafe { ensure_exception_set() };
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

// ─── Binary arithmetic ───────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Add(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::Add, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Subtract(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::Subtract, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Multiply(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::Multiply, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_TrueDivide(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::TrueDivide, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_FloorDivide(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::FloorDivide, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Remainder(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::Remainder, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Power(
    o1: *mut PyObject,
    o2: *mut PyObject,
    o3: *mut PyObject,
) -> *mut PyObject {
    // CPython's `PyNumber_Power(base, exp, mod)` computes `pow(base, exp, mod)`:
    // a genuine 3-argument modular exponentiation, NOT `base ** exp` with the
    // modulus dropped. When `mod` is None (or absent), it is plain `base ** exp`.
    // The runtime numeric authority owns both forms (`molt_pow` / `molt_pow_mod`)
    // with correct bignum and exception semantics.
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => {
            unsafe { ensure_exception_set() };
            return ptr::null_mut();
        }
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => {
            unsafe { ensure_exception_set() };
            return ptr::null_mut();
        }
    };
    // A NULL o3 means "no modulus" (two-arg pow). A non-NULL o3 that resolves
    // to None likewise means two-arg pow; the runtime authority treats a None
    // / 0 modulus as the two-argument form.
    let mod_bits = if o3.is_null() {
        0
    } else {
        resolve_bits(o3).unwrap_or(0)
    };
    let h = crate::hooks::hooks_or_stubs();
    let result = unsafe { (h.number_power)(a, b, mod_bits) };
    unsafe { pyobj_from_result(result) }
}

// ─── Unary operations ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Negative(o: *mut PyObject) -> *mut PyObject {
    unsafe { unary_op(NumberUnaryOp::Negative, o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Positive(o: *mut PyObject) -> *mut PyObject {
    unsafe { unary_op(NumberUnaryOp::Positive, o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Absolute(o: *mut PyObject) -> *mut PyObject {
    unsafe { unary_op(NumberUnaryOp::Absolute, o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Invert(o: *mut PyObject) -> *mut PyObject {
    unsafe { unary_op(NumberUnaryOp::Invert, o) }
}

// ─── Bitwise operations ──────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Lshift(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::Lshift, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Rshift(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::Rshift, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_And(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::And, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Or(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::Or, o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Xor(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { binary_op(NumberBinaryOp::Xor, o1, o2) }
}

// ─── Type conversions ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Long(o: *mut PyObject) -> *mut PyObject {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let obj = MoltObject::from_bits(bits);
    if obj.is_int() {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    if obj.is_float() {
        match obj.as_float() {
            Some(v) => return pyobj_from_int(v as i64),
            None => return ptr::null_mut(),
        }
    }
    if obj.is_bool() {
        match obj.as_bool() {
            Some(b) => return pyobj_from_int(b as i64),
            None => return ptr::null_mut(),
        }
    }
    ptr::null_mut()
}

/// PyNumber_Int — alias for PyNumber_Long (Python 2 compat, still used).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Int(o: *mut PyObject) -> *mut PyObject {
    unsafe { PyNumber_Long(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Float(o: *mut PyObject) -> *mut PyObject {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let obj = MoltObject::from_bits(bits);
    if obj.is_float() {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    match as_f64(bits) {
        Some(v) => pyobj_from_float(v),
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Index(o: *mut PyObject) -> *mut PyObject {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let obj = MoltObject::from_bits(bits);
    if obj.is_int() {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    if obj.is_bool() {
        match obj.as_bool() {
            Some(b) => return pyobj_from_int(b as i64),
            None => return ptr::null_mut(),
        }
    }
    // Not an integer type — raise TypeError.
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_TypeError,
            c"'float' object cannot be interpreted as an integer".as_ptr(),
        );
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyIndex_Check(o: *mut PyObject) -> c_int {
    let Some(bits) = resolve_bits(o) else {
        return 0;
    };
    let obj = MoltObject::from_bits(bits);
    (obj.is_int() || obj.is_bool()) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_AsSsize_t(o: *mut PyObject, _exc: *mut PyObject) -> Py_ssize_t {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return -1,
    };
    match as_i64(bits) {
        Some(v) => v as Py_ssize_t,
        None => -1,
    }
}

// ─── In-place operations (return new object, same semantics) ─────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceAdd(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_Add(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceSubtract(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_Subtract(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceMultiply(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_Multiply(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceTrueDivide(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_TrueDivide(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceFloorDivide(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_FloorDivide(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceRemainder(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_Remainder(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceLshift(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_Lshift(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceRshift(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_Rshift(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceAnd(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_And(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceOr(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { PyNumber_Or(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceXor(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_Xor(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Divmod(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let quotient = unsafe { PyNumber_FloorDivide(o1, o2) };
    let remainder = unsafe { PyNumber_Remainder(o1, o2) };
    if quotient.is_null() || remainder.is_null() {
        if !quotient.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(quotient) };
        }
        if !remainder.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(remainder) };
        }
        return ptr::null_mut();
    }
    let tuple = unsafe { crate::api::sequences::PyTuple_New(2) };
    if tuple.is_null() {
        unsafe { crate::api::refcount::Py_DECREF(quotient) };
        unsafe { crate::api::refcount::Py_DECREF(remainder) };
        return ptr::null_mut();
    }
    unsafe { crate::api::sequences::PyTuple_SetItem(tuple, 0, quotient) };
    unsafe { crate::api::sequences::PyTuple_SetItem(tuple, 1, remainder) };
    tuple
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_MatrixMultiply(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    // `@` dispatches `__matmul__` on the operands (numpy arrays override it).
    // Route to the runtime authority, which raises a CPython-shaped TypeError
    // for operands that do not support the operator.
    unsafe { binary_op(NumberBinaryOp::MatrixMultiply, o1, o2) }
}
