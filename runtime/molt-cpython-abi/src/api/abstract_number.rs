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
use std::os::raw::{c_int, c_void};
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

/// One of the unary numeric conversion slots on `PyNumberMethods`.
#[derive(Clone, Copy)]
enum NumberSlot {
    Int,
    Float,
    Index,
}

/// Best-effort `tp_name` of `o`'s type, for CPython-shaped error messages and
/// the silent-failure diagnostic trail.
unsafe fn type_name_of(o: *mut PyObject) -> String {
    if o.is_null() {
        return "NULL".to_string();
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return "object".to_string();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        return "object".to_string();
    }
    unsafe { std::ffi::CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// True when an exception is already pending in either the C-API thread-local
/// store or the runtime's own exception slot.
fn conversion_exception_pending() -> bool {
    if !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return true;
    }
    crate::hooks::hooks()
        .map(|h| unsafe { (h.exception_pending)() } != 0)
        .unwrap_or(false)
}

/// Call a foreign object's unary number slot (`nb_int` / `nb_float` /
/// `nb_index`) if the object's type defines it. Returns `None` when the slot is
/// absent (so the caller can try the next fallback), or `Some(result)` where
/// `result` is the slot's return value — a new reference on success, or NULL
/// with a pending exception on failure (both of which the caller propagates).
///
/// This is how CPython's `PyNumber_Long`/`Float`/`Index` reach a C extension's
/// own conversion (Objects/abstract.c). Molt's numeric authority only knows its
/// own native int/float/bool; a foreign object (for example a numpy `int32`
/// scalar) carries its value behind these slots. Skipping them was the silent
/// `NULL`-without-exception bug this dispatch closes: numpy's ufunc reduction
/// setup packs an integer identity through `PyNumber_Long`, and a bare NULL
/// there stranded `_multiarray_umath` init with the opaque "returned non-zero
/// without setting an exception".
unsafe fn call_number_unary_slot(o: *mut PyObject, slot: NumberSlot) -> Option<*mut PyObject> {
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return None;
    }
    let num = unsafe { (*tp).tp_as_number }.cast::<crate::abi_types::PyNumberMethods>();
    if num.is_null() {
        return None;
    }
    let fptr = unsafe {
        match slot {
            NumberSlot::Int => (*num).nb_int,
            NumberSlot::Float => (*num).nb_float,
            NumberSlot::Index => (*num).nb_index,
        }
    };
    if fptr.is_null() {
        return None;
    }
    type UnaryFunc = unsafe extern "C" fn(*mut PyObject) -> *mut PyObject;
    let f: UnaryFunc = unsafe { std::mem::transmute::<*mut c_void, UnaryFunc>(fptr) };
    Some(unsafe { f(o) })
}

/// Finalize a foreign conversion slot's result, enforcing the ABI contract that
/// a NULL return carries a pending exception. If the slot broke that contract,
/// record the site on the permanent silent-failure surface (`MOLT_TRACE_CAPI`)
/// and raise a `SystemError` so the caller never sees a bare NULL.
unsafe fn finalize_slot_result(
    o: *mut PyObject,
    result: *mut PyObject,
    capi_name: &str,
) -> *mut PyObject {
    if result.is_null() && !conversion_exception_pending() {
        crate::capi_trace::record_silent_failure(capi_name, Some(&unsafe { type_name_of(o) }));
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_SystemError,
                c"numeric conversion slot returned NULL without setting an exception".as_ptr(),
            );
        }
    }
    result
}

/// No native or foreign conversion applied: record the site on the permanent
/// silent-failure surface and raise the CPython-shaped `TypeError`, so a failing
/// conversion is never a bare NULL (the C-API contract every caller relies on).
unsafe fn conversion_type_error(o: *mut PyObject, capi_name: &str, message: String) -> *mut PyObject {
    crate::capi_trace::record_silent_failure(capi_name, Some(&unsafe { type_name_of(o) }));
    if !conversion_exception_pending()
        && let Ok(cmsg) = std::ffi::CString::new(message)
    {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                cmsg.as_ptr(),
            );
        }
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Long(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        unsafe { ensure_exception_set() };
        return ptr::null_mut();
    }
    // Native Molt fast path.
    if let Some(bits) = resolve_bits(o) {
        let obj = MoltObject::from_bits(bits);
        if obj.is_int() {
            unsafe { crate::api::refcount::Py_INCREF(o) };
            return o;
        }
        if obj.is_float()
            && let Some(v) = obj.as_float()
        {
            return pyobj_from_int(v as i64);
        }
        if obj.is_bool()
            && let Some(b) = obj.as_bool()
        {
            return pyobj_from_int(b as i64);
        }
    }
    // Foreign object: dispatch to its `nb_int` slot, then `nb_index`
    // (CPython Objects/abstract.c `PyNumber_Long`).
    if let Some(result) = unsafe { call_number_unary_slot(o, NumberSlot::Int) } {
        return unsafe { finalize_slot_result(o, result, "PyNumber_Long") };
    }
    if let Some(result) = unsafe { call_number_unary_slot(o, NumberSlot::Index) } {
        return unsafe { finalize_slot_result(o, result, "PyNumber_Long") };
    }
    let message = format!(
        "int() argument must be a string, a bytes-like object or a real number, not '{}'",
        unsafe { type_name_of(o) }
    );
    unsafe { conversion_type_error(o, "PyNumber_Long", message) }
}

