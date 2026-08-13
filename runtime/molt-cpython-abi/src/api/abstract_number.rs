//! Number abstract protocol — PyNumber_* operations.
//!
//! The single numeric authority lives in `molt-lang-runtime`: arbitrary-precision
//! int promotion, float coercion, operator-overload dispatch, and CPython-shaped
//! exception raising. This ABI layer MUST NOT reimplement arithmetic — an i64
//! reimplementation silently wraps Python's unbounded ints at 64 bits and masks
//! the exceptions CPython raises. Every `PyNumber_*` here resolves its operands
//! to runtime handle bits and delegates to that authority through the numeric
//! hooks (`number_binary_op` / `number_unary_op` / `number_power`).

use crate::abi_types::{Py_ssize_t, PyNumberMethods, PyObject, PyTypeObject};
use crate::bridge::{ResolvedPyObject, resolve_pyobject, resolved_molt_handle};
use crate::hooks::{NumberBinaryOp, NumberUnaryOp};
use molt_lang_obj_model::MoltObject;
use std::os::raw::{c_int, c_void};
use std::ptr;

/// Helper: resolve a PyObject to its Molt bits.
fn resolve_bits(op: *mut PyObject) -> Option<u64> {
    if op.is_null() {
        return None;
    }
    resolved_molt_handle(op).map(|value| value.bits())
}

struct ProtocolArg {
    ptr: *mut PyObject,
    owned: bool,
}

impl Drop for ProtocolArg {
    fn drop(&mut self) {
        if self.owned && !self.ptr.is_null() {
            unsafe {
                let rc = (*self.ptr).ob_refcnt;
                if rc > 1 {
                    (*self.ptr).ob_refcnt = rc - 1;
                } else {
                    let ty = (*self.ptr).ob_type;
                    if !ty.is_null()
                        && let Some(dealloc) = (*ty).tp_dealloc
                    {
                        dealloc(self.ptr);
                    }
                }
            }
        }
    }
}

unsafe fn protocol_arg(op: *mut PyObject) -> Option<ProtocolArg> {
    if !op.is_null() {
        let physical = unsafe { (*op).ob_type };
        if std::ptr::eq(physical, &raw const crate::abi_types::PyLong_Type)
            || std::ptr::eq(physical, &raw const crate::abi_types::PyBool_Type)
            || std::ptr::eq(physical, &raw const crate::abi_types::PyFloat_Type)
            || std::ptr::eq(physical, &raw const crate::abi_types::PyComplex_Type)
        {
            return Some(ProtocolArg {
                ptr: op,
                owned: false,
            });
        }
    }
    let arg = match resolve_pyobject(op) {
        Some(ResolvedPyObject::ManagedMolt(handle)) => {
            if crate::api::numbers::is_numeric_handle(handle.bits()) {
                let (ptr, owned) = unsafe {
                    crate::api::numbers::materialize_numeric_borrowed_handle(handle.bits())
                };
                ProtocolArg { ptr, owned }
            } else {
                ProtocolArg {
                    ptr: op,
                    owned: false,
                }
            }
        }
        _ => ProtocolArg {
            ptr: op,
            owned: false,
        },
    };
    if arg.ptr.is_null() {
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { crate::api::errors::PyErr_NoMemory() };
        }
        None
    } else {
        Some(arg)
    }
}

fn same_protocol_identity(a: *mut PyObject, b: *mut PyObject) -> bool {
    a == b
}

unsafe fn protocol_pair(a: *mut PyObject, b: *mut PyObject) -> Option<(ProtocolArg, ProtocolArg)> {
    let first = unsafe { protocol_arg(a) }?;
    let second = if same_protocol_identity(a, b) {
        ProtocolArg {
            ptr: first.ptr,
            owned: false,
        }
    } else {
        unsafe { protocol_arg(b) }?
    };
    Some((first, second))
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"PyNumber operation failed: runtime numeric authority unavailable".as_ptr(),
            );
        }
    }
}

/// Convert a numeric-result handle from the runtime authority into a PyObject*.
unsafe fn pyobj_from_result(result: crate::hooks::OwnedHandleResult) -> *mut PyObject {
    match result.decode() {
        crate::hooks::DecodedHandleResult::Ok(bits) => unsafe {
            crate::bridge::molt_capi_result_to_pyobj(bits)
        },
        crate::hooks::DecodedHandleResult::Missing | crate::hooks::DecodedHandleResult::Error => {
            unsafe { ensure_exception_set() };
            ptr::null_mut()
        }
    }
}

