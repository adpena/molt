//! Numeric type bridge — PyLong_*, PyFloat_*, PyBool_*.

use crate::abi_types::{Py_False, Py_True, Py_complex, Py_ssize_t, PyComplexObject, PyObject};
use crate::bridge::GLOBAL_BRIDGE;
use crate::hooks::hooks_or_stubs;
use molt_lang_obj_model::MoltObject;
use std::ffi::c_void;
use std::os::raw::{c_char, c_double, c_int, c_long, c_longlong, c_ulong, c_ulonglong};
use std::ptr;

// ─── PyLong ──────────────────────────────────────────────────────────────────

fn py_long_from_i64(v: i64) -> *mut PyObject {
    let bits = MoltObject::try_from_int(v)
        .map(MoltObject::bits)
        .unwrap_or_else(|| unsafe { (hooks_or_stubs().int_from_i64)(v) });
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) }
}

fn py_long_from_u64(v: u64) -> *mut PyObject {
    let bits = MoltObject::try_from_uint(v)
        .map(MoltObject::bits)
        .unwrap_or_else(|| unsafe { (hooks_or_stubs().int_from_u64)(v) });
    if bits == 0 {
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) }
}

// ─── Checked PyLong_As* conversion core ──────────────────────────────────────
//
// CPython's `PyLong_As*` family has an exact three-part contract
// (Objects/longobject.c):
//   1. non-int input either dispatches `__index__` (`AsLong`/`AsLongLong`/
//      `*AndOverflow`, via `_PyNumber_Index`) or raises TypeError
//      "an integer is required" (`AsSsize_t`/`AsUnsigned*`, `PyLong_Check` only);
//   2. an int outside the C target range raises OverflowError with a
//      width-specific message ("Python int too large to convert to C <type>");
//   3. `-1` is returned ONLY with an exception set — never as a bare sentinel a
//      caller could mistake for the value -1.
// The pre-fix `py_long_as_i64` violated all three (silent -1, silent
// truncation, no `__index__`), which numpy consumed as real shape/index values.

/// A successfully resolved integer value.
enum LongValue {
    Signed(i64),
    /// In `(i64::MAX, u64::MAX]` — representable only as unsigned 64-bit.
    Big(u64),
    Wide {
        bits: u64,
        low_u64: Option<u64>,
        sign: i8,
    },
}

/// Why a resolution failed. `Raised` means a CPython-shaped exception is
/// already pending (set by `__index__` dispatch or the NULL guard).
enum LongError {
    /// Not an integer, and `__index__` was not consulted (strict mode).
    NotInt,
    /// A genuine int whose magnitude exceeds the 64-bit hook envelope
    /// (`< i64::MIN` or `> u64::MAX`). For every C target of ≤ 64 bits this is
    /// exactly CPython's OverflowError case.
    /// Exception already set; propagate the sentinel.
    Raised,
}

/// Resolve `op` to a 64-bit integer through the verified paths: inline int,
/// bool (an int subtype), heap BigInt via the checked runtime hooks, then —
/// when `use_index` (the `AsLong`/`AsLongLong` family) — the `__index__`
/// protocol via `PyNumber_Index`. NULL raises SystemError (PyErr_BadInternalCall)
/// exactly like CPython's NULL guard.
fn py_long_value(op: *mut PyObject, use_index: bool) -> Result<LongValue, LongError> {
    if op.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return Err(LongError::Raised);
    }
    let op_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
    if let Some(value) = op_handle {
        let bits = value.bits();
        let obj = value.decode();
        if let Some(v) = obj.as_int() {
            return Ok(LongValue::Signed(v));
        }
        if obj.is_bool() {
            return Ok(LongValue::Signed(obj.as_bool().unwrap_or(false) as i64));
        }
        if obj.is_ptr() {
            let h = hooks_or_stubs();
            if unsafe { (h.classify_heap)(bits) } == crate::abi_types::MoltTypeTag::Int as u8 {
                let mut sv = 0i64;
                if unsafe { (h.int_as_i64_checked)(bits, &raw mut sv) } == 0 {
                    return Ok(LongValue::Signed(sv));
                }
                let mut uv = 0u64;
                if unsafe { (h.int_as_u64_checked)(bits, &raw mut uv) } == 0 {
                    return Ok(LongValue::Big(uv));
                }
                // A genuine int beyond ±2^64: overflow for any ≤64-bit target.
                let Some(sign) = big_int_sign(bits).map(|sign| if sign < 0 { -1 } else { 1 })
                else {
                    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                        set_long_overflow_msg(c"runtime integer sign authority unavailable");
                    }
                    return Err(LongError::Raised);
                };
                return Ok(LongValue::Wide {
                    bits,
                    low_u64: None,
                    sign,
                });
            }
        }
    }
    if use_index {
        // `_PyNumber_Index` dispatch: raises the CPython-shaped TypeError
        // ("'X' object cannot be interpreted as an integer") on failure.
        let index = unsafe { crate::api::abstract_number::PyNumber_Index(op) };
        if index.is_null() {
            return Err(LongError::Raised);
        }
        // The result is a real int; re-resolve WITHOUT __index__ (no loops).
        let mut result = py_long_value(index, false);
        if let Ok(LongValue::Wide {
            bits,
            low_u64: None,
            sign,
        }) = &result
        {
            let mut low_u64 = 0u64;
            if unsafe { (hooks_or_stubs().int_as_u64_mask)(*bits, 64, &raw mut low_u64) } == 0 {
                result = Ok(LongValue::Wide {
                    bits: *bits,
                    low_u64: Some(low_u64),
                    sign: *sign,
                });
            }
        }
        unsafe { crate::api::refcount::Py_DECREF(index) };
        return match result {
            Err(LongError::NotInt) => {
                // An `__index__` that produced a non-int: fail loud.
                set_long_type_error();
                Err(LongError::Raised)
            }
            other => other,
        };
    }
    Err(LongError::NotInt)
}

/// TypeError "an integer is required" — the strict (`PyLong_Check`-only)
/// non-int rejection shared by `AsSsize_t`/`AsUnsigned*` (Objects/longobject.c).
fn set_long_type_error() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_TypeError,
            c"an integer is required".as_ptr(),
        );
    }
}

/// OverflowError with the exact CPython width message.
fn set_long_overflow_msg(message: &'static std::ffi::CStr) {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_OverflowError,
            message.as_ptr(),
        );
    }
}

enum CheckedLongError {
    NotInt,
    Raised,
    Negative,
    Overflow(i8),
}

fn checked_signed_value(
    op: *mut PyObject,
    use_index: bool,
    min: i64,
    max: i64,
) -> Result<i64, CheckedLongError> {
    match py_long_value(op, use_index) {
        Ok(LongValue::Signed(value)) if value >= min && value <= max => Ok(value),
        Ok(LongValue::Signed(value)) => {
            Err(CheckedLongError::Overflow(if value < 0 { -1 } else { 1 }))
        }
        Ok(LongValue::Big(_)) => Err(CheckedLongError::Overflow(1)),
        Ok(LongValue::Wide { sign, .. }) => Err(CheckedLongError::Overflow(sign)),
        Err(LongError::NotInt) => Err(CheckedLongError::NotInt),
        Err(LongError::Raised) => Err(CheckedLongError::Raised),
    }
}

