use crate::PyToken;
use std::cmp::Ordering;
use std::mem;
use std::sync::atomic::Ordering as AtomicOrdering;

use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_traits::{Signed, ToPrimitive, Zero};

use crate::object::ops::{as_float_extended, is_float_extended};
use crate::{
    INLINE_INT_MAX_I128, INLINE_INT_MIN_I128, MoltHeader, TYPE_ID_BIGINT, TYPE_ID_COMPLEX,
    TYPE_ID_OBJECT, alloc_object, call_callable0, class_mro_vec, class_name_for_error,
    dec_ref_bits, exception_pending, maybe_ptr_from_bits, obj_from_bits, object_class_bits,
    object_type_id, raise_exception, runtime_state_for_gil, type_of_bits,
};

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ComplexParts {
    pub(crate) re: f64,
    pub(crate) im: f64,
}

fn builtin_int_bits_for_gil() -> Option<u64> {
    let state = runtime_state_for_gil()?;
    let ptr = state.builtin_classes.load(AtomicOrdering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { (*ptr).int })
    }
}

fn builtin_float_bits_for_gil() -> Option<u64> {
    let state = runtime_state_for_gil()?;
    let ptr = state.builtin_classes.load(AtomicOrdering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(unsafe { (*ptr).float })
    }
}

fn is_int_subclass_bits(class_bits: u64) -> bool {
    let Some(int_bits) = builtin_int_bits_for_gil() else {
        return false;
    };
    if class_bits == int_bits {
        return true;
    }
    class_mro_vec(class_bits).contains(&int_bits)
}

fn is_float_subclass_bits(class_bits: u64) -> bool {
    let Some(float_bits) = builtin_float_bits_for_gil() else {
        return false;
    };
    if class_bits == float_bits {
        return true;
    }
    class_mro_vec(class_bits).contains(&float_bits)
}

pub(crate) fn int_subclass_value_bits_raw(obj_bits: u64) -> Option<u64> {
    let obj = obj_from_bits(obj_bits);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_OBJECT {
            return None;
        }
        let class_bits = object_class_bits(ptr);
        if class_bits == 0 || !is_int_subclass_bits(class_bits) {
            return None;
        }
        Some(*(ptr as *const u64))
    }
}

pub(crate) fn float_subclass_value_bits_raw(obj_bits: u64) -> Option<u64> {
    let obj = obj_from_bits(obj_bits);
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_OBJECT {
            return None;
        }
        let class_bits = object_class_bits(ptr);
        if class_bits == 0 || !is_float_subclass_bits(class_bits) {
            return None;
        }
        Some(*(ptr as *const u64))
    }
}

#[inline(always)]
pub(crate) fn to_i64(obj: MoltObject) -> Option<i64> {
    if obj.is_int() {
        return Some(obj.as_int_unchecked());
    }
    if obj.is_bool() {
        return Some(if (obj.bits() & 0x1) == 1 { 1 } else { 0 });
    }
    // Float that represents an exact integer (e.g., 1.0 from codegen).
    // Handles both inline floats and heap-allocated NaN floats
    // (NaN never passes the fract/abs checks, so this is a no-op for NaN).
    if let Some(f) = as_float_extended(obj)
        && f.fract() == 0.0
        && f.abs() < (1i64 << 53) as f64
    {
        return Some(f as i64);
    }
    if let Some(bits) = int_subclass_value_bits_raw(obj.bits()) {
        let val_obj = obj_from_bits(bits);
        if let Some(i) = val_obj.as_int() {
            return Some(i);
        }
        if val_obj.is_bool() {
            return Some(if val_obj.as_bool().unwrap_or(false) {
                1
            } else {
                0
            });
        }
        if let Some(ptr) = bigint_ptr_from_bits(bits) {
            return unsafe { bigint_ref(ptr) }.to_i64();
        }
    }
    // A bare heap BigInt (a Python `int` whose magnitude exceeds the inline-int
    // tag range, ~2^46) is still a plain integer. Convert it when it fits in
    // i64 so intrinsics that accept large integers (e.g. os.utime nanosecond
    // timestamps) see the value instead of falling through to `None`.
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return unsafe { bigint_ref(ptr) }.to_i64();
    }
    None
}

pub(crate) fn bigint_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = maybe_ptr_from_bits(bits)?;
    unsafe {
        if object_type_id(ptr) == TYPE_ID_BIGINT {
            Some(ptr)
        } else {
            None
        }
    }
}

pub(crate) fn complex_ptr_from_bits(bits: u64) -> Option<*mut u8> {
    let ptr = maybe_ptr_from_bits(bits)?;
    unsafe {
        if object_type_id(ptr) == TYPE_ID_COMPLEX {
            Some(ptr)
        } else {
            None
        }
    }
}

pub(crate) unsafe fn complex_ref(ptr: *mut u8) -> &'static ComplexParts {
    unsafe { &*(ptr as *const ComplexParts) }
}