type BinaryFunc = unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> *mut PyObject;
type UnaryFunc = unsafe extern "C" fn(*mut PyObject) -> *mut PyObject;
type TernaryFunc =
    unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject;

#[derive(Clone, Copy)]
pub(crate) enum BinarySlot {
    Add,
    Subtract,
    Multiply,
    Remainder,
    Divmod,
    Lshift,
    Rshift,
    And,
    Xor,
    Or,
    FloorDivide,
    TrueDivide,
    MatrixMultiply,
}

#[derive(Clone, Copy)]
pub(crate) enum InPlaceSlot {
    Add,
    Subtract,
    Multiply,
    Remainder,
    Lshift,
    Rshift,
    And,
    Xor,
    Or,
    FloorDivide,
    TrueDivide,
}

unsafe fn number_methods(o: *mut PyObject) -> *mut PyNumberMethods {
    if o.is_null() {
        return ptr::null_mut();
    }
    let ty = unsafe { (*o).ob_type };
    if ty.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*ty).tp_as_number }.cast::<PyNumberMethods>()
}

unsafe fn binary_slot(o: *mut PyObject, slot: BinarySlot) -> *mut c_void {
    let methods = unsafe { number_methods(o) };
    if methods.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        match slot {
            BinarySlot::Add => (*methods).nb_add,
            BinarySlot::Subtract => (*methods).nb_subtract,
            BinarySlot::Multiply => (*methods).nb_multiply,
            BinarySlot::Remainder => (*methods).nb_remainder,
            BinarySlot::Divmod => (*methods).nb_divmod,
            BinarySlot::Lshift => (*methods).nb_lshift,
            BinarySlot::Rshift => (*methods).nb_rshift,
            BinarySlot::And => (*methods).nb_and,
            BinarySlot::Xor => (*methods).nb_xor,
            BinarySlot::Or => (*methods).nb_or,
            BinarySlot::FloorDivide => (*methods).nb_floor_divide,
            BinarySlot::TrueDivide => (*methods).nb_true_divide,
            BinarySlot::MatrixMultiply => (*methods).nb_matrix_multiply,
        }
    }
}

unsafe fn inplace_slot(o: *mut PyObject, slot: InPlaceSlot) -> *mut c_void {
    let methods = unsafe { number_methods(o) };
    if methods.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        match slot {
            InPlaceSlot::Add => (*methods).nb_inplace_add,
            InPlaceSlot::Subtract => (*methods).nb_inplace_subtract,
            InPlaceSlot::Multiply => (*methods).nb_inplace_multiply,
            InPlaceSlot::Remainder => (*methods).nb_inplace_remainder,
            InPlaceSlot::Lshift => (*methods).nb_inplace_lshift,
            InPlaceSlot::Rshift => (*methods).nb_inplace_rshift,
            InPlaceSlot::And => (*methods).nb_inplace_and,
            InPlaceSlot::Xor => (*methods).nb_inplace_xor,
            InPlaceSlot::Or => (*methods).nb_inplace_or,
            InPlaceSlot::FloorDivide => (*methods).nb_inplace_floor_divide,
            InPlaceSlot::TrueDivide => (*methods).nb_inplace_true_divide,
        }
    }
}

pub(crate) fn is_not_implemented(result: *mut PyObject) -> bool {
    ptr::eq(result, &raw mut crate::abi_types::Py_NotImplementedSentinel)
}

pub(crate) unsafe fn discard_not_implemented(result: *mut PyObject) {
    unsafe { crate::api::refcount::Py_DECREF(result) };
}

unsafe fn binop_type_error(o1: *mut PyObject, o2: *mut PyObject, op_name: &str) -> *mut PyObject {
    let message = format!(
        "unsupported operand type(s) for {op_name}: '{}' and '{}'",
        unsafe { type_name_of(o1) },
        unsafe { type_name_of(o2) }
    );
    let message = std::ffi::CString::new(message).expect("operator error contains no NUL");
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
            message.as_ptr(),
        )
    };
    ptr::null_mut()
}

unsafe fn call_binary_func(
    slot: *mut c_void,
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    let func: BinaryFunc = unsafe { std::mem::transmute(slot) };
    unsafe { func(o1, o2) }
}