fn checked_unsigned_value(op: *mut PyObject, max: u64) -> Result<u64, CheckedLongError> {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(value)) if value < 0 => Err(CheckedLongError::Negative),
        Ok(LongValue::Signed(value)) if value as u64 <= max => Ok(value as u64),
        Ok(LongValue::Signed(_)) => Err(CheckedLongError::Overflow(1)),
        Ok(LongValue::Big(value)) if value <= max => Ok(value),
        Ok(LongValue::Big(_)) | Ok(LongValue::Wide { .. }) => Err(CheckedLongError::Overflow(1)),
        Err(LongError::NotInt) => Err(CheckedLongError::NotInt),
        Err(LongError::Raised) => Err(CheckedLongError::Raised),
    }
}

fn masked_unsigned_value(op: *mut PyObject, width: u32) -> Result<u64, CheckedLongError> {
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    match py_long_value(op, true) {
        Ok(LongValue::Signed(value)) => Ok((value as u64) & mask),
        Ok(LongValue::Big(value)) => Ok(value & mask),
        Ok(LongValue::Wide { bits, low_u64, .. }) => {
            let low_u64 = if let Some(low_u64) = low_u64 {
                low_u64
            } else {
                let mut low_u64 = 0u64;
                if unsafe { (hooks_or_stubs().int_as_u64_mask)(bits, 64, &raw mut low_u64) } != 0 {
                    if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                        set_long_overflow_msg(c"runtime integer mask authority unavailable");
                    }
                    return Err(CheckedLongError::Raised);
                }
                low_u64
            };
            Ok(low_u64 & mask)
        }
        Err(LongError::NotInt) => Err(CheckedLongError::NotInt),
        Err(LongError::Raised) => Err(CheckedLongError::Raised),
    }
}

/// The runtime handle bits for the small int `v`, or 0 when unavailable.
fn small_int_bits(v: i64) -> u64 {
    MoltObject::try_from_int(v)
        .map(MoltObject::bits)
        .unwrap_or(0)
}

/// Sign of a heap bignum beyond the ±2^64 hook envelope, via the runtime
/// numeric authority: `v >> 128` is `-1` for negatives and `0` otherwise.
/// `None` when the authority is unavailable (stub hooks) or errored.
fn big_int_sign(bits: u64) -> Option<i64> {
    let h = hooks_or_stubs();
    let shift = small_int_bits(128);
    if shift == 0 {
        return None;
    }
    let result =
        unsafe { (h.number_binary_op)(crate::hooks::NumberBinaryOp::Rshift as u32, bits, shift) };
    if result == 0 {
        return None;
    }
    MoltObject::from_bits(result).as_int()
}

/// True when `op` resolves to a genuine Molt int (inline int, bool, or heap
/// BigInt) — the objects `py_long_as_ssize_clamped` reads directly without an
/// `__index__` round-trip. Zero-alloc; the slice fast path depends on it.
pub(crate) fn is_int_like(op: *mut PyObject) -> bool {
    if op.is_null() {
        return false;
    }
    let Some(bits) = GLOBAL_BRIDGE.molt_handle_for_pyobj(op) else {
        return false;
    };
    let obj = bits.decode();
    obj.is_int()
        || obj.is_bool()
        || (obj.is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(bits.bits()) }
                == crate::abi_types::MoltTypeTag::Int as u8)
}

/// `PyNumber_AsSsize_t(v, NULL)`-style clamped read of an *integer* object
/// (Objects/abstract.c): an out-of-`Py_ssize_t` value CLAMPS to
/// `PY_SSIZE_T_MAX`/`PY_SSIZE_T_MIN` by the value's sign instead of raising —
/// the `err == NULL` contract the slice machinery relies on
/// (`_PyEval_SliceIndex`, Python/ceval.c). Returns `None` with a pending
/// exception when `op` is not an int or the sign of a beyond-±2^64 bignum
/// cannot be resolved (runtime authority absent — loud, never a wrong-direction
/// clamp).
pub(crate) fn py_long_as_ssize_clamped(op: *mut PyObject) -> Option<isize> {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(v)) => {
            // On 32-bit targets (wasm32) clamp rather than truncate.
            Some(v.clamp(isize::MIN as i64, isize::MAX as i64) as isize)
        }
        Ok(LongValue::Big(_)) => Some(isize::MAX),
        Ok(LongValue::Wide { sign, .. }) if sign < 0 => Some(isize::MIN),
        Ok(LongValue::Wide { .. }) => Some(isize::MAX),
        Err(LongError::NotInt) => {
            set_long_type_error();
            None
        }
        Err(LongError::Raised) => None,
    }
}

/// Exact int→f64 conversion for a heap bignum beyond the 64-bit hook envelope,
/// via the runtime's own numeric authority: `v / 1` (TrueDivide) is the exact
/// CPython `nb_float`-equivalent conversion, raising the runtime's OverflowError
/// past f64 range. Returns `None` when the authority is unavailable (stub hooks)
/// or errored (pending exception).
fn big_int_as_f64(bits: u64) -> Option<f64> {
    let h = hooks_or_stubs();
    let one = small_int_bits(1);
    if one == 0 {
        return None;
    }
    let result =
        unsafe { (h.number_binary_op)(crate::hooks::NumberBinaryOp::TrueDivide as u32, bits, one) };
    (result != 0)
        .then(|| MoltObject::from_bits(result).as_float())
        .flatten()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NumericParseError {
    InvalidBase,
    InvalidLiteral,
    Overflow,
    Type,
}

fn ascii_digit_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'z' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'Z' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}

fn strip_base_prefix(bytes: &[u8], base: u32) -> &[u8] {
    if bytes.len() >= 2 && bytes[0] == b'0' {
        match (bytes[1], base) {
            (b'x' | b'X', 16) | (b'o' | b'O', 8) | (b'b' | b'B', 2) => &bytes[2..],
            _ => bytes,
        }
    } else {
        bytes
    }
}

/// A validated Python int literal: sign + base + digit values, before any
/// magnitude folding. The single validation authority behind both the 64-bit
/// fast parse and the arbitrary-precision hook-composed fallback.
struct ScannedIntLiteral {
    negative: bool,
    base: u32,
    digits: Vec<u32>,
}

fn scan_python_int_literal(
    bytes: &[u8],
    base_arg: c_int,
) -> Result<ScannedIntLiteral, NumericParseError> {
    if base_arg != 0 && !(2..=36).contains(&base_arg) {
        return Err(NumericParseError::InvalidBase);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| NumericParseError::InvalidLiteral)?;
    let mut body = text.trim().as_bytes();
    if body.is_empty() {
        return Err(NumericParseError::InvalidLiteral);
    }

    let negative = match body[0] {
        b'+' => {
            body = &body[1..];
            false
        }
        b'-' => {
            body = &body[1..];
            true
        }
        _ => false,
    };
    if body.is_empty() {
        return Err(NumericParseError::InvalidLiteral);
    }

    let mut base = base_arg as u32;
    if base == 0 {
        base = if body.len() >= 2 && body[0] == b'0' {
            match body[1] {
                b'x' | b'X' => 16,
                b'o' | b'O' => 8,
                b'b' | b'B' => 2,
                _ => 10,
            }
        } else {
            10
        };
    }
    body = strip_base_prefix(body, base);

    let mut digits = Vec::with_capacity(body.len());
    let mut previous_digit = false;
    let mut previous_underscore = false;
    for &byte in body {
        if byte == b'_' {
            if !previous_digit || previous_underscore {
                return Err(NumericParseError::InvalidLiteral);
            }
            previous_digit = false;
            previous_underscore = true;
            continue;
        }
        let Some(digit) = ascii_digit_value(byte) else {
            return Err(NumericParseError::InvalidLiteral);
        };
        if digit >= base {
            return Err(NumericParseError::InvalidLiteral);
        }
        digits.push(digit);
        previous_digit = true;
        previous_underscore = false;
    }
    if digits.is_empty() || previous_underscore {
        return Err(NumericParseError::InvalidLiteral);
    }
    Ok(ScannedIntLiteral {
        negative,
        base,
        digits,
    })
}