pub(crate) fn complex_bits(_py: &PyToken<'_>, re: f64, im: f64) -> u64 {
    let total = mem::size_of::<MoltHeader>() + mem::size_of::<ComplexParts>();
    let ptr = alloc_object(_py, total, TYPE_ID_COMPLEX);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        std::ptr::write(ptr as *mut ComplexParts, ComplexParts { re, im });
    }
    MoltObject::from_ptr(ptr).bits()
}

pub(crate) fn complex_from_obj_strict(
    _py: &PyToken<'_>,
    obj: MoltObject,
) -> Result<Option<ComplexParts>, ()> {
    if let Some(ptr) = complex_ptr_from_bits(obj.bits()) {
        return Ok(Some(unsafe { *complex_ref(ptr) }));
    }
    if let Some(f) = obj.as_float() {
        return Ok(Some(ComplexParts { re: f, im: 0.0 }));
    }
    if let Some(i) = to_i64(obj) {
        return Ok(Some(ComplexParts {
            re: i as f64,
            im: 0.0,
        }));
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        if let Some(val) = unsafe { bigint_ref(ptr) }.to_f64() {
            return Ok(Some(ComplexParts { re: val, im: 0.0 }));
        }
        return Err(());
    }
    Ok(None)
}

pub(crate) fn complex_from_obj_lossy(obj: MoltObject) -> Option<ComplexParts> {
    if let Some(ptr) = complex_ptr_from_bits(obj.bits()) {
        return Some(unsafe { *complex_ref(ptr) });
    }
    if let Some(f) = obj.as_float() {
        return Some(ComplexParts { re: f, im: 0.0 });
    }
    if let Some(i) = to_i64(obj) {
        return Some(ComplexParts {
            re: i as f64,
            im: 0.0,
        });
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return unsafe { bigint_ref(ptr) }
            .to_f64()
            .map(|val| ComplexParts { re: val, im: 0.0 });
    }
    None
}

pub(crate) fn to_bigint(obj: MoltObject) -> Option<BigInt> {
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return Some(unsafe { bigint_ref(ptr).clone() });
    }
    if let Some(bits) = int_subclass_value_bits_raw(obj.bits()) {
        let val_obj = obj_from_bits(bits);
        if let Some(i) = val_obj.as_int() {
            return Some(BigInt::from(i));
        }
        if val_obj.is_bool() {
            return Some(BigInt::from(if val_obj.as_bool().unwrap_or(false) {
                1
            } else {
                0
            }));
        }
        if let Some(ptr) = bigint_ptr_from_bits(bits) {
            return Some(unsafe { bigint_ref(ptr).clone() });
        }
    }
    None
}

pub(crate) const INT_BYTES_OK: i32 = 0;
pub(crate) const INT_BYTES_OVERFLOW: i32 = 1;
pub(crate) const INT_BYTES_NEGATIVE_UNSIGNED: i32 = 2;
pub(crate) const INT_BYTES_INVALID: i32 = -1;

/// Shared arbitrary-width two's-complement decoder for Python `int.from_bytes`
/// and the CPython ABI `_PyLong_FromByteArray` hook.
pub(crate) fn bigint_from_bytes(data: &[u8], little_endian: bool, signed: bool) -> BigInt {
    match (little_endian, signed) {
        (true, true) => BigInt::from_signed_bytes_le(data),
        (false, true) => BigInt::from_signed_bytes_be(data),
        (true, false) => BigInt::from_bytes_le(Sign::Plus, data),
        (false, false) => BigInt::from_bytes_be(Sign::Plus, data),
    }
}

/// Shared arbitrary-width fixed-width encoder.
///
/// On magnitude overflow the low `out.len()` bytes are still written before
/// `INT_BYTES_OVERFLOW` is returned, matching `_PyLong_AsByteArray`.
pub(crate) fn bigint_to_bytes(
    value: &BigInt,
    out: &mut [u8],
    little_endian: bool,
    signed: bool,
) -> i32 {
    if !signed && value.sign() == Sign::Minus {
        return INT_BYTES_NEGATIVE_UNSIGNED;
    }

    // num-bigint already owns the minimal two's-complement/sign-magnitude
    // encoder. Use its single output buffer directly: the prior modulus,
    // remainder, normalization and padding path allocated several full-width
    // BigInts and a second Vec for every conversion.
    let bytes = if signed {
        if little_endian {
            value.to_signed_bytes_le()
        } else {
            value.to_signed_bytes_be()
        }
    } else if little_endian {
        value.to_bytes_le().1
    } else {
        value.to_bytes_be().1
    };
    let fits = value.is_zero() && out.is_empty() || bytes.len() <= out.len();
    let pad = if signed && value.sign() == Sign::Minus {
        0xff
    } else {
        0
    };
    out.fill(pad);
    let copied = bytes.len().min(out.len());
    if copied != 0 {
        if little_endian {
            out[..copied].copy_from_slice(&bytes[..copied]);
        } else {
            let out_start = out.len() - copied;
            let bytes_start = bytes.len() - copied;
            out[out_start..].copy_from_slice(&bytes[bytes_start..]);
        }
    };
    if fits {
        INT_BYTES_OK
    } else {
        INT_BYTES_OVERFLOW
    }
}