pub(crate) unsafe fn foreign_binary_op1(
    slot: BinarySlot,
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    let type1: *mut PyTypeObject = unsafe { (*o1).ob_type };
    let type2: *mut PyTypeObject = unsafe { (*o2).ob_type };
    let slot1 = unsafe { binary_slot(o1, slot) };
    let mut slot2 = if !ptr::eq(type1, type2) {
        unsafe { binary_slot(o2, slot) }
    } else {
        ptr::null_mut()
    };
    if slot2 == slot1 {
        slot2 = ptr::null_mut();
    }

    if !slot1.is_null() {
        if !slot2.is_null() && unsafe { crate::api::typeobj::PyType_IsSubtype(type2, type1) } == 1 {
            let result = unsafe { call_binary_func(slot2, o1, o2) };
            if result.is_null() || !is_not_implemented(result) {
                return result;
            }
            unsafe { discard_not_implemented(result) };
            slot2 = ptr::null_mut();
        }
        let result = unsafe { call_binary_func(slot1, o1, o2) };
        if result.is_null() || !is_not_implemented(result) {
            return result;
        }
        unsafe { discard_not_implemented(result) };
    }
    if !slot2.is_null() {
        let result = unsafe { call_binary_func(slot2, o1, o2) };
        if result.is_null() || !is_not_implemented(result) {
            return result;
        }
        unsafe { discard_not_implemented(result) };
    }
    let result = &raw mut crate::abi_types::Py_NotImplementedSentinel;
    unsafe { crate::api::refcount::Py_INCREF(result) };
    result
}

unsafe fn foreign_binary_op(
    slot: BinarySlot,
    op_name: &str,
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    let result = unsafe { foreign_binary_op1(slot, o1, o2) };
    if is_not_implemented(result) {
        unsafe { discard_not_implemented(result) };
        return unsafe { binop_type_error(o1, o2, op_name) };
    }
    result
}

/// CPython `BINARY_IOP1`: try the left operand's in-place numeric slot, then
/// the canonical reflected-aware binary dispatch. `PySequence_InPlace*` shares
/// this authority with `PyNumber_InPlace*`; callers own the returned reference,
/// including the `NotImplemented` sentinel.
pub(crate) unsafe fn foreign_binary_iop1(
    inplace: InPlaceSlot,
    binary: BinarySlot,
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    let slot = unsafe { inplace_slot(o1, inplace) };
    if !slot.is_null() {
        let result = unsafe { call_binary_func(slot, o1, o2) };
        if result.is_null() || !is_not_implemented(result) {
            return result;
        }
        unsafe { discard_not_implemented(result) };
    }
    unsafe { foreign_binary_op1(binary, o1, o2) }
}

unsafe fn power_slot(o: *mut PyObject) -> *mut c_void {
    let methods = unsafe { number_methods(o) };
    if methods.is_null() {
        ptr::null_mut()
    } else {
        unsafe { (*methods).nb_power }
    }
}

unsafe fn call_ternary_func(
    slot: *mut c_void,
    o1: *mut PyObject,
    o2: *mut PyObject,
    o3: *mut PyObject,
) -> *mut PyObject {
    let func: TernaryFunc = unsafe { std::mem::transmute(slot) };
    unsafe { func(o1, o2, o3) }
}

unsafe fn foreign_power(o1: *mut PyObject, o2: *mut PyObject, o3: *mut PyObject) -> *mut PyObject {
    let type1 = unsafe { (*o1).ob_type };
    let type2 = unsafe { (*o2).ob_type };
    let slot1 = unsafe { power_slot(o1) };
    let mut slot2 = if !ptr::eq(type1, type2) {
        unsafe { power_slot(o2) }
    } else {
        ptr::null_mut()
    };
    if slot2 == slot1 {
        slot2 = ptr::null_mut();
    }
    if !slot1.is_null() {
        if !slot2.is_null() && unsafe { crate::api::typeobj::PyType_IsSubtype(type2, type1) } == 1 {
            let result = unsafe { call_ternary_func(slot2, o1, o2, o3) };
            if result.is_null() || !is_not_implemented(result) {
                return result;
            }
            unsafe { discard_not_implemented(result) };
            slot2 = ptr::null_mut();
        }
        let result = unsafe { call_ternary_func(slot1, o1, o2, o3) };
        if result.is_null() || !is_not_implemented(result) {
            return result;
        }
        unsafe { discard_not_implemented(result) };
    }
    if !slot2.is_null() {
        let result = unsafe { call_ternary_func(slot2, o1, o2, o3) };
        if result.is_null() || !is_not_implemented(result) {
            return result;
        }
        unsafe { discard_not_implemented(result) };
    }
    if !o3.is_null() {
        let slot3 = unsafe { power_slot(o3) };
        if !slot3.is_null() && slot3 != slot1 && slot3 != slot2 {
            let result = unsafe { call_ternary_func(slot3, o1, o2, o3) };
            if result.is_null() || !is_not_implemented(result) {
                return result;
            }
            unsafe { discard_not_implemented(result) };
        }
    }
    unsafe { binop_type_error(o1, o2, "** or pow()") }
}