fn parse_python_int_literal(bytes: &[u8], base_arg: c_int) -> Result<i128, NumericParseError> {
    let scanned = scan_python_int_literal(bytes, base_arg)?;
    let limit: u128 = if scanned.negative {
        (i64::MAX as u128) + 1
    } else {
        u64::MAX as u128
    };
    let mut value = 0u128;
    for &digit in &scanned.digits {
        value = value
            .checked_mul(scanned.base as u128)
            .and_then(|acc| acc.checked_add(digit as u128))
            .ok_or(NumericParseError::Overflow)?;
        if value > limit {
            return Err(NumericParseError::Overflow);
        }
    }
    if scanned.negative {
        if value == (i64::MAX as u128) + 1 {
            Ok(i64::MIN as i128)
        } else {
            Ok(-((value as i64) as i128))
        }
    } else {
        Ok(value as i128)
    }
}

/// Fold a validated literal beyond the 64-bit fast path into an
/// arbitrary-precision runtime int by Horner's rule through the runtime
/// numeric authority (`acc*base + digit` per digit). Returns handle bits or
/// `None` when the authority is unavailable (stub hooks) or errored.
fn build_big_int_from_literal(scanned: &ScannedIntLiteral) -> Option<u64> {
    let h = hooks_or_stubs();
    let base_bits = small_int_bits(scanned.base as i64);
    if base_bits == 0 {
        return None;
    }
    let mut acc = small_int_bits(0);
    for &digit in &scanned.digits {
        let mul = unsafe {
            (h.number_binary_op)(
                crate::hooks::NumberBinaryOp::Multiply as u32,
                acc,
                base_bits,
            )
        };
        if mul == 0 {
            return None;
        }
        let digit_bits = small_int_bits(digit as i64);
        let add = unsafe {
            (h.number_binary_op)(crate::hooks::NumberBinaryOp::Add as u32, mul, digit_bits)
        };
        if add == 0 {
            return None;
        }
        acc = add;
    }
    if scanned.negative {
        let neg = unsafe { (h.number_unary_op)(crate::hooks::NumberUnaryOp::Negative as u32, acc) };
        if neg == 0 {
            return None;
        }
        acc = neg;
    }
    Some(acc)
}

fn normalize_float_literal(bytes: &[u8]) -> Result<String, NumericParseError> {
    let text = std::str::from_utf8(bytes).map_err(|_| NumericParseError::InvalidLiteral)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(NumericParseError::InvalidLiteral);
    }
    let raw = trimmed.as_bytes();
    let mut out = String::with_capacity(raw.len());
    for (index, &byte) in raw.iter().enumerate() {
        if byte == b'_' {
            let prev_is_digit = index > 0 && raw[index - 1].is_ascii_digit();
            let next_is_digit = index + 1 < raw.len() && raw[index + 1].is_ascii_digit();
            if !prev_is_digit || !next_is_digit {
                return Err(NumericParseError::InvalidLiteral);
            }
            continue;
        }
        if !byte.is_ascii() {
            return Err(NumericParseError::InvalidLiteral);
        }
        out.push(byte as char);
    }
    Ok(out)
}

fn parse_python_float_literal(bytes: &[u8]) -> Result<f64, NumericParseError> {
    let normalized = normalize_float_literal(bytes)?;
    let lower = normalized.to_ascii_lowercase();
    match lower.as_str() {
        "nan" | "+nan" | "-nan" => return Ok(f64::NAN),
        "inf" | "+inf" | "infinity" | "+infinity" => return Ok(f64::INFINITY),
        "-inf" | "-infinity" => return Ok(f64::NEG_INFINITY),
        _ => {}
    }
    normalized
        .parse::<f64>()
        .map_err(|_| NumericParseError::InvalidLiteral)
}

unsafe fn py_textlike_bytes(op: *mut PyObject) -> Result<Vec<u8>, NumericParseError> {
    if op.is_null() {
        return Err(NumericParseError::Type);
    }
    if unsafe { crate::api::strings::PyUnicode_Check(op) } != 0 {
        let mut len: Py_ssize_t = 0;
        let ptr = unsafe { crate::api::strings::PyUnicode_AsUTF8AndSize(op, &raw mut len) };
        if ptr.is_null() || len < 0 {
            return Err(NumericParseError::InvalidLiteral);
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len as usize) };
        return Ok(bytes.to_vec());
    }
    if unsafe { crate::api::strings::PyBytes_Check(op) } != 0 {
        let mut ptr_out: *mut c_char = ptr::null_mut();
        let mut len: Py_ssize_t = 0;
        if unsafe {
            crate::api::strings::PyBytes_AsStringAndSize(op, &raw mut ptr_out, &raw mut len)
        } != 0
            || ptr_out.is_null()
            || len < 0
        {
            return Err(NumericParseError::InvalidLiteral);
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr_out.cast::<u8>(), len as usize) };
        return Ok(bytes.to_vec());
    }
    Err(NumericParseError::Type)
}