pub(crate) fn bigint_num_bits(value: &BigInt) -> Option<usize> {
    usize::try_from(value.bits()).ok()
}

pub(crate) fn bigint_to_inline(value: &BigInt) -> Option<i64> {
    let val = value.to_i64()?;
    if (val as i128) >= INLINE_INT_MIN_I128 && (val as i128) <= INLINE_INT_MAX_I128 {
        Some(val)
    } else {
        None
    }
}

pub(crate) fn int_bits_from_bigint(_py: &PyToken<'_>, value: BigInt) -> u64 {
    if let Some(i) = bigint_to_inline(&value) {
        return MoltObject::from_int(i).bits();
    }
    bigint_bits(_py, value)
}

pub(crate) fn inline_int_from_i128(val: i128) -> Option<i64> {
    if (INLINE_INT_MIN_I128..=INLINE_INT_MAX_I128).contains(&val) {
        Some(val as i64)
    } else {
        None
    }
}

pub(crate) fn int_bits_from_i64(_py: &PyToken<'_>, val: i64) -> u64 {
    if let Some(inline) = inline_int_from_i128(val as i128) {
        return MoltObject::from_int(inline).bits();
    }
    bigint_bits(_py, BigInt::from(val))
}

pub(crate) fn bigint_bits(_py: &PyToken<'_>, value: BigInt) -> u64 {
    // BigInt contains a Vec<u64> which stores digits on the heap.
    // The BigInt struct itself is fixed-size (Sign enum + Vec metadata).
    // We allocate space for MoltHeader + the BigInt struct.
    // The Vec's heap buffer is separate and managed by the Vec allocator.
    let bigint_size = mem::size_of::<BigInt>();
    let total = mem::size_of::<MoltHeader>() + bigint_size;
    let ptr = alloc_object(_py, total, TYPE_ID_BIGINT);
    if ptr.is_null() {
        crate::record_memory_error_without_allocation(_py);
        return MoltObject::none().bits();
    }
    unsafe {
        std::ptr::write(ptr as *mut BigInt, value);
    }
    MoltObject::from_ptr(ptr).bits()
}

#[inline]
pub(crate) fn int_bits_from_i128(_py: &PyToken<'_>, val: i128) -> u64 {
    if let Some(i) = inline_int_from_i128(val) {
        MoltObject::from_int(i).bits()
    } else {
        bigint_bits(_py, BigInt::from(val))
    }
}

pub(crate) unsafe fn bigint_ref(ptr: *mut u8) -> &'static BigInt {
    unsafe { &*(ptr as *const BigInt) }
}

pub(crate) fn compare_bigint_float(big: &BigInt, f: f64) -> Option<Ordering> {
    if f.is_nan() {
        return None;
    }
    if f.is_infinite() {
        if f.is_sign_positive() {
            return Some(Ordering::Less);
        }
        return Some(Ordering::Greater);
    }
    if let Some(big_f) = big.to_f64() {
        return big_f.partial_cmp(&f);
    }
    if big.is_negative() {
        Some(Ordering::Less)
    } else {
        Some(Ordering::Greater)
    }
}

pub(crate) fn bigint_from_f64_trunc(val: f64) -> BigInt {
    if val == 0.0 {
        return BigInt::from(0);
    }
    let bits = val.to_bits();
    let sign = if (bits >> 63) != 0 { -1 } else { 1 };
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac_bits = bits & ((1u64 << 52) - 1);
    let (mantissa, exp) = if exp_bits == 0 {
        (frac_bits, 1 - 1023 - 52)
    } else {
        ((1u64 << 52) | frac_bits, exp_bits - 1023 - 52)
    };
    let mut big = BigInt::from(mantissa);
    if exp >= 0 {
        big <<= exp as usize;
    } else {
        big >>= (-exp) as usize;
    }
    if sign < 0 { -big } else { big }
}

pub(crate) fn round_half_even(val: f64) -> f64 {
    if !val.is_finite() {
        return val;
    }
    let floor = val.floor();
    let ceil = val.ceil();
    let diff_floor = (val - floor).abs();
    let diff_ceil = (ceil - val).abs();
    if diff_floor < diff_ceil {
        return floor;
    }
    if diff_ceil < diff_floor {
        return ceil;
    }
    if floor.abs() > i64::MAX as f64 {
        return floor;
    }
    let floor_int = floor as i64;
    if floor_int & 1 == 0 { floor } else { ceil }
}