unsafe fn inplace_binary_op(
    inplace: InPlaceSlot,
    binary: BinarySlot,
    op_name: &str,
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    if resolve_bits(o1).is_some() && resolve_bits(o2).is_some() {
        return unsafe {
            match binary {
                BinarySlot::Add => PyNumber_Add(o1, o2),
                BinarySlot::Subtract => PyNumber_Subtract(o1, o2),
                BinarySlot::Multiply => PyNumber_Multiply(o1, o2),
                BinarySlot::Remainder => PyNumber_Remainder(o1, o2),
                BinarySlot::Lshift => PyNumber_Lshift(o1, o2),
                BinarySlot::Rshift => PyNumber_Rshift(o1, o2),
                BinarySlot::And => PyNumber_And(o1, o2),
                BinarySlot::Xor => PyNumber_Xor(o1, o2),
                BinarySlot::Or => PyNumber_Or(o1, o2),
                BinarySlot::FloorDivide => PyNumber_FloorDivide(o1, o2),
                BinarySlot::TrueDivide => PyNumber_TrueDivide(o1, o2),
                BinarySlot::Divmod | BinarySlot::MatrixMultiply => unreachable!(),
            }
        };
    }
    let Some((p1, p2)) = (unsafe { protocol_pair(o1, o2) }) else {
        return ptr::null_mut();
    };
    let result = unsafe { foreign_binary_iop1(inplace, binary, p1.ptr, p2.ptr) };
    if is_not_implemented(result) {
        unsafe { discard_not_implemented(result) };
        return unsafe { binop_type_error(p1.ptr, p2.ptr, op_name) };
    }
    result
}

/// Dispatch a binary numeric op through the runtime authority.
unsafe fn binary_op(op: NumberBinaryOp, o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    let (Some(a), Some(b)) = (resolve_bits(o1), resolve_bits(o2)) else {
        let (slot, op_name) = match op {
            NumberBinaryOp::Add => (BinarySlot::Add, "+"),
            NumberBinaryOp::Subtract => (BinarySlot::Subtract, "-"),
            NumberBinaryOp::Multiply => (BinarySlot::Multiply, "*"),
            NumberBinaryOp::TrueDivide => (BinarySlot::TrueDivide, "/"),
            NumberBinaryOp::FloorDivide => (BinarySlot::FloorDivide, "//"),
            NumberBinaryOp::Remainder => (BinarySlot::Remainder, "%"),
            NumberBinaryOp::Lshift => (BinarySlot::Lshift, "<<"),
            NumberBinaryOp::Rshift => (BinarySlot::Rshift, ">>"),
            NumberBinaryOp::And => (BinarySlot::And, "&"),
            NumberBinaryOp::Or => (BinarySlot::Or, "|"),
            NumberBinaryOp::Xor => (BinarySlot::Xor, "^"),
            NumberBinaryOp::MatrixMultiply => (BinarySlot::MatrixMultiply, "@"),
        };
        let Some((p1, p2)) = (unsafe { protocol_pair(o1, o2) }) else {
            return ptr::null_mut();
        };
        return unsafe { foreign_binary_op(slot, op_name, p1.ptr, p2.ptr) };
    };
    let h = crate::hooks::hooks_or_stubs();
    let result = unsafe { (h.number_binary_op)(op as u32, a, b) };
    unsafe { pyobj_from_result(result) }
}