unsafe fn set_numeric_parse_error(kind: NumericParseError, message: &'static std::ffi::CStr) {
    let exc = match kind {
        NumericParseError::InvalidBase | NumericParseError::InvalidLiteral => {
            &raw mut crate::abi_types::PyExc_ValueError
        }
        NumericParseError::Overflow => &raw mut crate::abi_types::PyExc_OverflowError,
        NumericParseError::Type => &raw mut crate::abi_types::PyExc_TypeError,
    };
    unsafe { crate::api::errors::PyErr_SetString(exc, message.as_ptr()) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLong(v: c_long) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_i64(v as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromSsize_t(v: isize) -> *mut PyObject {
    unsafe { PyLong_FromLong(v as c_long) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromSize_t(v: usize) -> *mut PyObject {
    py_long_from_u64(v as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromLongLong(v: c_longlong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_i64(v as i64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedLong(v: c_ulong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_u64(v as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedLongLong(v: c_ulonglong) -> *mut PyObject {
    #[allow(clippy::unnecessary_cast)]
    py_long_from_u64(v as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromVoidPtr(p: *mut c_void) -> *mut PyObject {
    py_long_from_u64(p as usize as u64)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromDouble(v: c_double) -> *mut PyObject {
    if v.is_nan() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_ValueError,
                c"cannot convert float NaN to integer".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    if !v.is_finite() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_OverflowError,
                c"cannot convert float infinity to integer".as_ptr(),
            );
        }
        return ptr::null_mut();
    }

    let truncated = v.trunc();
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&truncated) {
        return py_long_from_i64(truncated as i64);
    }
    // |value| >= 2^63: CPython builds the exact arbitrary-precision integer
    // (Objects/longobject.c PyLong_FromDouble never overflows for a finite
    // double). Any such double is integral (mantissa 53 bits < magnitude), so
    // compose it EXACTLY as mantissa << (exponent - 52) through the runtime
    // numeric authority. The pre-fix body raised OverflowError here.
    let raw = truncated.abs().to_bits();
    let exponent = ((raw >> 52) & 0x7ff) as i64 - 1023;
    let mantissa = (raw & ((1u64 << 52) - 1)) | (1u64 << 52);
    let shift = exponent - 52; // > 0 whenever |value| >= 2^63
    let h = hooks_or_stubs();
    let mantissa_bits = unsafe { (h.int_from_u64)(mantissa) };
    let shift_bits = small_int_bits(shift);
    let shifted = if mantissa_bits != 0 && shift_bits != 0 {
        unsafe {
            (h.number_binary_op)(
                crate::hooks::NumberBinaryOp::Lshift as u32,
                mantissa_bits,
                shift_bits,
            )
        }
    } else {
        0
    };
    let result = if shifted != 0 && truncated < 0.0 {
        unsafe { (h.number_unary_op)(crate::hooks::NumberUnaryOp::Negative as u32, shifted) }
    } else {
        shifted
    };
    if result == 0 {
        // Runtime numeric authority unavailable (stub hooks) or errored: fail
        // loudly, never silently — but only set our message when the authority
        // did not already raise its own.
        let h2 = hooks_or_stubs();
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
            && unsafe { (h2.exception_pending)() } == 0
        {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    &raw mut crate::abi_types::PyExc_OverflowError,
                    c"float too large for the Molt int authority (runtime hooks absent)".as_ptr(),
                );
            }
        }
        return ptr::null_mut();
    }
    unsafe { GLOBAL_BRIDGE.handle_to_pyobj(result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_Sign(op: *mut PyObject) -> c_int {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(value)) => value.signum() as c_int,
        Ok(LongValue::Big(_)) => 1,
        Ok(LongValue::Wide { sign, .. }) => sign.signum() as c_int,
        Err(LongError::NotInt | LongError::Raised) => 0,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnicodeObject(u: *mut PyObject, base: c_int) -> *mut PyObject {
    let bytes = match unsafe { py_textlike_bytes(u) } {
        Ok(bytes) => bytes,
        Err(kind) => {
            unsafe {
                set_numeric_parse_error(kind, c"int() argument must be a string-like object")
            };
            return ptr::null_mut();
        }
    };
    match parse_python_int_literal(&bytes, base) {
        Ok(value) if value < 0 => py_long_from_i64(value as i64),
        Ok(value) => py_long_from_u64(value as u64),
        Err(NumericParseError::Overflow) => {
            // CPython `_PyLong_FromString` parses arbitrary-precision literals
            // with no magnitude bound (Objects/longobject.c). Beyond the 64-bit
            // fast path, fold the already-validated digits through the runtime
            // numeric authority; only when that authority is absent (stub
            // hooks) does this raise instead of fabricating a capped value.
            let scanned = match scan_python_int_literal(&bytes, base) {
                Ok(scanned) => scanned,
                Err(kind) => {
                    unsafe { set_numeric_parse_error(kind, c"invalid literal for int()") };
                    return ptr::null_mut();
                }
            };
            match build_big_int_from_literal(&scanned) {
                Some(bits) => unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) },
                None => {
                    unsafe {
                        set_numeric_parse_error(
                            NumericParseError::Overflow,
                            c"int literal exceeds the Molt int authority (runtime hooks absent)",
                        )
                    };
                    ptr::null_mut()
                }
            }
        }
        Err(kind) => {
            unsafe { set_numeric_parse_error(kind, c"invalid literal for int()") };
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_FromByteArray(
    bytes: *const u8,
    n: usize,
    little_endian: c_int,
    is_signed: c_int,
) -> *mut PyObject {
    if bytes.is_null() {
        return ptr::null_mut();
    }
    let data = unsafe { std::slice::from_raw_parts(bytes, n.min(8)) };
    let mut value = 0u64;
    if little_endian != 0 {
        for (shift, byte) in data.iter().enumerate() {
            value |= (*byte as u64) << (shift * 8);
        }
    } else {
        for byte in data {
            value = (value << 8) | (*byte as u64);
        }
    }
    if is_signed != 0 && !data.is_empty() {
        let sign_bit = if little_endian != 0 {
            data[data.len() - 1] & 0x80
        } else {
            data[0] & 0x80
        };
        if sign_bit != 0 {
            let bits = (data.len() * 8) as u32;
            let signed = if bits >= 64 {
                value as i64
            } else {
                let mask = 1u64 << (bits - 1);
                ((value ^ mask).wrapping_sub(mask)) as i64
            };
            return py_long_from_i64(signed);
        }
    }
    py_long_from_u64(value)
}

/// CPython `PyLong_AsLong` (Objects/longobject.c): accepts int and any object
/// with `__index__` (via `_PyNumber_Index`); raises OverflowError
/// "Python int too large to convert to C long" when the value does not fit the
/// platform `long` (32-bit on Windows/wasm32); returns -1 only with an
/// exception set. The pre-fix body silently truncated (`as c_long`) and
/// returned bare -1 sentinels.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLong(op: *mut PyObject) -> c_long {
    match checked_signed_value(op, true, c_long::MIN as i64, c_long::MAX as i64) {
        Ok(value) => value as c_long,
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C long");
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    }
}

/// CPython 3.14 `PyLong_IsZero`: return 1 for a zero int, 0 for a non-zero
/// int, and -1 with TypeError for any non-int object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_IsZero(op: *mut PyObject) -> c_int {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(value)) => (value == 0) as c_int,
        Ok(LongValue::Big(value)) => (value == 0) as c_int,
        Ok(LongValue::Wide { .. }) => 0,
        Err(LongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(LongError::Raised) => -1,
    }
}

/// CPython ``PyLong_AsDouble`` (Objects/longobject.c): converts any Python
/// int — including heap bignums — to ``double``, raising OverflowError only
/// past f64 range and TypeError "an integer is required" for a non-int; -1.0
/// is returned only with an exception set. Bignums beyond the 64-bit hook
/// envelope convert exactly through the runtime numeric authority
/// (``v / 1`` TrueDivide), which raises its own OverflowError past 1e308.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsDouble(op: *mut PyObject) -> c_double {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(v)) => v as c_double,
        Ok(LongValue::Big(v)) => v as c_double,
        Err(LongError::NotInt) => {
            set_long_type_error();
            -1.0
        }
        Ok(LongValue::Wide { .. }) => {
            // A genuine int beyond ±2^64: exact conversion via the runtime
            // authority when available; honest OverflowError otherwise.
            let bits = GLOBAL_BRIDGE
                .molt_handle_for_pyobj(op)
                .map(|value| value.bits())
                .unwrap_or(0);
            if bits != 0
                && let Some(v) = big_int_as_f64(bits)
            {
                return v;
            }
            if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                set_long_overflow_msg(c"int too large to convert to float");
            }
            -1.0
        }
        Err(LongError::Raised) => -1.0,
    }
}

/// CPython `PyLong_AsLongAndOverflow`: on out-of-range returns **-1** with
/// `*overflow = ±1` and NO exception (the caller handles the overflow); a
/// non-int dispatches `__index__` and raises TypeError with `*overflow = 0`.
/// The pre-fix body returned a clamped MAX/MIN (divergent) and a silent -1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongAndOverflow(
    op: *mut PyObject,
    overflow: *mut c_int,
) -> c_long {
    let mut ov: c_int = 0;
    let result = match checked_signed_value(op, true, c_long::MIN as i64, c_long::MAX as i64) {
        Ok(value) => value as c_long,
        Err(CheckedLongError::Overflow(sign)) => {
            ov = sign as c_int;
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    };
    if !overflow.is_null() {
        unsafe { *overflow = ov };
    }
    result
}

/// CPython `PyLong_AsSsize_t` (Objects/longobject.c): `PyLong_Check` only — NO
/// `__index__` dispatch; TypeError "an integer is required" for a non-int;
/// OverflowError "Python int too large to convert to C ssize_t" out of range
/// (`isize` is 32-bit on the wasm32 witness — the pre-fix `as isize` silently
/// truncated there, feeding numpy wrong shapes/strides; and its silent -1 was
/// numpy's reshape-"infer" sentinel).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsSsize_t(op: *mut PyObject) -> isize {
    match checked_signed_value(op, false, isize::MIN as i64, isize::MAX as i64) {
        Ok(value) => value as isize,
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C ssize_t");
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    }
}

/// CPython `PyLong_AsLongLong`: `__index__` dispatch, OverflowError
/// "Python int too large to convert to C long long" beyond i64, -1 only with
/// an exception set (was: silent truncation through the unchecked hook).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLong(op: *mut PyObject) -> c_longlong {
    match checked_signed_value(op, true, i64::MIN, i64::MAX) {
        Ok(value) => value as c_longlong,
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C long long");
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    }
}

/// CPython `PyLong_AsLongLongAndOverflow`: same contract as the `long` variant
/// — **-1** with `*overflow = ±1` (no exception) on out-of-range, `__index__`
/// dispatch for non-int. The pre-fix body returned `c_longlong::MAX` on
/// positive overflow (divergent clamp).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsLongLongAndOverflow(
    op: *mut PyObject,
    overflow: *mut c_int,
) -> c_longlong {
    let mut ov: c_int = 0;
    let result = match checked_signed_value(op, true, i64::MIN, i64::MAX) {
        Ok(value) => value as c_longlong,
        Err(CheckedLongError::Overflow(sign)) => {
            ov = sign as c_int;
            -1
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            -1
        }
        Err(CheckedLongError::Raised | CheckedLongError::Negative) => -1,
    };
    if !overflow.is_null() {
        unsafe { *overflow = ov };
    }
    result
}

/// CPython `PyLong_AsUnsignedLong` (Objects/longobject.c): `PyLong_Check` only;
/// TypeError "an integer is required" for non-int; OverflowError
/// "can't convert negative value to unsigned int" for negatives (CPython's
/// historical message says "int" even for `unsigned long`); OverflowError
/// "Python int too large to convert to C unsigned long" past the width. The
/// pre-fix body wrapped negatives/non-ints to huge values with no exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLong(op: *mut PyObject) -> c_ulong {
    const SENTINEL: c_ulong = c_ulong::MAX; // (unsigned long)-1
    match checked_unsigned_value(op, c_ulong::MAX as u64) {
        Ok(value) => value as c_ulong,
        Err(CheckedLongError::Negative) => {
            set_long_overflow_msg(c"can't convert negative value to unsigned int");
            SENTINEL
        }
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C unsigned long");
            SENTINEL
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            SENTINEL
        }
        Err(CheckedLongError::Raised) => SENTINEL,
    }
}

/// CPython `PyLong_AsUnsignedLongLong`: same strict contract at 64-bit width.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLongLong(op: *mut PyObject) -> c_ulonglong {
    const SENTINEL: c_ulonglong = c_ulonglong::MAX; // (unsigned long long)-1
    match checked_unsigned_value(op, u64::MAX) {
        Ok(value) => value as c_ulonglong,
        Err(CheckedLongError::Negative) => {
            set_long_overflow_msg(c"can't convert negative value to unsigned int");
            SENTINEL
        }
        Err(CheckedLongError::Overflow(_)) => {
            set_long_overflow_msg(c"Python int too large to convert to C unsigned long long");
            SENTINEL
        }
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            SENTINEL
        }
        Err(CheckedLongError::Raised) => SENTINEL,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLongMask(op: *mut PyObject) -> c_ulong {
    match masked_unsigned_value(op, c_ulong::BITS) {
        Ok(value) => value as c_ulong,
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            c_ulong::MAX
        }
        Err(CheckedLongError::Raised) => c_ulong::MAX,
        Err(CheckedLongError::Negative | CheckedLongError::Overflow(_)) => unreachable!(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsUnsignedLongLongMask(op: *mut PyObject) -> c_ulonglong {
    match masked_unsigned_value(op, c_ulonglong::BITS) {
        Ok(value) => value as c_ulonglong,
        Err(CheckedLongError::NotInt) => {
            set_long_type_error();
            c_ulonglong::MAX
        }
        Err(CheckedLongError::Raised) => c_ulonglong::MAX,
        Err(CheckedLongError::Negative | CheckedLongError::Overflow(_)) => unreachable!(),
    }
}

/// CPython `PyLong_AsVoidPtr`: TypeError for non-int, OverflowError when the
/// value does not fit a pointer; NULL is returned only with an exception set
/// (the pre-fix body returned a bare NULL on every failure).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsVoidPtr(op: *mut PyObject) -> *mut c_void {
    match py_long_value(op, false) {
        Ok(LongValue::Signed(v)) if v < 0 => match isize::try_from(v) {
            Ok(v) => v as *mut c_void,
            Err(_) => {
                set_long_overflow_msg(c"Python int too large to convert to C void*");
                ptr::null_mut()
            }
        },
        Ok(LongValue::Signed(v)) => match usize::try_from(v) {
            Ok(v) => v as *mut c_void,
            Err(_) => {
                set_long_overflow_msg(c"Python int too large to convert to C void*");
                ptr::null_mut()
            }
        },
        Ok(LongValue::Big(v)) => match usize::try_from(v) {
            Ok(v) => v as *mut c_void,
            Err(_) => {
                set_long_overflow_msg(c"Python int too large to convert to C void*");
                ptr::null_mut()
            }
        },
        Err(LongError::NotInt) => {
            set_long_type_error();
            ptr::null_mut()
        }
        Ok(LongValue::Wide { .. }) => {
            set_long_overflow_msg(c"Python int too large to convert to C void*");
            ptr::null_mut()
        }
        Err(LongError::Raised) => ptr::null_mut(),
    }
}

/// Minimal two's-complement byte width for `v` (CPython's size-query contract
/// for `PyLong_AsNativeBytes`): the smallest n with
/// `-(1<<(8n-1)) <= v < 1<<(8n-1)`.
fn min_signed_byte_width(v: i64) -> usize {
    let significant = if v >= 0 {
        65 - v.leading_zeros() as usize
    } else {
        65 - (!v).leading_zeros() as usize
    };
    significant.div_ceil(8)
}

/// CPython `PyLong_AsNativeBytes`: returns the number of bytes actually needed
/// (the size-query contract — the pre-fix body always reported 8, so a size
/// probe for the value 5 was told 8), copies min(n_bytes, needed) bytes, and
/// sets an exception on every -1 return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsNativeBytes(
    op: *mut PyObject,
    buffer: *mut c_void,
    n_bytes: Py_ssize_t,
    _flags: c_int,
) -> Py_ssize_t {
    let (bytes, required): ([u8; 8], usize) = match py_long_value(op, false) {
        Ok(LongValue::Signed(v)) => (v.to_le_bytes(), min_signed_byte_width(v)),
        Ok(LongValue::Big(v)) => {
            // Needs the sign byte above the 8 value bytes: 9 total.
            (v.to_le_bytes(), 9)
        }
        Err(LongError::NotInt) => {
            set_long_type_error();
            return -1;
        }
        Ok(LongValue::Wide { .. }) => {
            set_long_overflow_msg(c"Molt int exceeds the 64-bit native-bytes envelope");
            return -1;
        }
        Err(LongError::Raised) => return -1,
    };
    if !buffer.is_null() && n_bytes > 0 {
        let count = (n_bytes as usize).min(bytes.len());
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast::<u8>(), count);
        }
        // A 9-byte requirement with a 9+ byte buffer gets its 0x00 sign byte.
        if required == 9 && (n_bytes as usize) >= 9 {
            unsafe { *buffer.cast::<u8>().add(8) = 0 };
        }
    }
    required as Py_ssize_t
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromNativeBytes(
    buffer: *const c_void,
    n_bytes: usize,
    _flags: c_int,
) -> *mut PyObject {
    if buffer.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n_bytes.min(8)) };
    let sign_extend = bytes.last().is_some_and(|byte| (byte & 0x80) != 0);
    let fill = if sign_extend { 0xff } else { 0x00 };
    let mut raw = [fill; 8];
    raw[..bytes.len()].copy_from_slice(bytes);
    py_long_from_i64(i64::from_le_bytes(raw))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_FromUnsignedNativeBytes(
    buffer: *const c_void,
    n_bytes: usize,
    _flags: c_int,
) -> *mut PyObject {
    if buffer.is_null() {
        return ptr::null_mut();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n_bytes.min(8)) };
    let mut raw = [0u8; 8];
    raw[..bytes.len()].copy_from_slice(bytes);
    py_long_from_u64(u64::from_le_bytes(raw))
}