pub(crate) fn round_float_ndigits(val: f64, ndigits: i64) -> f64 {
    if !val.is_finite() {
        return val;
    }
    if ndigits == 0 {
        return round_half_even(val);
    }
    if ndigits > 0 {
        if ndigits > 308 {
            return val;
        }
        let formatted = format!("{:.*}", ndigits as usize, val);
        return formatted.parse::<f64>().unwrap_or(val);
    }
    let factor = 10f64.powi((-ndigits) as i32);
    if !factor.is_finite() {
        return if val.is_sign_negative() { -0.0 } else { 0.0 };
    }
    if factor == 0.0 {
        return val;
    }
    let scaled = val / factor;
    round_half_even(scaled) * factor
}

pub(crate) fn index_i64_integral_bits(bits: u64) -> Option<i64> {
    let obj = obj_from_bits(bits);
    if obj.is_int() {
        return Some(obj.as_int_unchecked());
    }
    if obj.is_bool() {
        return Some(if (obj.bits() & 0x1) == 1 { 1 } else { 0 });
    }
    if let Some(ptr) = bigint_ptr_from_bits(bits) {
        return unsafe { bigint_ref(ptr) }.to_i64();
    }
    if let Some(value_bits) = int_subclass_value_bits_raw(bits) {
        let value = obj_from_bits(value_bits);
        if let Some(i) = value.as_int() {
            return Some(i);
        }
        if value.is_bool() {
            return Some(if value.as_bool().unwrap_or(false) {
                1
            } else {
                0
            });
        }
        if let Some(ptr) = bigint_ptr_from_bits(value_bits) {
            return unsafe { bigint_ref(ptr) }.to_i64();
        }
    }
    None
}

pub(crate) fn index_bigint_integral_bits(bits: u64) -> Option<BigInt> {
    let obj = obj_from_bits(bits);
    if obj.is_int() {
        return Some(BigInt::from(obj.as_int_unchecked()));
    }
    if obj.is_bool() {
        return Some(BigInt::from(if (obj.bits() & 0x1) == 1 { 1 } else { 0 }));
    }
    if let Some(ptr) = bigint_ptr_from_bits(bits) {
        return Some(unsafe { bigint_ref(ptr).clone() });
    }
    if let Some(value_bits) = int_subclass_value_bits_raw(bits) {
        let value = obj_from_bits(value_bits);
        if let Some(i) = value.as_int() {
            return Some(BigInt::from(i));
        }
        if value.is_bool() {
            return Some(BigInt::from(if value.as_bool().unwrap_or(false) {
                1
            } else {
                0
            }));
        }
        if let Some(ptr) = bigint_ptr_from_bits(value_bits) {
            return Some(unsafe { bigint_ref(ptr).clone() });
        }
    }
    None
}

pub(crate) fn index_i64_from_obj(_py: &PyToken<'_>, obj_bits: u64, err: &str) -> i64 {
    if let Some(value) = index_i64_integral_bits(obj_bits) {
        return value;
    }
    let Some(value) = index_bigint_from_obj(_py, obj_bits, err) else {
        return 0;
    };
    value.to_i64().unwrap_or_else(|| {
        raise_exception::<i64>(
            _py,
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        )
    })
}

#[inline]
pub(crate) fn float_pair_from_obj(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> Option<(f64, f64)> {
    // Only coerce to float when at least one operand is actually a float.
    // Without this guard, bigint + int silently loses precision by converting
    // the bigint to f64 (e.g. 10**20 + 1 → 1e20 instead of 100000000000000000001).
    // Checks both inline floats (non-NaN) and heap-allocated NaN floats.
    if !is_float_extended(lhs) && !is_float_extended(rhs) {
        return None;
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return Some((lf, rf));
    }
    if bigint_ptr_from_bits(lhs.bits()).is_some() || bigint_ptr_from_bits(rhs.bits()).is_some() {
        return raise_exception::<Option<(f64, f64)>>(
            _py,
            "OverflowError",
            "int too large to convert to float",
        );
    }
    None
}

pub(crate) fn compare_numbers(lhs: MoltObject, rhs: MoltObject) -> Option<Ordering> {
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        return Some(li.cmp(&ri));
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        return Some(l_big.cmp(&r_big));
    }
    if let Some(ptr) = bigint_ptr_from_bits(lhs.bits())
        && let Some(f) = to_f64(rhs)
    {
        return compare_bigint_float(unsafe { bigint_ref(ptr) }, f);
    }
    if let Some(ptr) = bigint_ptr_from_bits(rhs.bits())
        && let Some(f) = to_f64(lhs)
    {
        return compare_bigint_float(unsafe { bigint_ref(ptr) }, f).map(Ordering::reverse);
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return lf.partial_cmp(&rf);
    }
    None
}