/// Dispatch a unary numeric op through the runtime authority.
unsafe fn unary_op(op: NumberUnaryOp, o: *mut PyObject) -> *mut PyObject {
    let Some(a) = resolve_bits(o) else {
        let methods = unsafe { number_methods(o) };
        let (slot, op_name) = if methods.is_null() {
            (ptr::null_mut(), "")
        } else {
            unsafe {
                match op {
                    NumberUnaryOp::Negative => ((*methods).nb_negative, "unary -"),
                    NumberUnaryOp::Positive => ((*methods).nb_positive, "unary +"),
                    NumberUnaryOp::Absolute => ((*methods).nb_absolute, "abs()"),
                    NumberUnaryOp::Invert => ((*methods).nb_invert, "unary ~"),
                }
            }
        };
        if !slot.is_null() {
            let func: UnaryFunc = unsafe { std::mem::transmute(slot) };
            return unsafe { func(o) };
        }
        let message = format!("bad operand type for {op_name}: '{}'", unsafe {
            type_name_of(o)
        });
        let message = std::ffi::CString::new(message).expect("unary error contains no NUL");
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                message.as_ptr(),
            )
        };
        return ptr::null_mut();
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
fn is_runtime_int(bits: u64) -> bool {
    let obj = MoltObject::from_bits(bits);
    obj.is_int()
        || obj.is_bool()
        || obj.is_ptr()
            && unsafe { (crate::hooks::hooks_or_stubs().classify_heap)(bits) }
                == crate::abi_types::MoltTypeTag::Int as u8
}

/// Helper: build a PyObject from a float result.
fn pyobj_from_float(v: f64) -> *mut PyObject {
    unsafe { crate::api::numbers::PyFloat_FromDouble(v) }
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
            return unsafe { crate::bridge::molt_capi_result_to_pyobj(inline.bits()) };
        }
        unsafe { ensure_exception_set() };
        return ptr::null_mut();
    }
    unsafe { crate::bridge::molt_capi_result_to_pyobj(bits) }
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
    if resolve_bits(o1).is_none()
        || resolve_bits(o2).is_none()
        || (!o3.is_null() && resolve_bits(o3).is_none())
    {
        let Some((p1, p2)) = (unsafe { protocol_pair(o1, o2) }) else {
            return ptr::null_mut();
        };
        let p3 = if o3.is_null() {
            ProtocolArg {
                ptr: o3,
                owned: false,
            }
        } else if same_protocol_identity(o1, o3) {
            ProtocolArg {
                ptr: p1.ptr,
                owned: false,
            }
        } else if same_protocol_identity(o2, o3) {
            ProtocolArg {
                ptr: p2.ptr,
                owned: false,
            }
        } else {
            let Some(value) = (unsafe { protocol_arg(o3) }) else {
                return ptr::null_mut();
            };
            value
        };
        return unsafe { foreign_power(p1.ptr, p2.ptr, p3.ptr) };
    }
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
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"numeric conversion slot returned NULL without setting an exception".as_ptr(),
            );
        }
    }
    result
}

