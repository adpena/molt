//! Number abstract protocol — PyNumber_* operations.
//!
//! These implement the abstract numeric operations that work across int, float,
//! and bool types by bridging to Molt's internal object model.

use crate::abi_types::{Py_ssize_t, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ptr;

/// Helper: resolve a PyObject to its Molt bits.
fn resolve_bits(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    GLOBAL_BRIDGE.lock().pyobj_to_handle(op)
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

/// Helper: check if either operand is a float.
fn either_is_float(a_bits: u64, b_bits: u64) -> bool {
    MoltObject::from_bits(a_bits).is_float() || MoltObject::from_bits(b_bits).is_float()
}

/// Helper: build a PyObject from a float result.
fn pyobj_from_float(v: f64) -> *mut PyObject {
    let bits = MoltObject::from_float(v).bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

/// Helper: build a PyObject from an int result.
fn pyobj_from_int(v: i64) -> *mut PyObject {
    let bits = MoltObject::from_int(v).bits();
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

// ─── Binary arithmetic ───────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Add(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    if either_is_float(a, b) {
        match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => pyobj_from_float(x + y),
            _ => ptr::null_mut(),
        }
    } else {
        match (as_i64(a), as_i64(b)) {
            (Some(x), Some(y)) => pyobj_from_int(x.wrapping_add(y)),
            _ => ptr::null_mut(),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Subtract(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    if either_is_float(a, b) {
        match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => pyobj_from_float(x - y),
            _ => ptr::null_mut(),
        }
    } else {
        match (as_i64(a), as_i64(b)) {
            (Some(x), Some(y)) => pyobj_from_int(x.wrapping_sub(y)),
            _ => ptr::null_mut(),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Multiply(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    if either_is_float(a, b) {
        match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => pyobj_from_float(x * y),
            _ => ptr::null_mut(),
        }
    } else {
        match (as_i64(a), as_i64(b)) {
            (Some(x), Some(y)) => pyobj_from_int(x.wrapping_mul(y)),
            _ => ptr::null_mut(),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_TrueDivide(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    match (as_f64(a), as_f64(b)) {
        (Some(x), Some(y)) => {
            if y == 0.0 {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_ZeroDivisionError,
                        c"division by zero".as_ptr(),
                    );
                }
                ptr::null_mut()
            } else {
                pyobj_from_float(x / y)
            }
        }
        _ => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_FloorDivide(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    if either_is_float(a, b) {
        match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => {
                if y == 0.0 {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_ZeroDivisionError,
                            c"integer division or modulo by zero".as_ptr(),
                        );
                    }
                    ptr::null_mut()
                } else {
                    pyobj_from_float((x / y).floor())
                }
            }
            _ => ptr::null_mut(),
        }
    } else {
        match (as_i64(a), as_i64(b)) {
            (Some(x), Some(y)) => {
                if y == 0 {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_ZeroDivisionError,
                            c"integer division or modulo by zero".as_ptr(),
                        );
                    }
                    ptr::null_mut()
                } else {
                    pyobj_from_int(x.div_euclid(y))
                }
            }
            _ => ptr::null_mut(),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Remainder(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    if either_is_float(a, b) {
        match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => {
                if y == 0.0 {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_ZeroDivisionError,
                            c"integer division or modulo by zero".as_ptr(),
                        );
                    }
                    ptr::null_mut()
                } else {
                    pyobj_from_float(x % y)
                }
            }
            _ => ptr::null_mut(),
        }
    } else {
        match (as_i64(a), as_i64(b)) {
            (Some(x), Some(y)) => {
                if y == 0 {
                    unsafe {
                        crate::api::errors::PyErr_SetString(
                            &raw mut crate::abi_types::PyExc_ZeroDivisionError,
                            c"integer division or modulo by zero".as_ptr(),
                        );
                    }
                    ptr::null_mut()
                } else {
                    pyobj_from_int(x.rem_euclid(y))
                }
            }
            _ => ptr::null_mut(),
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Power(
    o1: *mut PyObject,
    o2: *mut PyObject,
    o3: *mut PyObject,
) -> *mut PyObject {
    let _ = o3; // modulus argument — rarely used, ignore for now
    let a = match resolve_bits(o1) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let b = match resolve_bits(o2) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    if either_is_float(a, b) {
        match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => pyobj_from_float(x.powf(y)),
            _ => ptr::null_mut(),
        }
    } else {
        match (as_i64(a), as_i64(b)) {
            (Some(x), Some(y)) => {
                if y < 0 {
                    // Negative exponent → float result
                    pyobj_from_float((x as f64).powf(y as f64))
                } else {
                    pyobj_from_int(x.wrapping_pow(y as u32))
                }
            }
            _ => ptr::null_mut(),
        }
    }
}

// ─── Unary operations ────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Negative(o: *mut PyObject) -> *mut PyObject {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let obj = MoltObject::from_bits(bits);
    if obj.is_float() {
        match obj.as_float() {
            Some(v) => pyobj_from_float(-v),
            None => ptr::null_mut(),
        }
    } else if obj.is_int() {
        match obj.as_int() {
            Some(v) => pyobj_from_int(-v),
            None => ptr::null_mut(),
        }
    } else if obj.is_bool() {
        match obj.as_bool() {
            Some(b) => pyobj_from_int(if b { -1 } else { 0 }),
            None => ptr::null_mut(),
        }
    } else {
        ptr::null_mut()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Positive(o: *mut PyObject) -> *mut PyObject {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let obj = MoltObject::from_bits(bits);
    if obj.is_float() {
        match obj.as_float() {
            Some(v) => pyobj_from_float(v),
            None => ptr::null_mut(),
        }
    } else if obj.is_int() || obj.is_bool() {
        match as_i64(bits) {
            Some(v) => pyobj_from_int(v),
            None => ptr::null_mut(),
        }
    } else {
        ptr::null_mut()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Absolute(o: *mut PyObject) -> *mut PyObject {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    let obj = MoltObject::from_bits(bits);
    if obj.is_float() {
        match obj.as_float() {
            Some(v) => pyobj_from_float(v.abs()),
            None => ptr::null_mut(),
        }
    } else if obj.is_int() {
        match obj.as_int() {
            Some(v) => pyobj_from_int(v.wrapping_abs()),
            None => ptr::null_mut(),
        }
    } else if obj.is_bool() {
        match obj.as_bool() {
            Some(b) => pyobj_from_int(b as i64),
            None => ptr::null_mut(),
        }
    } else {
        ptr::null_mut()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Invert(o: *mut PyObject) -> *mut PyObject {
    let bits = match resolve_bits(o) {
        Some(b) => b,
        None => return ptr::null_mut(),
    };
    match as_i64(bits) {
        Some(v) => pyobj_from_int(!v),
        None => ptr::null_mut(),
    }
}

// ─── Bitwise operations ──────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Lshift(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = resolve_bits(o1).and_then(as_i64);
    let b = resolve_bits(o2).and_then(as_i64);
    match (a, b) {
        (Some(x), Some(y)) => {
            if y < 0 {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_ValueError,
                        c"negative shift count".as_ptr(),
                    );
                }
                ptr::null_mut()
            } else {
                pyobj_from_int(x.wrapping_shl(y as u32))
            }
        }
        _ => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Rshift(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = resolve_bits(o1).and_then(as_i64);
    let b = resolve_bits(o2).and_then(as_i64);
    match (a, b) {
        (Some(x), Some(y)) => {
            if y < 0 {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        &raw mut crate::abi_types::PyExc_ValueError,
                        c"negative shift count".as_ptr(),
                    );
                }
                ptr::null_mut()
            } else {
                pyobj_from_int(x.wrapping_shr(y as u32))
            }
        }
        _ => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_And(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = resolve_bits(o1).and_then(as_i64);
    let b = resolve_bits(o2).and_then(as_i64);
    match (a, b) {
        (Some(x), Some(y)) => pyobj_from_int(x & y),
        _ => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Or(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = resolve_bits(o1).and_then(as_i64);
    let b = resolve_bits(o2).and_then(as_i64);
    match (a, b) {
        (Some(x), Some(y)) => pyobj_from_int(x | y),
        _ => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Xor(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let a = resolve_bits(o1).and_then(as_i64);
    let b = resolve_bits(o2).and_then(as_i64);
    match (a, b) {
        (Some(x), Some(y)) => pyobj_from_int(x ^ y),
        _ => ptr::null_mut(),
    }
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
    _o1: *mut PyObject,
    _o2: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_TypeError,
            c"unsupported operand type(s) for @".as_ptr(),
        );
    }
    ptr::null_mut()
}