/// CPython `_PyLong_AsInt` / `PyLong_AsInt` (Objects/longobject.c): via
/// `PyLong_AsLongAndOverflow`, so `__index__` dispatches and a non-int raises
/// TypeError; out-of-`int` raises OverflowError
/// "Python int too large to convert to C int". The pre-fix body returned a
/// bare -1 for a non-int (in-range, so indistinguishable from int(-1)).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_AsInt(op: *mut PyObject) -> c_int {
    let mut overflow: c_int = 0;
    let value = unsafe { PyLong_AsLongAndOverflow(op, &raw mut overflow) };
    if value == -1 && overflow == 0 && !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return -1;
    }
    // Widen to i64 so the range test is platform-independent (c_long is 32-bit
    // on Windows/wasm32, 64-bit on LP64) and never an absurd comparison.
    let wide = i64::from(value);
    if overflow != 0 || wide > c_int::MAX as i64 || wide < c_int::MIN as i64 {
        set_long_overflow_msg(c"Python int too large to convert to C int");
        return -1;
    }
    value as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_AsInt(op: *mut PyObject) -> c_int {
    unsafe { _PyLong_AsInt(op) }
}

fn set_long_overflow() {
    unsafe {
        crate::api::errors::PyErr_SetString(
            &raw mut crate::abi_types::PyExc_OverflowError,
            c"int too big to convert".as_ptr(),
        );
    }
}