/// No native or foreign conversion applied: record the site on the permanent
/// silent-failure surface and raise the CPython-shaped `TypeError`, so a failing
/// conversion is never a bare NULL (the C-API contract every caller relies on).
unsafe fn conversion_type_error(
    o: *mut PyObject,
    capi_name: &str,
    message: String,
) -> *mut PyObject {
    crate::capi_trace::record_silent_failure(capi_name, Some(&unsafe { type_name_of(o) }));
    if !conversion_exception_pending()
        && let Ok(cmsg) = std::ffi::CString::new(message)
    {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
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
    let physical = unsafe { (*o).ob_type };
    if std::ptr::eq(physical, &raw const crate::abi_types::PyLong_Type) {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    if std::ptr::eq(physical, &raw const crate::abi_types::PyBool_Type) {
        return pyobj_from_int(
            (o == (&raw mut crate::abi_types::Py_True).cast::<PyObject>()) as i64,
        );
    }
    // Native Molt fast path.
    if let Some(bits) = resolve_bits(o) {
        let obj = MoltObject::from_bits(bits);
        if obj.is_bool() {
            return pyobj_from_int(obj.as_bool().unwrap_or(false) as i64);
        }
        if is_runtime_int(bits) {
            return unsafe { crate::api::numbers::materialize_numeric_borrowed_handle(bits).0 };
        }
        if obj.is_float()
            && let Some(v) = obj.as_float()
        {
            return unsafe { crate::api::numbers::PyLong_FromDouble(v) };
        }
    }
    if unsafe { crate::api::numbers::PyLong_Check(o) } != 0 {
        return unsafe { crate::api::numbers::copy_layout_long_to_exact(o) };
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
    let physical = unsafe { (*o).ob_type };
    if std::ptr::eq(physical, &raw const crate::abi_types::PyFloat_Type) {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    // Native Molt fast path.
    if let Some(bits) = resolve_bits(o) {
        let obj = MoltObject::from_bits(bits);
        if obj.is_float() {
            return pyobj_from_float(obj.as_float().unwrap_or(0.0));
        }
        if let Some(v) = as_f64(bits) {
            return pyobj_from_float(v);
        }
        if is_runtime_int(bits) {
            let carrier =
                unsafe { crate::api::numbers::materialize_numeric_borrowed_handle(bits).0 };
            if carrier.is_null() {
                return ptr::null_mut();
            }
            let value = unsafe { crate::api::numbers::PyLong_AsDouble(carrier) };
            unsafe { crate::api::refcount::Py_DECREF(carrier) };
            if value == -1.0 && conversion_exception_pending() {
                return ptr::null_mut();
            }
            return pyobj_from_float(value);
        }
    }
    if unsafe { crate::api::numbers::PyFloat_Check(o) } != 0 {
        let value = unsafe { crate::api::numbers::PyFloat_AsDouble(o) };
        if value == -1.0 && conversion_exception_pending() {
            return ptr::null_mut();
        }
        return pyobj_from_float(value);
    }
    // Foreign object: dispatch to its `nb_float` slot (CPython
    // Objects/abstract.c `PyNumber_Float`). The `nb_index`-fallback CPython also
    // offers is intentionally omitted — every numeric foreign type Molt links
    // (numpy scalars, decimals) defines `nb_float`, and an honest TypeError for
    // the residual case is strictly better than the prior bare NULL.
    if let Some(result) = unsafe { call_number_unary_slot(o, NumberSlot::Float) } {
        return unsafe { finalize_slot_result(o, result, "PyNumber_Float") };
    }
    if let Some(index) = unsafe { call_number_unary_slot(o, NumberSlot::Index) } {
        let index = unsafe { finalize_slot_result(o, index, "PyNumber_Float") };
        if index.is_null() {
            return ptr::null_mut();
        }
        let value = unsafe { crate::api::numbers::PyLong_AsDouble(index) };
        unsafe { crate::api::refcount::Py_DECREF(index) };
        if value == -1.0 && conversion_exception_pending() {
            return ptr::null_mut();
        }
        return pyobj_from_float(value);
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
    let physical = unsafe { (*o).ob_type };
    if std::ptr::eq(physical, &raw const crate::abi_types::PyLong_Type) {
        unsafe { crate::api::refcount::Py_INCREF(o) };
        return o;
    }
    if std::ptr::eq(physical, &raw const crate::abi_types::PyBool_Type) {
        return pyobj_from_int(
            (o == (&raw mut crate::abi_types::Py_True).cast::<PyObject>()) as i64,
        );
    }
    // Native Molt fast path.
    if let Some(bits) = resolve_bits(o) {
        let obj = MoltObject::from_bits(bits);
        if obj.is_bool() {
            return pyobj_from_int(obj.as_bool().unwrap_or(false) as i64);
        }
        if is_runtime_int(bits) {
            return unsafe { crate::api::numbers::materialize_numeric_borrowed_handle(bits).0 };
        }
    }
    if unsafe { crate::api::numbers::PyLong_Check(o) } != 0 {
        return unsafe { crate::api::numbers::copy_layout_long_to_exact(o) };
    }
    // Foreign object: dispatch to its `nb_index` slot only (CPython's
    // `_PyNumber_Index` never falls back to `nb_int`/`nb_float`).
    if let Some(result) = unsafe { call_number_unary_slot(o, NumberSlot::Index) } {
        return unsafe { finalize_slot_result(o, result, "PyNumber_Index") };
    }
    let message = format!("'{}' object cannot be interpreted as an integer", unsafe {
        type_name_of(o)
    });
    unsafe { conversion_type_error(o, "PyNumber_Index", message) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyIndex_Check(o: *mut PyObject) -> c_int {
    // Native Molt integers/bools are indices.
    if let Some(bits) = resolve_bits(o) {
        if is_runtime_int(bits) {
            return 1;
        }
        return 0;
    }
    // Foreign object: CPython's `PyIndex_Check` tests `tp_as_number->nb_index`
    // (numpy integer scalars define it). A native non-integer (float) has no
    // `nb_index` slot and correctly yields 0 — the prior code answered 0 for
    // every foreign object, stranding numpy's index-checks on its own scalars.
    if o.is_null() {
        return 0;
    }
    let tp = unsafe { (*o).ob_type };
    if tp.is_null() {
        return 0;
    }
    let num = unsafe { (*tp).tp_as_number }.cast::<crate::abi_types::PyNumberMethods>();
    (!num.is_null() && !unsafe { (*num).nb_index }.is_null()) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_AsSsize_t(o: *mut PyObject, exc: *mut PyObject) -> Py_ssize_t {
    // Otherwise reduce through `PyNumber_Index` (which now dispatches the
    // object's `nb_index` slot — e.g. a numpy integer scalar) and read the
    // ssize_t, as CPython's `PyNumber_AsSsize_t` does (Objects/abstract.c). A
    // failed reduction leaves a pending exception; we propagate the -1 sentinel
    // instead of the prior bare -1 that carried none. (`_exc` overflow-clamp
    // translation is a no-op here: ssize_t is i64 on wasm32 and PyLong_AsSsize_t
    // already raises an honest OverflowError for a genuine out-of-range value.)
    let index = unsafe { PyNumber_Index(o) };
    if index.is_null() {
        return -1;
    }
    let value = unsafe { crate::api::numbers::PyLong_AsSsize_t(index) };
    if value == -1
        && unsafe {
            crate::api::errors::PyErr_ExceptionMatches(
                (&raw mut crate::abi_types::PyExc_OverflowError)
                    .cast::<crate::abi_types::PyObject>(),
            )
        } != 0
    {
        let sign = unsafe { crate::api::numbers::_PyLong_Sign(index) };
        unsafe { crate::api::errors::PyErr_Clear() };
        if exc.is_null() {
            unsafe { crate::api::refcount::Py_DECREF(index) };
            return if sign < 0 {
                Py_ssize_t::MIN
            } else {
                Py_ssize_t::MAX
            };
        }
        unsafe {
            crate::api::errors::PyErr_SetString(
                exc,
                c"cannot fit 'int' into an index-sized integer".as_ptr(),
            )
        };
    }
    unsafe { crate::api::refcount::Py_DECREF(index) };
    value
}

// ─── In-place operations (return new object, same semantics) ─────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceAdd(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::Add, BinarySlot::Add, "+=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceSubtract(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::Subtract, BinarySlot::Subtract, "-=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceMultiply(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::Multiply, BinarySlot::Multiply, "*=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceTrueDivide(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        inplace_binary_op(
            InPlaceSlot::TrueDivide,
            BinarySlot::TrueDivide,
            "/=",
            o1,
            o2,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceFloorDivide(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        inplace_binary_op(
            InPlaceSlot::FloorDivide,
            BinarySlot::FloorDivide,
            "//=",
            o1,
            o2,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceRemainder(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::Remainder, BinarySlot::Remainder, "%=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceLshift(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::Lshift, BinarySlot::Lshift, "<<=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceRshift(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::Rshift, BinarySlot::Rshift, ">>=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceAnd(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::And, BinarySlot::And, "&=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceOr(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::Or, BinarySlot::Or, "|=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceXor(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { inplace_binary_op(InPlaceSlot::Xor, BinarySlot::Xor, "^=", o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlacePower(
    o1: *mut PyObject,
    o2: *mut PyObject,
    o3: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_Power(o1, o2, o3) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_InPlaceMatrixMultiply(
    o1: *mut PyObject,
    o2: *mut PyObject,
) -> *mut PyObject {
    unsafe { PyNumber_MatrixMultiply(o1, o2) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Divmod(o1: *mut PyObject, o2: *mut PyObject) -> *mut PyObject {
    if resolve_bits(o1).is_none() || resolve_bits(o2).is_none() {
        let Some((p1, p2)) = (unsafe { protocol_pair(o1, o2) }) else {
            return ptr::null_mut();
        };
        return unsafe { foreign_binary_op(BinarySlot::Divmod, "divmod()", p1.ptr, p2.ptr) };
    }
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
    static mut ADD_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    static mut BASE_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    static mut SUBCLASS_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    static mut INPLACE_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };

    unsafe extern "C" fn fake_nb_int(_o: *mut PyObject) -> *mut PyObject {
        &raw mut FAKE_INT_RESULT
    }

    unsafe extern "C" fn fake_nb_add(_left: *mut PyObject, _right: *mut PyObject) -> *mut PyObject {
        &raw mut ADD_RESULT
    }

    unsafe extern "C" fn base_nb_add(_left: *mut PyObject, _right: *mut PyObject) -> *mut PyObject {
        &raw mut BASE_RESULT
    }

    unsafe extern "C" fn subclass_nb_add(
        _left: *mut PyObject,
        _right: *mut PyObject,
    ) -> *mut PyObject {
        &raw mut SUBCLASS_RESULT
    }

    unsafe extern "C" fn fake_nb_inplace_add(
        _left: *mut PyObject,
        _right: *mut PyObject,
    ) -> *mut PyObject {
        &raw mut INPLACE_RESULT
    }

    unsafe extern "C" fn not_implemented_nb_add(
        _left: *mut PyObject,
        _right: *mut PyObject,
    ) -> *mut PyObject {
        let result = &raw mut crate::abi_types::Py_NotImplementedSentinel;
        unsafe { crate::api::refcount::Py_INCREF(result) };
        result
    }

    unsafe fn foreign_object(
        name: &'static std::ffi::CStr,
        methods: *mut PyNumberMethods,
        base: *mut PyTypeObject,
    ) -> (PyTypeObject, PyObject) {
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_as_number = methods.cast::<c_void>();
        ty.tp_name = name.as_ptr();
        ty.tp_base = base;
        let obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        (ty, obj)
    }

    #[test]
    fn py_number_add_dispatches_to_foreign_nb_add() {
        let mut methods: PyNumberMethods = unsafe { std::mem::zeroed() };
        methods.nb_add = fake_nb_add as *mut c_void;
        let (mut ty, mut left) =
            unsafe { foreign_object(c"foreign_add", &raw mut methods, ptr::null_mut()) };
        left.ob_type = &raw mut ty;
        let mut right = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };

        let result = unsafe { PyNumber_Add(&raw mut left, &raw mut right) };
        assert_eq!(result, &raw mut ADD_RESULT);
    }

    #[test]
    fn py_number_add_prioritizes_subclass_reflected_slot() {
        let mut base_methods: PyNumberMethods = unsafe { std::mem::zeroed() };
        base_methods.nb_add = base_nb_add as *mut c_void;
        let (mut base_ty, mut left) =
            unsafe { foreign_object(c"base_number", &raw mut base_methods, ptr::null_mut()) };
        left.ob_type = &raw mut base_ty;

        let mut subclass_methods: PyNumberMethods = unsafe { std::mem::zeroed() };
        subclass_methods.nb_add = subclass_nb_add as *mut c_void;
        let (mut subclass_ty, mut right) =
            unsafe { foreign_object(c"sub_number", &raw mut subclass_methods, &raw mut base_ty) };
        right.ob_type = &raw mut subclass_ty;

        let result = unsafe { PyNumber_Add(&raw mut left, &raw mut right) };
        assert_eq!(result, &raw mut SUBCLASS_RESULT);
    }

    #[test]
    fn py_number_inplace_add_calls_nb_inplace_add() {
        let mut methods: PyNumberMethods = unsafe { std::mem::zeroed() };
        methods.nb_add = fake_nb_add as *mut c_void;
        methods.nb_inplace_add = fake_nb_inplace_add as *mut c_void;
        let (mut ty, mut left) =
            unsafe { foreign_object(c"foreign_iadd", &raw mut methods, ptr::null_mut()) };
        left.ob_type = &raw mut ty;
        let mut right = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };

        let result = unsafe { PyNumber_InPlaceAdd(&raw mut left, &raw mut right) };
        assert_eq!(result, &raw mut INPLACE_RESULT);
    }

    #[test]
    fn py_number_add_not_implemented_raises_cpython_type_error() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        unsafe { crate::api::errors::PyErr_Clear() };
        let mut methods: PyNumberMethods = unsafe { std::mem::zeroed() };
        methods.nb_add = not_implemented_nb_add as *mut c_void;
        let (mut left_ty, mut left) =
            unsafe { foreign_object(c"left_number", &raw mut methods, ptr::null_mut()) };
        left.ob_type = &raw mut left_ty;
        let (mut right_ty, mut right) =
            unsafe { foreign_object(c"right_number", ptr::null_mut(), ptr::null_mut()) };
        right.ob_type = &raw mut right_ty;

        let result = unsafe { PyNumber_Add(&raw mut left, &raw mut right) };
        assert!(result.is_null());
        assert_eq!(
            unsafe {
                crate::api::errors::PyErr_ExceptionMatches(
                    (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
                )
            },
            1
        );
        unsafe { crate::api::errors::PyErr_Clear() };
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
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
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
