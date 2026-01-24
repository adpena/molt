use std::cmp::Ordering;
use std::mem;

use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive};

use crate::{
    alloc_object, attr_lookup_ptr_allow_missing, call_callable0, class_name_for_error,
    dec_ref_bits, exception_pending, intern_static_name, maybe_ptr_from_bits, obj_from_bits,
    object_type_id, raise_exception, runtime_state, type_of_bits, MoltHeader, INLINE_INT_MAX_I128,
    INLINE_INT_MIN_I128, TYPE_ID_BIGINT,
};

pub(crate) fn to_i64(obj: MoltObject) -> Option<i64> {
    if obj.is_int() {
        return obj.as_int();
    }
    if obj.is_bool() {
        return Some(if obj.as_bool().unwrap_or(false) { 1 } else { 0 });
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

pub(crate) fn to_bigint(obj: MoltObject) -> Option<BigInt> {
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    let ptr = bigint_ptr_from_bits(obj.bits())?;
    Some(unsafe { bigint_ref(ptr).clone() })
}

pub(crate) fn bigint_to_inline(value: &BigInt) -> Option<i64> {
    let val = value.to_i64()?;
    if (val as i128) >= INLINE_INT_MIN_I128 && (val as i128) <= INLINE_INT_MAX_I128 {
        Some(val)
    } else {
        None
    }
}

pub(crate) fn int_bits_from_bigint(value: BigInt) -> u64 {
    if let Some(i) = bigint_to_inline(&value) {
        return MoltObject::from_int(i).bits();
    }
    bigint_bits(value)
}

pub(crate) fn inline_int_from_i128(val: i128) -> Option<i64> {
    if (INLINE_INT_MIN_I128..=INLINE_INT_MAX_I128).contains(&val) {
        Some(val as i64)
    } else {
        None
    }
}

pub(crate) fn int_bits_from_i64(val: i64) -> u64 {
    if let Some(inline) = inline_int_from_i128(val as i128) {
        return MoltObject::from_int(inline).bits();
    }
    bigint_bits(BigInt::from(val))
}

pub(crate) fn bigint_bits(value: BigInt) -> u64 {
    let total = mem::size_of::<MoltHeader>() + mem::size_of::<BigInt>();
    let ptr = alloc_object(total, TYPE_ID_BIGINT);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        std::ptr::write(ptr as *mut BigInt, value);
    }
    MoltObject::from_ptr(ptr).bits()
}

pub(crate) fn int_bits_from_i128(val: i128) -> u64 {
    if let Some(i) = inline_int_from_i128(val) {
        MoltObject::from_int(i).bits()
    } else {
        bigint_bits(BigInt::from(val))
    }
}

pub(crate) unsafe fn bigint_ref(ptr: *mut u8) -> &'static BigInt {
    &*(ptr as *const BigInt)
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
    if sign < 0 {
        -big
    } else {
        big
    }
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
    if floor_int & 1 == 0 {
        floor
    } else {
        ceil
    }
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

pub(crate) fn index_i64_from_obj(obj_bits: u64, err: &str) -> i64 {
    let obj = obj_from_bits(obj_bits);
    if let Some(i) = to_i64(obj) {
        return i;
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        unsafe {
            let index_name_bits =
                intern_static_name(&runtime_state().interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(ptr, index_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    return i;
                }
                let res_type = class_name_for_error(type_of_bits(res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                raise_exception::<i64>("TypeError", &msg);
            }
        }
    }
    raise_exception::<i64>("TypeError", err)
}

pub(crate) fn float_pair_from_obj(lhs: MoltObject, rhs: MoltObject) -> Option<(f64, f64)> {
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return Some((lf, rf));
    }
    if (lhs.is_float() || rhs.is_float())
        && (bigint_ptr_from_bits(lhs.bits()).is_some()
            || bigint_ptr_from_bits(rhs.bits()).is_some())
    {
        return raise_exception::<Option<(f64, f64)>>(
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
    if let Some(ptr) = bigint_ptr_from_bits(lhs.bits()) {
        if let Some(f) = to_f64(rhs) {
            return compare_bigint_float(unsafe { bigint_ref(ptr) }, f);
        }
    }
    if let Some(ptr) = bigint_ptr_from_bits(rhs.bits()) {
        if let Some(f) = to_f64(lhs) {
            return compare_bigint_float(unsafe { bigint_ref(ptr) }, f).map(Ordering::reverse);
        }
    }
    if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
        return lf.partial_cmp(&rf);
    }
    None
}

pub(crate) fn split_maxsplit_from_obj(obj_bits: u64) -> i64 {
    let obj = obj_from_bits(obj_bits);
    let msg = format!(
        "'{}' object cannot be interpreted as an integer",
        crate::type_name(obj)
    );
    let Some(value) = index_bigint_from_obj(obj_bits, &msg) else {
        return 0;
    };
    if value.is_negative() {
        return -1;
    }
    value.to_i64().unwrap_or(i64::MAX)
}

pub(crate) fn index_i64_with_overflow(
    obj_bits: u64,
    err: &str,
    overflow_err: Option<&str>,
) -> Option<i64> {
    let value = index_bigint_from_obj(obj_bits, err)?;
    if let Some(i) = value.to_i64() {
        return Some(i);
    }
    let msg = match overflow_err {
        Some(msg) => msg.to_string(),
        None => format!(
            "cannot fit '{}' into an index-sized integer",
            class_name_for_error(type_of_bits(obj_bits))
        ),
    };
    raise_exception::<Option<i64>>("IndexError", &msg)
}

pub(crate) fn index_bigint_from_obj(obj_bits: u64, err: &str) -> Option<BigInt> {
    let obj = obj_from_bits(obj_bits);
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj_bits) {
        return Some(unsafe { bigint_ref(ptr).clone() });
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj_bits) {
        unsafe {
            let index_name_bits =
                intern_static_name(&runtime_state().interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(ptr, index_name_bits) {
                let res_bits = call_callable0(call_bits);
                dec_ref_bits(call_bits);
                if exception_pending() {
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    return Some(BigInt::from(i));
                }
                if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = bigint_ref(big_ptr).clone();
                    dec_ref_bits(res_bits);
                    return Some(big);
                }
                let res_type = class_name_for_error(type_of_bits(res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                raise_exception::<u64>("TypeError", &msg);
                return None;
            }
            if exception_pending() {
                return None;
            }
        }
    }
    raise_exception::<u64>("TypeError", err);
    None
}

pub(crate) fn to_f64(obj: MoltObject) -> Option<f64> {
    if let Some(val) = obj.as_float() {
        return Some(val);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        return unsafe { bigint_ref(ptr) }.to_f64();
    }
    None
}