/// CPython `_PyLong_AsByteArray` (Objects/longobject.c): serializes the int
/// into exactly `n` bytes, raising OverflowError "int too big to convert" when
/// it does not fit and "can't convert negative int to unsigned" for a negative
/// value with `is_signed == 0`. The pre-fix body silently truncated any value
/// beyond 64 bits and returned bare -1 for a non-int; both now raise honestly
/// (values beyond the ±2^64 hook envelope raise OverflowError rather than
/// round-tripping corrupted).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyLong_AsByteArray(
    v: *mut crate::abi_types::PyLongObject,
    bytes: *mut u8,
    n: usize,
    little_endian: c_int,
    is_signed: c_int,
) -> c_int {
    if v.is_null() || bytes.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    // Widen to i128 so the (i64::MAX, u64::MAX] band serializes correctly.
    let value: i128 = match py_long_value(v.cast::<PyObject>(), false) {
        Ok(LongValue::Signed(v)) => v as i128,
        Ok(LongValue::Big(v)) => v as i128,
        Err(LongError::NotInt) => {
            set_long_type_error();
            return -1;
        }
        Ok(LongValue::Wide { .. }) => {
            set_long_overflow();
            return -1;
        }
        Err(LongError::Raised) => return -1,
    };
    if n == 0 {
        if value == 0 {
            return 0;
        }
        set_long_overflow();
        return -1;
    }
    if value < 0 && is_signed == 0 {
        set_long_overflow_msg(c"can't convert negative int to unsigned");
        return -1;
    }
    if n < 16 {
        let bits = (n * 8) as u32;
        if is_signed != 0 {
            let min = -(1i128.checked_shl(bits - 1).unwrap_or(i128::MAX));
            let max = 1i128
                .checked_shl(bits - 1)
                .map(|v| v - 1)
                .unwrap_or(i128::MAX);
            if value < min || value > max {
                set_long_overflow();
                return -1;
            }
        } else {
            let max = 1u128.checked_shl(bits).map(|v| v - 1).unwrap_or(u128::MAX);
            if (value as u128) > max {
                set_long_overflow();
                return -1;
            }
        }
    }

    let raw = value as u128;
    let fill = if value < 0 { 0xff } else { 0x00 };
    for index in 0..n {
        let source_index = if little_endian != 0 {
            index
        } else {
            n - 1 - index
        };
        let byte = if source_index < 16 {
            ((raw >> (source_index * 8)) & 0xff) as u8
        } else {
            fill
        };
        unsafe {
            *bytes.add(index) = byte;
        }
    }
    0
}

// ─── PyFloat ─────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_FromDouble(v: c_double) -> *mut PyObject {
    let bits = MoltObject::from_float(v).bits();
    unsafe { GLOBAL_BRIDGE.handle_to_pyobj(bits) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_FromString(v: *mut PyObject) -> *mut PyObject {
    let bytes = match unsafe { py_textlike_bytes(v) } {
        Ok(bytes) => bytes,
        Err(kind) => {
            unsafe {
                set_numeric_parse_error(kind, c"float() argument must be a string-like object")
            };
            return ptr::null_mut();
        }
    };
    match parse_python_float_literal(&bytes) {
        Ok(value) => unsafe { PyFloat_FromDouble(value) },
        Err(kind) => {
            unsafe { set_numeric_parse_error(kind, c"could not convert string to float") };
            ptr::null_mut()
        }
    }
}

/// The `tp_name` of `op`'s type, for CPython-shaped conversion errors. Safe
/// for any non-null object with a readable header.
unsafe fn float_arg_type_name(op: *mut PyObject) -> String {
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        return "<unknown>".to_string();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        "<unknown>".to_string()
    } else {
        unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    }
}