/// PyNumber_Int — alias for PyNumber_Long (Python 2 compat, still used).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Int(o: *mut PyObject) -> *mut PyObject {
    unsafe { PyNumber_Long(o) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Float(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        unsafe { ensure_exception_set() };
        return ptr::null_mut();
    }
    // Native Molt fast path.
    if let Some(bits) = resolve_bits(o) {
        let obj = MoltObject::from_bits(bits);
        if obj.is_float() {
            unsafe { crate::api::refcount::Py_INCREF(o) };
            return o;
        }
        if let Some(v) = as_f64(bits) {
            return pyobj_from_float(v);
        }
    }
    // Foreign object: dispatch to its `nb_float` slot (CPython
    // Objects/abstract.c `PyNumber_Float`). The `nb_index`-fallback CPython also
    // offers is intentionally omitted — every numeric foreign type Molt links
    // (numpy scalars, decimals) defines `nb_float`, and an honest TypeError for
    // the residual case is strictly better than the prior bare NULL.
    if let Some(result) = unsafe { call_number_unary_slot(o, NumberSlot::Float) } {
        return unsafe { finalize_slot_result(o, result, "PyNumber_Float") };
    }
    let message = format!(
        "float() argument must be a string or a real number, not '{}'",
        unsafe { type_name_of(o) }
    );
    unsafe { conversion_type_error(o, "PyNumber_Float", message) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Index(o: *mut PyObject) -> *mut PyObject {
    if o.is_null() {
        unsafe { ensure_exception_set() };
        return ptr::null_mut();
    }
    // Native Molt fast path.
    if let Some(bits) = resolve_bits(o) {
        let obj = MoltObject::from_bits(bits);
        if obj.is_int() {
            unsafe { crate::api::refcount::Py_INCREF(o) };
            return o;
        }
        if obj.is_bool()
            && let Some(b) = obj.as_bool()
        {
            return pyobj_from_int(b as i64);
        }
    }
    // Foreign object: dispatch to its `nb_index` slot only (CPython's
    // `_PyNumber_Index` never falls back to `nb_int`/`nb_float`).
    if let Some(result) = unsafe { call_number_unary_slot(o, NumberSlot::Index) } {
        return unsafe { finalize_slot_result(o, result, "PyNumber_Index") };
    }
    let message = format!(
        "'{}' object cannot be interpreted as an integer",
        unsafe { type_name_of(o) }
    );
    unsafe { conversion_type_error(o, "PyNumber_Index", message) }
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

#[cfg(test)]
mod conversion_slot_tests {
    use super::*;
    use crate::abi_types::{PyNumberMethods, PyObject, PyTypeObject};

    // Stand-in "converted integer" the fake `nb_int` slot hands back. Its
    // contents are irrelevant; the test only checks pointer identity.
    static mut FAKE_INT_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };

    unsafe extern "C" fn fake_nb_int(_o: *mut PyObject) -> *mut PyObject {
        &raw mut FAKE_INT_RESULT
    }

    /// A foreign object whose type exposes `nb_int` must have `PyNumber_Long`
    /// dispatch to that slot — the numpy-scalar path that previously returned a
    /// bare NULL and stranded `_multiarray_umath` init with the opaque
    /// "Py_mod_exec slot returned non-zero without setting an exception".
    #[test]
    fn py_number_long_dispatches_to_foreign_nb_int() {
        let _ = crate::capi_trace::take_last_silent_failure();
        let mut num: PyNumberMethods = unsafe { std::mem::zeroed() };
        num.nb_int = fake_nb_int as *mut c_void;
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_as_number = (&raw mut num).cast::<c_void>();
        ty.tp_name = c"fake_scalar".as_ptr();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let result = unsafe { PyNumber_Long(&raw mut obj) };
        assert_eq!(result, &raw mut FAKE_INT_RESULT);
    }

    /// A foreign object with no numeric conversion slot must never return a bare
    /// NULL: `PyNumber_Long` records the site on the permanent silent-failure
    /// surface and raises an honest `TypeError` (the C-API contract every caller
    /// — including numpy's reduction-identity packing — relies on).
    #[test]
    fn py_number_long_without_slot_is_never_a_silent_null() {
        let _ = crate::capi_trace::take_last_silent_failure();
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_name = c"opaque".as_ptr();
        // tp_as_number left NULL: neither nb_int nor nb_index is available.
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        let result = unsafe { PyNumber_Long(&raw mut obj) };
        assert!(result.is_null());
        let recorded = crate::capi_trace::take_last_silent_failure();
        assert!(
            recorded.as_deref().unwrap_or("").contains("PyNumber_Long"),
            "expected PyNumber_Long on the silent-failure surface, got {recorded:?}"
        );
        assert!(
            !unsafe { crate::api::errors::PyErr_Occurred() }.is_null(),
            "PyNumber_Long must leave a pending exception, never a bare NULL"
        );
        unsafe { crate::api::errors::PyErr_Clear() };
    }
}