pub(crate) fn split_maxsplit_from_obj(_py: &PyToken<'_>, obj_bits: u64) -> i64 {
    let obj = obj_from_bits(obj_bits);
    let msg = format!(
        "'{}' object cannot be interpreted as an integer",
        crate::type_name(_py, obj)
    );
    let Some(value) = index_bigint_from_obj(_py, obj_bits, &msg) else {
        return 0;
    };
    // The boxed transport is i64 on every backend, but Python's index-sized
    // admission follows the target pointer width (also used by sys.maxsize).
    match value.to_isize() {
        Some(value) => value as i64,
        None => raise_exception::<i64>(
            _py,
            "OverflowError",
            "Python int too large to convert to C ssize_t",
        ),
    }
}

pub(crate) fn index_i64_with_overflow(
    _py: &PyToken<'_>,
    obj_bits: u64,
    err: &str,
    overflow_err: Option<&str>,
) -> Option<i64> {
    let value = index_bigint_from_obj(_py, obj_bits, err)?;
    if let Some(i) = value.to_i64() {
        return Some(i);
    }
    let msg = match overflow_err {
        Some(msg) => msg.to_string(),
        None => format!(
            "cannot fit '{}' into an index-sized integer",
            class_name_for_error(type_of_bits(_py, obj_bits))
        ),
    };
    raise_exception::<Option<i64>>(_py, "IndexError", &msg)
}

fn sequence_index_type_error(_py: &PyToken<'_>, key_bits: u64, container_tp: &str) -> String {
    format!(
        "{} indices must be integers or slices, not {}",
        container_tp,
        crate::type_name(_py, obj_from_bits(key_bits)),
    )
}

pub(crate) fn sequence_index_i64_with_type_error(
    _py: &PyToken<'_>,
    key_bits: u64,
    type_err: &str,
) -> Option<i64> {
    if let Some(i) = index_i64_integral_bits(key_bits) {
        return Some(i);
    }
    index_i64_with_overflow(_py, key_bits, type_err, None)
}

pub(crate) fn sequence_index_bigint(
    _py: &PyToken<'_>,
    key_bits: u64,
    container_tp: &str,
) -> Option<BigInt> {
    let type_err = sequence_index_type_error(_py, key_bits, container_tp);
    index_bigint_from_obj(_py, key_bits, &type_err)
}

/// Coerce a subscript key to an `i64` sequence index under CPython `__index__`
/// semantics. This is the single authority for tuple / list / bytearray index
/// extraction (and shares its integral fast path with `range`).
///
/// Accepts `int`, `bool`, an `int` subclass, a bare `bigint`, and any object
/// implementing `__index__`. REJECTS `float` (even an integral `2.0`), `str`,
/// `None`, and every other non-index type with `TypeError: <container> indices
/// must be integers or slices, not <keytype>` — byte-identical to CPython
/// 3.12+ (`_PyIndex_Check` gates sequence subscript, and `float` has no
/// `nb_index`). An index magnitude exceeding `isize` raises
/// `IndexError: cannot fit '<type>' into an index-sized integer`.
///
/// Do NOT reintroduce a `to_i64` fast path here: `to_i64` is a *numeric*
/// coercion that silently accepts integral floats, whereas sequence indexing is
/// the integer protocol (`__index__`), which floats deliberately do not satisfy.
///
/// Returns `None` with a pending exception on any error; the caller must
/// propagate (`return MoltObject::none().bits()`).
pub(crate) fn sequence_index_i64(
    _py: &PyToken<'_>,
    key_bits: u64,
    container_tp: &str,
) -> Option<i64> {
    // Fast path: int / bool / int-subclass — no BigInt allocation, no float.
    if let Some(i) = index_i64_integral_bits(key_bits) {
        return Some(i);
    }
    // Slow path: bare bigint, `__index__` objects, overflow -> IndexError, and
    // the TypeError raise for float / other non-index keys. The message is the
    // CPython "<container> indices must be integers or slices, not <keytype>"
    // sequence family, shared by tuple / list / range / byte / bytearray.
    let type_err = sequence_index_type_error(_py, key_bits, container_tp);
    index_i64_with_overflow(_py, key_bits, &type_err, None)
}