/// CPython `PyFloat_AsDouble` (Objects/floatobject.c): exact float read, else
/// `nb_float` dispatch, else the `nb_index` route, else TypeError
/// "must be real number, not '<type>'" with **-1.0** (never a silent NaN — the
/// pre-fix NaN return poisoned numpy compute paths with fake values).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_AsDouble(op: *mut PyObject) -> c_double {
    if op.is_null() {
        unsafe { crate::api::errors::PyErr_BadArgument() };
        return -1.0;
    }
    let op_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
    if let Some(bits) = op_handle {
        let obj = bits.decode();
        if obj.is_float() {
            return obj.as_float().unwrap_or(-1.0);
        }
        if let Some(i) = obj.as_int() {
            return i as f64;
        }
        if obj.is_bool() {
            return obj.as_bool().unwrap_or(false) as i64 as f64;
        }
        if obj.is_ptr() {
            let h = hooks_or_stubs();
            if unsafe { (h.classify_heap)(bits.bits()) } == crate::abi_types::MoltTypeTag::Int as u8
            {
                // Heap bignum: checked 64-bit reads, then the exact runtime
                // conversion authority for the beyond-64-bit band.
                let mut sv = 0i64;
                if unsafe { (h.int_as_i64_checked)(bits.bits(), &raw mut sv) } == 0 {
                    return sv as f64;
                }
                let mut uv = 0u64;
                if unsafe { (h.int_as_u64_checked)(bits.bits(), &raw mut uv) } == 0 {
                    return uv as f64;
                }
                if let Some(v) = big_int_as_f64(bits.bits()) {
                    return v;
                }
                if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
                    set_long_overflow_msg(c"int too large to convert to float");
                }
                return -1.0;
            }
        }
    }
    // Foreign or non-numeric: dispatch `nb_float` (PyNumber_Float's foreign
    // path), then the `nb_index` route, mirroring floatobject.c.
    let converted = unsafe { crate::api::abstract_number::PyNumber_Float(op) };
    if !converted.is_null() {
        let converted_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(converted);
        let value = if let Some(bits) = converted_handle {
            bits.decode().as_float()
        } else {
            None
        };
        unsafe { crate::api::refcount::Py_DECREF(converted) };
        if let Some(v) = value {
            return v;
        }
    } else if unsafe { crate::api::abstract_number::PyIndex_Check(op) } != 0 {
        unsafe { crate::api::errors::PyErr_Clear() };
        let index = unsafe { crate::api::abstract_number::PyNumber_Index(op) };
        if index.is_null() {
            return -1.0;
        }
        let value = unsafe { PyLong_AsDouble(index) };
        unsafe { crate::api::refcount::Py_DECREF(index) };
        return value;
    }
    // Shape the failure like PyFloat_AsDouble (the pending PyNumber_Float
    // TypeError has CPython's float()-flavored text; PyFloat_AsDouble's is
    // "must be real number"). Preserve any non-TypeError exception verbatim.
    if unsafe {
        crate::api::errors::PyErr_ExceptionMatches(&raw mut crate::abi_types::PyExc_TypeError)
    } == 1
        || unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
    {
        let message = format!("must be real number, not '{}'", unsafe {
            float_arg_type_name(op)
        });
        let cmsg = std::ffi::CString::new(message).unwrap_or_default();
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                cmsg.as_ptr(),
            );
        }
    }
    -1.0
}

const PY_HASH_INF: isize = 314159;

fn pointer_hash(ptr: *mut PyObject) -> isize {
    let width = usize::BITS;
    let raw = ptr as usize;
    let rotated = (raw >> 4) | (raw << (width - 4));
    let hash = rotated as isize;
    if hash == -1 { -2 } else { hash }
}

fn frexp_abs(value: f64) -> (f64, i32) {
    if value == 0.0 {
        return (0.0, 0);
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = bits & ((1u64 << 52) - 1);
    if exponent == 0 {
        let (m, e) = frexp_abs(value * ((1u64 << 54) as f64));
        return (m, e - 54);
    }
    let m = f64::from_bits((1022u64 << 52) | mantissa);
    (m, exponent - 1022)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_HashDouble(inst: *mut PyObject, v: c_double) -> isize {
    if v.is_infinite() {
        return if v.is_sign_positive() {
            PY_HASH_INF
        } else {
            -PY_HASH_INF
        };
    }
    if v.is_nan() {
        return pointer_hash(inst);
    }

    let sign = if v.is_sign_negative() { -1 } else { 1 };
    let (mut mantissa, mut exponent) = frexp_abs(v.abs());
    let hash_bits = if std::mem::size_of::<isize>() >= 8 {
        61u32
    } else {
        31u32
    };
    let modulus = (1u64 << hash_bits) - 1;
    let mut hash = 0u64;

    while mantissa != 0.0 {
        hash = ((hash << 28) & modulus) | (hash >> (hash_bits - 28));
        mantissa *= 268_435_456.0;
        exponent -= 28;
        let chunk = mantissa as u64;
        mantissa -= chunk as f64;
        hash += chunk;
        if hash >= modulus {
            hash -= modulus;
        }
    }

    let rotate = if exponent >= 0 {
        (exponent as u32) % hash_bits
    } else {
        hash_bits - 1 - ((-1 - exponent) as u32 % hash_bits)
    };
    if rotate != 0 {
        hash = ((hash << rotate) & modulus) | (hash >> (hash_bits - rotate));
    }

    let signed = if sign < 0 {
        -(hash as isize)
    } else {
        hash as isize
    };
    if signed == -1 { -2 } else { signed }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_FromDoubles(real: c_double, imag: c_double) -> *mut PyObject {
    let obj = Box::new(PyComplexObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut crate::abi_types::PyComplex_Type,
        },
        cval: Py_complex { real, imag },
    });
    Box::into_raw(obj).cast::<PyObject>()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_FromCComplex(value: Py_complex) -> *mut PyObject {
    unsafe { PyComplex_FromDoubles(value.real, value.imag) }
}

pub unsafe extern "C" fn molt_complex_dealloc(op: *mut PyObject) {
    if op.is_null() {
        return;
    }
    unsafe { drop(Box::from_raw(op.cast::<PyComplexObject>())) };
}

/// CPython `PyComplex_AsCComplex` (Objects/complexobject.c): a real complex
/// reads directly; everything else falls back to `PyFloat_AsDouble` (which
/// dispatches `nb_float`/`nb_index` on foreign objects) for the real part with
/// imag 0, propagating its TypeError on total failure. The `__complex__`
/// special-method probe CPython runs first is a documented residual (needs a
/// `_PyObject_LookupSpecial` equivalent); every numeric type Molt links
/// exposes `nb_float`/`nb_index`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_AsCComplex(op: *mut PyObject) -> Py_complex {
    if op.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                &raw mut crate::abi_types::PyExc_TypeError,
                c"complex argument is NULL".as_ptr(),
            );
        }
        return Py_complex {
            real: -1.0,
            imag: 0.0,
        };
    }
    if unsafe { PyComplex_Check(op) } != 0 && GLOBAL_BRIDGE.molt_handle_for_pyobj(op).is_none() {
        // `op` is a genuine (non-bridge-minted) C complex object here. On wasm32
        // a statically declared C `PyObject` is only 4-byte aligned, but
        // `PyComplexObject` (two `c_double`s) has alignment 8, so dereferencing
        // it directly would be a misaligned read (UB; caught by the debug
        // alignment check). Take a raw field ref (no dereference) and read the
        // `cval` unaligned.
        let cval = unsafe { &raw const (*op.cast::<PyComplexObject>()).cval };
        return unsafe { std::ptr::read_unaligned(cval) };
    }
    let real = unsafe { PyFloat_AsDouble(op) };
    if real == -1.0 && !unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
        return Py_complex {
            real: -1.0,
            imag: 0.0,
        };
    }
    Py_complex { real, imag: 0.0 }
}