pub(crate) fn index_bigint_from_obj(_py: &PyToken<'_>, obj_bits: u64, err: &str) -> Option<BigInt> {
    if let Some(value) = index_bigint_integral_bits(obj_bits) {
        return Some(value);
    }
    if maybe_ptr_from_bits(obj_bits).is_some() {
        unsafe {
            if let Some(call_bits) =
                crate::builtins::attr::lookup_special_method(_py, obj_bits, b"__index__")
            {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(value) = index_bigint_integral_bits(res_bits) {
                    let exact = builtin_int_bits_for_gil() == Some(type_of_bits(_py, res_bits));
                    let accepted =
                        exact || warn_numeric_subclass_result(_py, "__index__", "int", res_bits);
                    dec_ref_bits(_py, res_bits);
                    if !accepted {
                        return None;
                    }
                    return Some(value);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                raise_exception::<u64>(_py, "TypeError", &msg);
                return None;
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    raise_exception::<u64>(_py, "TypeError", err);
    None
}

fn warn_numeric_subclass_result(
    py: &PyToken<'_>,
    protocol: &str,
    expected: &str,
    result_bits: u64,
) -> bool {
    let actual = class_name_for_error(type_of_bits(py, result_bits));
    let message = format!(
        "{protocol} returned non-{expected} (type {actual}).  The ability to return an instance of a strict subclass of {expected} is deprecated, and may be removed in a future version of Python."
    );
    crate::builtins::warnings_ext::emit_deprecation_warning(py, &message)
}

fn integer_as_double(py: &PyToken<'_>, value: &BigInt) -> Option<f64> {
    // num_bigint may return Some(infinity), not just None, on overflow.
    if let Some(value) = value.to_f64().filter(|value| value.is_finite()) {
        return Some(value);
    }
    raise_exception::<()>(py, "OverflowError", "int too large to convert to float");
    None
}

/// Shared numeric protocol used by the float constructor after its exact-float
/// identity fast path. No text parsing and no instance-attribute lookup occurs.
/// A missing numeric protocol returns None without an exception, permitting
/// the constructor's text parser; every protocol failure returns None with its
/// original pending exception. Int subclasses must reach __float__ before any
/// integral payload extraction. Float subclasses likewise reach their slot here.
pub(crate) fn float_from_number_protocol(py: &PyToken<'_>, bits: u64) -> Option<f64> {
    let obj = obj_from_bits(bits);
    if let Some(value) = as_float_extended(obj) {
        return Some(value);
    }
    if let Some(value) = obj.as_int() {
        return Some(value as f64);
    }
    if let Some(value) = obj.as_bool() {
        return Some(if value { 1.0 } else { 0.0 });
    }
    if let Some(ptr) = bigint_ptr_from_bits(bits) {
        return integer_as_double(py, unsafe { bigint_ref(ptr) });
    }
    if let Some(call_bits) =
        unsafe { crate::builtins::attr::lookup_special_method(py, bits, b"__float__") }
    {
        let result = unsafe { call_callable0(py, call_bits) };
        dec_ref_bits(py, call_bits);
        if exception_pending(py) {
            dec_ref_bits(py, result);
            return None;
        }
        let result_obj = obj_from_bits(result);
        if let Some(value) = as_float_extended(result_obj) {
            dec_ref_bits(py, result);
            return Some(value);
        }
        let owner = class_name_for_error(type_of_bits(py, bits));
        if let Some(payload) = float_subclass_value_bits_raw(result)
            && let Some(value) = as_float_extended(obj_from_bits(payload))
        {
            let accepted =
                warn_numeric_subclass_result(py, &format!("{owner}.__float__"), "float", result);
            // The returned object remains owned through warning callbacks.
            dec_ref_bits(py, result);
            return accepted.then_some(value);
        }
        let actual = class_name_for_error(type_of_bits(py, result));
        dec_ref_bits(py, result);
        raise_exception::<()>(
            py,
            "TypeError",
            &format!("{owner}.__float__ returned non-float (type {actual})"),
        );
        return None;
    }
    if exception_pending(py) {
        return None;
    }
    // The integral protocol already owns strict result admission, subclass
    // warnings, and callback exception preservation. Its payload path also
    // implements inherited int.__float__ without consulting int.__index__.
    if let Some(value) = index_bigint_integral_bits(bits) {
        return integer_as_double(py, &value);
    }
    let has_index = unsafe { crate::builtins::attr::has_special_method(py, bits, b"__index__") };
    if exception_pending(py) || !has_index {
        return None;
    }
    let message = format!(
        "must be real number, not {}",
        class_name_for_error(type_of_bits(py, bits))
    );
    let value = index_bigint_from_obj(py, bits, &message)?;
    integer_as_double(py, &value)
}

/// PyFloat_AsDouble-style numeric conversion. An existing float or float
/// subclass is read directly, ignoring an overridden __float__. Other values
/// use type-level __float__, then __index__; text is never parsed. None always
/// means a pending exception, including callback/warning errors and integer
/// overflow. Float infinity and NaN remain valid values.
pub(crate) fn float_as_double(py: &PyToken<'_>, bits: u64) -> Option<f64> {
    if let Some(value) = as_float_extended(obj_from_bits(bits)) {
        return Some(value);
    }
    if let Some(payload) = float_subclass_value_bits_raw(bits)
        && let Some(value) = as_float_extended(obj_from_bits(payload))
    {
        return Some(value);
    }
    if let Some(value) = float_from_number_protocol(py, bits) {
        return Some(value);
    }
    if !exception_pending(py) {
        raise_exception::<()>(
            py,
            "TypeError",
            &format!(
                "must be real number, not {}",
                class_name_for_error(type_of_bits(py, bits))
            ),
        );
    }
    None
}

#[inline]
pub(crate) fn to_f64(obj: MoltObject) -> Option<f64> {
    // Handle both inline floats (non-NaN) and heap-allocated NaN floats.
    if let Some(val) = as_float_extended(obj) {
        return Some(val);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return unsafe { bigint_ref(ptr) }.to_f64();
    }
    if let Some(bits) = float_subclass_value_bits_raw(obj.bits()) {
        return as_float_extended(obj_from_bits(bits));
    }
    None
}

#[cfg(test)]
mod float_conversion_tests {
    use super::*;
    use crate::builtins::functions::{alloc_runtime_function_obj, runtime_fn_addr};
    use crate::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FLOAT_CALLS: AtomicU64 = AtomicU64::new(0);
    static INDEX_CALLS: AtomicU64 = AtomicU64::new(0);
    static RESULT: AtomicU64 = AtomicU64::new(0);
    static RAISE: AtomicU64 = AtomicU64::new(0);

    extern "C" fn float_conversion_callback(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            FLOAT_CALLS.fetch_add(1, Ordering::SeqCst);
            if RAISE.load(Ordering::SeqCst) != 0 {
                return raise_exception::<_>(py, "RuntimeError", "float protocol failure");
            }
            let result = RESULT.load(Ordering::SeqCst);
            inc_ref_bits(py, result);
            result
        })
    }

    extern "C" fn index_conversion_callback(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(py, {
            INDEX_CALLS.fetch_add(1, Ordering::SeqCst);
            if RAISE.load(Ordering::SeqCst) != 0 {
                return raise_exception::<_>(py, "LookupError", "index protocol failure");
            }
            let result = RESULT.load(Ordering::SeqCst);
            inc_ref_bits(py, result);
            result
        })
    }

    fn conversion_class(py: &PyToken<'_>, base: u64, float: bool, index: bool) -> u64 {
        let name = attr_name_bits_from_bytes(py, b"FloatConversionContract").unwrap();
        let class = crate::molt_class_new(name);
        dec_ref_bits(py, name);
        crate::molt_class_set_base(class, base);
        let methods: [(&[u8], bool, extern "C" fn(u64) -> u64, &str); 2] = [
            (
                b"__float__",
                float,
                float_conversion_callback,
                "float_conversion_callback",
            ),
            (
                b"__index__",
                index,
                index_conversion_callback,
                "index_conversion_callback",
            ),
        ];
        for (name, enabled, method, symbol) in methods {
            if !enabled {
                continue;
            }
            let name = attr_name_bits_from_bytes(py, name).unwrap();
            let function =
                alloc_runtime_function_obj(py, runtime_fn_addr(symbol, method as *const ()), 1);
            assert!(!function.is_null());
            let function = MoltObject::from_ptr(function).bits();
            crate::molt_set_attr_name(class, name, function);
            dec_ref_bits(py, name);
            dec_ref_bits(py, function);
        }
        unsafe {
            crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
                .unwrap();
        }
        assert!(!exception_pending(py));
        class
    }

    fn plain_instance(py: &PyToken<'_>, class: u64) -> u64 {
        let ptr = obj_from_bits(class).as_ptr().unwrap();
        let size = unsafe { crate::object::layout::class_cached_layout_size(ptr).unwrap() };
        let instance = crate::object::builders::alloc_class_instance(py, size, class);
        unsafe {
            crate::object::gc::gc_publish_initialized(
                py,
                obj_from_bits(instance).as_ptr().unwrap(),
            );
        }
        instance
    }

    fn reset_callbacks(result: u64, raise: bool) {
        RESULT.store(result, Ordering::SeqCst);
        RAISE.store(u64::from(raise), Ordering::SeqCst);
        FLOAT_CALLS.store(0, Ordering::SeqCst);
        INDEX_CALLS.store(0, Ordering::SeqCst);
    }

    fn assert_error(py: &PyToken<'_>, kind: &str, message: &str) {
        assert!(exception_pending(py));
        let error = crate::builtins::exceptions::molt_exception_last_pending();
        assert!(crate::builtins::exceptions::exception_matches_builtin_name(
            py, error, kind
        ));
        let text = crate::builtins::exceptions::exception_materialized_message_bits(
            py,
            obj_from_bits(error).as_ptr().unwrap(),
        );
        assert_eq!(string_obj_to_owned(obj_from_bits(text)).unwrap(), message);
        clear_exception(py);
        dec_ref_bits(py, error);
    }

    #[test]
    fn float_conversion_contract_constructor_and_as_double_subclass_policies() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).float, true, false);
            let value = crate::molt_float_new(class, MoltObject::from_float(1.25).bits());
            assert!(!exception_pending(py));
            reset_callbacks(MoltObject::from_float(2.5).bits(), false);
            assert_eq!(float_as_double(py, value), Some(1.25));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 0);
            let constructed = crate::molt_float_from_obj(value);
            assert_eq!(as_float_extended(obj_from_bits(constructed)), Some(2.5));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            let from_number = crate::molt_float_from_number(builtin_classes(py).float, value);
            assert_eq!(as_float_extended(obj_from_bits(from_number)), Some(1.25));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            for bits in [constructed, from_number, value, class] {
                dec_ref_bits(py, bits);
            }

            let class = conversion_class(py, builtin_classes(py).int, true, true);
            let value =
                crate::molt_int_new(class, MoltObject::from_int(1).bits(), missing_bits(py));
            assert!(!exception_pending(py));
            reset_callbacks(MoltObject::from_float(3.5).bits(), false);
            assert_eq!(float_as_double(py, value), Some(3.5));
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
            dec_ref_bits(py, value);
            dec_ref_bits(py, class);
        });
    }

    #[test]
    fn float_conversion_contract_protocol_order_errors_and_integer_overflow() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = conversion_class(py, builtin_classes(py).object, true, true);
            let value = plain_instance(py, class);
            reset_callbacks(MoltObject::from_int(4).bits(), true);
            assert_eq!(float_as_double(py, value), None);
            assert_error(py, "RuntimeError", "float protocol failure");
            assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);
            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
            reset_callbacks(MoltObject::from_int(4).bits(), false);
            assert_eq!(float_as_double(py, value), None);
            assert_error(
                py,
                "TypeError",
                "FloatConversionContract.__float__ returned non-float (type int)",
            );
            assert_eq!(INDEX_CALLS.load(Ordering::SeqCst), 0);
            dec_ref_bits(py, value);
            dec_ref_bits(py, class);

            let class = conversion_class(py, builtin_classes(py).object, false, true);
            let value = plain_instance(py, class);
            reset_callbacks(MoltObject::from_float(1.5).bits(), false);
            assert_eq!(float_as_double(py, value), None);
            assert_error(py, "TypeError", "__index__ returned non-int (type float)");
            reset_callbacks(MoltObject::from_int(4).bits(), true);
            assert_eq!(float_as_double(py, value), None);
            assert_error(py, "LookupError", "index protocol failure");
            let huge = bigint_bits(py, BigInt::from(1u8) << 4096usize);
            reset_callbacks(huge, false);
            for input in [huge, value] {
                assert_eq!(float_as_double(py, input), None);
                assert_error(py, "OverflowError", "int too large to convert to float");
            }
            for bits in [huge, value, class] {
                dec_ref_bits(py, bits);
            }
            for number in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
                let value = crate::object::ops::float_result_bits(py, number);
                let converted = float_as_double(py, value).unwrap();
                assert!(converted == number || (converted.is_nan() && number.is_nan()));
                dec_ref_bits(py, value);
            }
        });
    }

    #[test]
    fn float_conversion_contract_strict_subclass_warnings_can_raise() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            struct ResetWarnings;
            impl Drop for ResetWarnings {
                fn drop(&mut self) {
                    crate::molt_warnings_resetwarnings();
                }
            }
            let _reset = ResetWarnings;
            crate::molt_warnings_resetwarnings();
            let category = crate::builtins::exceptions::exception_type_bits_from_name(
                py,
                "DeprecationWarning",
            );
            let float_class = conversion_class(py, builtin_classes(py).float, false, false);
            let returned = crate::molt_float_new(float_class, MoltObject::from_float(4.25).bits());
            let class = conversion_class(py, builtin_classes(py).object, true, false);
            let value = plain_instance(py, class);
            let ignore = attr_name_bits_from_bytes(py, b"ignore").unwrap();
            crate::molt_warnings_simplefilter(
                ignore,
                category,
                MoltObject::from_int(0).bits(),
                MoltObject::from_bool(false).bits(),
            );
            reset_callbacks(returned, false);
            assert_eq!(float_as_double(py, value), Some(4.25));
            assert!(!exception_pending(py));
            let error = attr_name_bits_from_bytes(py, b"error").unwrap();
            crate::molt_warnings_simplefilter(
                error,
                category,
                MoltObject::from_int(0).bits(),
                MoltObject::from_bool(false).bits(),
            );
            assert_eq!(float_as_double(py, value), None);
            assert_error(
                py,
                "DeprecationWarning",
                "FloatConversionContract.__float__ returned non-float (type FloatConversionContract).  The ability to return an instance of a strict subclass of float is deprecated, and may be removed in a future version of Python.",
            );
            for bits in [returned, float_class, value, class, ignore, error] {
                dec_ref_bits(py, bits);
            }

            let class = conversion_class(py, builtin_classes(py).object, false, true);
            let value = plain_instance(py, class);
            reset_callbacks(MoltObject::from_bool(true).bits(), false);
            assert_eq!(float_as_double(py, value), None);
            assert_error(
                py,
                "DeprecationWarning",
                "__index__ returned non-int (type bool).  The ability to return an instance of a strict subclass of int is deprecated, and may be removed in a future version of Python.",
            );
            dec_ref_bits(py, value);
            dec_ref_bits(py, class);
        });
    }
}