/// CPython `PyComplex_RealAsDouble`: the real part of a complex, else
/// `PyFloat_AsDouble` (Objects/complexobject.c).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_RealAsDouble(op: *mut PyObject) -> c_double {
    if !op.is_null()
        && unsafe { PyComplex_Check(op) } != 0
        && GLOBAL_BRIDGE.molt_handle_for_pyobj(op).is_none()
    {
        // Unaligned read: a C-minted PyComplexObject may sit on a 4-byte
        // boundary on wasm32 (a98ef2978e's misaligned-deref UB class).
        let field = unsafe { &raw const (*op.cast::<PyComplexObject>()).cval.real };
        return unsafe { std::ptr::read_unaligned(field) };
    }
    unsafe { PyFloat_AsDouble(op) }
}

/// CPython `PyComplex_ImagAsDouble`: the imag part of a complex, else **0.0
/// with NO error and no conversion attempt** (Objects/complexobject.c). The
/// pre-fix delegation to `PyComplex_AsCComplex` left a live TypeError while
/// returning 0.0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_ImagAsDouble(op: *mut PyObject) -> c_double {
    if !op.is_null()
        && unsafe { PyComplex_Check(op) } != 0
        && GLOBAL_BRIDGE.molt_handle_for_pyobj(op).is_none()
    {
        // Unaligned read: same C-minted-object alignment class as above.
        let field = unsafe { &raw const (*op.cast::<PyComplexObject>()).cval.imag };
        return unsafe { std::ptr::read_unaligned(field) };
    }
    0.0
}

/// CPython `PyComplex_Check` (Include/complexobject.h): `PyObject_TypeCheck`
/// semantics — exact type OR any subclass via the subtype walk, not the
/// pre-fix exact-pointer identity.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyComplex_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ob_type = unsafe { (*op).ob_type };
    if std::ptr::eq(ob_type, &raw const crate::abi_types::PyComplex_Type) {
        return 1;
    }
    unsafe {
        crate::api::typeobj::PyType_IsSubtype(ob_type, &raw mut crate::abi_types::PyComplex_Type)
    }
}

// ─── PyBool ──────────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyBool_FromLong(v: c_long) -> *mut PyObject {
    if v != 0 {
        (&raw mut Py_True).cast::<PyObject>()
    } else {
        (&raw mut Py_False).cast::<PyObject>()
    }
}

// ─── Type checks (PyLong_Check etc.) ─────────────────────────────────────────

macro_rules! type_check {
    ($name:ident, $pred:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(op: *mut PyObject) -> c_int {
            if op.is_null() {
                return 0;
            }
            let op_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
            match op_handle {
                Some(value) => value.decode().$pred() as c_int,
                None => 0,
            }
        }
    };
}

/// CPython `PyLong_Check` (Include/longobject.h): `Py_TPFLAGS_LONG_SUBCLASS`
/// semantics — true for int, **bool** (an int subtype: `PyLong_Check(True)`
/// is 1), heap bignums, and foreign int subclasses via the subtype walk. The
/// pre-fix `is_int()` answered 0 for bool AND for heap bignums.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyLong_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let op_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
    if let Some(value) = op_handle {
        let bits = value.bits();
        let obj = value.decode();
        if obj.is_int() || obj.is_bool() {
            return 1;
        }
        if obj.is_ptr() {
            return (unsafe { (hooks_or_stubs().classify_heap)(bits) }
                == crate::abi_types::MoltTypeTag::Int as u8) as c_int;
        }
        return 0;
    }
    // Foreign C object: walk the subtype chain against the int type.
    unsafe {
        crate::api::typeobj::PyType_IsSubtype((*op).ob_type, &raw mut crate::abi_types::PyLong_Type)
    }
}

/// CPython `PyFloat_Check` (Include/floatobject.h): float plus foreign float
/// subclasses (numpy `float64` subclasses `float`) via the subtype walk.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyFloat_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let op_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
    if let Some(value) = op_handle {
        return value.decode().is_float() as c_int;
    }
    unsafe {
        crate::api::typeobj::PyType_IsSubtype(
            (*op).ob_type,
            &raw mut crate::abi_types::PyFloat_Type,
        )
    }
}

type_check!(PyBool_Check, is_bool);

/// CPython `PyNumber_Check` (Objects/abstract.c): true when the type provides
/// `nb_index`/`nb_int`/`nb_float` or is complex — including foreign C objects,
/// whose slots the pre-fix bridged-only test never consulted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyNumber_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let op_handle = GLOBAL_BRIDGE.molt_handle_for_pyobj(op);
    if let Some(value) = op_handle {
        let bits = value.bits();
        let obj = value.decode();
        if obj.is_int() || obj.is_float() || obj.is_bool() {
            return 1;
        }
        if obj.is_ptr()
            && unsafe { (hooks_or_stubs().classify_heap)(bits) }
                == crate::abi_types::MoltTypeTag::Int as u8
        {
            return 1;
        }
        return 0;
    }
    if unsafe { PyComplex_Check(op) } != 0 {
        return 1;
    }
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        return 0;
    }
    let num = unsafe { (*tp).tp_as_number }.cast::<crate::abi_types::PyNumberMethods>();
    if num.is_null() {
        return 0;
    }
    (!unsafe { (*num).nb_index }.is_null()
        || !unsafe { (*num).nb_int }.is_null()
        || !unsafe { (*num).nb_float }.is_null()) as c_int
}

#[cfg(test)]
mod tests {
    use super::{NumericParseError, parse_python_float_literal, parse_python_int_literal};

    #[test]
    fn parses_python_int_literals_with_base_prefixes_and_underscores() {
        assert_eq!(parse_python_int_literal(b"  +1_024  ", 10), Ok(1024));
        assert_eq!(parse_python_int_literal(b"0xff", 0), Ok(255));
        assert_eq!(parse_python_int_literal(b"-0b101", 0), Ok(-5));
        assert_eq!(
            parse_python_int_literal(b"-9223372036854775808", 10),
            Ok(i64::MIN as i128)
        );
    }

    #[test]
    fn rejects_invalid_or_overflowing_python_int_literals() {
        assert_eq!(
            parse_python_int_literal(b"1__0", 10),
            Err(NumericParseError::InvalidLiteral)
        );
        assert_eq!(
            parse_python_int_literal(b"10", 1),
            Err(NumericParseError::InvalidBase)
        );
        assert_eq!(
            parse_python_int_literal(b"18446744073709551616", 10),
            Err(NumericParseError::Overflow)
        );
    }

    #[test]
    fn parses_python_float_literals_with_special_values_and_underscores() {
        assert_eq!(parse_python_float_literal(b"  1_024.5  "), Ok(1024.5));
        assert!(parse_python_float_literal(b"nan").unwrap().is_nan());
        assert_eq!(
            parse_python_float_literal(b"-Infinity"),
            Ok(f64::NEG_INFINITY)
        );
    }

    #[test]
    fn rejects_invalid_python_float_literals() {
        assert_eq!(
            parse_python_float_literal(b"1__0.0"),
            Err(NumericParseError::InvalidLiteral)
        );
        assert_eq!(
            parse_python_float_literal("π".as_bytes()),
            Err(NumericParseError::InvalidLiteral)
        );
    }
}
