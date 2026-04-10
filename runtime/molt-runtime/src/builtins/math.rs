use crate::PyToken;
use crate::builtins::callable::molt_is_callable;
use crate::builtins::numbers::{index_bigint_from_obj, index_i64_from_obj, int_bits_from_bigint};
use crate::object::ops::{as_float_extended, float_result_bits, format_obj, type_name};
use crate::{
    MoltObject, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES, TYPE_ID_LIST, TYPE_ID_STRING, TYPE_ID_TUPLE,
    alloc_list, alloc_tuple, attr_lookup_ptr_allow_missing, bigint_bits, bigint_from_f64_trunc,
    bigint_ptr_from_bits, bigint_ref, bigint_to_inline, bytes_like_slice, call_callable0,
    call_callable2, class_name_for_error, dec_ref_bits, dict_get_in_place, dict_set_in_place,
    exception_pending, inc_ref_bits, intern_static_name, is_truthy, maybe_ptr_from_bits, molt_iter,
    molt_iter_next, molt_mul, molt_sorted_builtin, obj_from_bits, object_type_id, raise_exception,
    raise_not_iterable, runtime_state, seq_vec_ref, string_bytes, string_len, to_i64, type_of_bits,
};
#[cfg(feature = "stdlib_crypto")]
use digest::Digest;
use num_bigint::{BigInt, BigUint};
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};
#[cfg(feature = "stdlib_crypto")]
use sha2::Sha512;

#[derive(Debug)]
enum RealValue {
    Float(f64),
    IntExact(i64),
    BigIntExact(BigInt),
    IntCoerced(i64),
    BigIntCoerced(BigInt),
}

#[cfg(target_arch = "wasm32")]
fn math_log(x: f64) -> f64 {
    libm::log(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_log(x: f64) -> f64 {
    x.ln()
}

#[cfg(target_arch = "wasm32")]
fn math_log2(x: f64) -> f64 {
    libm::log2(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_log2(x: f64) -> f64 {
    x.log2()
}

#[cfg(target_arch = "wasm32")]
fn math_log10(x: f64) -> f64 {
    libm::log10(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_log10(x: f64) -> f64 {
    x.log10()
}

#[cfg(target_arch = "wasm32")]
fn math_log1p(x: f64) -> f64 {
    libm::log1p(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_log1p(x: f64) -> f64 {
    x.ln_1p()
}

#[cfg(target_arch = "wasm32")]
fn math_exp(x: f64) -> f64 {
    libm::exp(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_exp(x: f64) -> f64 {
    x.exp()
}

#[cfg(target_arch = "wasm32")]
fn math_expm1(x: f64) -> f64 {
    libm::expm1(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_expm1(x: f64) -> f64 {
    x.exp_m1()
}

#[cfg(target_arch = "wasm32")]
fn math_fma(x: f64, y: f64, z: f64) -> f64 {
    libm::fma(x, y, z)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_fma(x: f64, y: f64, z: f64) -> f64 {
    x.mul_add(y, z)
}

#[cfg(target_arch = "wasm32")]
fn math_sin(x: f64) -> f64 {
    libm::sin(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_sin(x: f64) -> f64 {
    x.sin()
}

#[cfg(target_arch = "wasm32")]
fn math_cos(x: f64) -> f64 {
    libm::cos(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_cos(x: f64) -> f64 {
    x.cos()
}

#[cfg(target_arch = "wasm32")]
fn math_acos(x: f64) -> f64 {
    libm::acos(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_acos(x: f64) -> f64 {
    x.acos()
}

#[cfg(target_arch = "wasm32")]
fn math_tan(x: f64) -> f64 {
    libm::tan(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_tan(x: f64) -> f64 {
    x.tan()
}

#[cfg(target_arch = "wasm32")]
fn math_asin(x: f64) -> f64 {
    libm::asin(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_asin(x: f64) -> f64 {
    x.asin()
}

#[cfg(target_arch = "wasm32")]
fn math_atan(x: f64) -> f64 {
    libm::atan(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_atan(x: f64) -> f64 {
    x.atan()
}

#[cfg(target_arch = "wasm32")]
fn math_atan2(y: f64, x: f64) -> f64 {
    libm::atan2(y, x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_atan2(y: f64, x: f64) -> f64 {
    y.atan2(x)
}

#[cfg(target_arch = "wasm32")]
fn math_sinh(x: f64) -> f64 {
    libm::sinh(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_sinh(x: f64) -> f64 {
    x.sinh()
}

#[cfg(target_arch = "wasm32")]
fn math_cosh(x: f64) -> f64 {
    libm::cosh(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_cosh(x: f64) -> f64 {
    x.cosh()
}

#[cfg(target_arch = "wasm32")]
fn math_tanh(x: f64) -> f64 {
    libm::tanh(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_tanh(x: f64) -> f64 {
    x.tanh()
}

#[cfg(target_arch = "wasm32")]
fn math_asinh(x: f64) -> f64 {
    libm::asinh(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_asinh(x: f64) -> f64 {
    x.asinh()
}

#[cfg(target_arch = "wasm32")]
fn math_acosh(x: f64) -> f64 {
    libm::acosh(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_acosh(x: f64) -> f64 {
    x.acosh()
}

#[cfg(target_arch = "wasm32")]
fn math_atanh(x: f64) -> f64 {
    libm::atanh(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_atanh(x: f64) -> f64 {
    x.atanh()
}

fn math_erf(x: f64) -> f64 {
    libm::erf(x)
}

fn math_erfc(x: f64) -> f64 {
    libm::erfc(x)
}

fn math_lgamma(x: f64) -> f64 {
    libm::lgamma(x)
}

#[cfg(target_arch = "wasm32")]
fn math_floor(x: f64) -> f64 {
    libm::floor(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_floor(x: f64) -> f64 {
    x.floor()
}

#[cfg(target_arch = "wasm32")]
fn math_ceil(x: f64) -> f64 {
    libm::ceil(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_ceil(x: f64) -> f64 {
    x.ceil()
}

#[cfg(target_arch = "wasm32")]
fn math_trunc(x: f64) -> f64 {
    libm::trunc(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_trunc(x: f64) -> f64 {
    x.trunc()
}

#[cfg(target_arch = "wasm32")]
fn math_sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_sqrt(x: f64) -> f64 {
    x.sqrt()
}

#[cfg(target_arch = "wasm32")]
fn math_hypot(x: f64, y: f64) -> f64 {
    libm::hypot(x, y)
}

#[cfg(not(target_arch = "wasm32"))]
fn math_hypot(x: f64, y: f64) -> f64 {
    x.hypot(y)
}

fn math_nextafter(x: f64, y: f64) -> f64 {
    libm::nextafter(x, y)
}

fn math_remainder(x: f64, y: f64) -> f64 {
    libm::remainder(x, y)
}

fn math_fmod(x: f64, y: f64) -> f64 {
    libm::fmod(x, y)
}

fn math_frexp(x: f64) -> (f64, i32) {
    libm::frexp(x)
}

fn math_ldexp(x: f64, exp: i32) -> f64 {
    libm::ldexp(x, exp)
}

fn render_float(_py: &PyToken<'_>, value: f64) -> String {
    format_obj(_py, MoltObject::from_float(value))
}

fn log2_bigint(value: &BigInt) -> f64 {
    let bits = value.bits();
    let shift = bits.saturating_sub(53);
    let top = if shift == 0 {
        value.clone()
    } else {
        value >> shift
    };
    let top_f = top.to_f64().unwrap_or(0.0);
    math_log2(top_f) + (shift as f64)
}

fn log_bigint(value: &BigInt) -> f64 {
    log2_bigint(value) * std::f64::consts::LN_2
}

fn coerce_real(_py: &PyToken<'_>, val_bits: u64) -> Option<RealValue> {
    let obj = obj_from_bits(val_bits);
    if let Some(f) = as_float_extended(obj) {
        return Some(RealValue::Float(f));
    }
    if let Some(i) = to_i64(obj) {
        return Some(RealValue::IntExact(i));
    }
    if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
        return Some(RealValue::BigIntExact(unsafe { bigint_ref(ptr) }.clone()));
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let float_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.float_name, b"__float__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(f) = as_float_extended(res_obj) {
                    return Some(RealValue::Float(f));
                }
                let owner = class_name_for_error(type_of_bits(_py, val_bits));
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                return raise_exception::<Option<RealValue>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(RealValue::IntCoerced(i));
                }
                if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = bigint_ref(big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(RealValue::BigIntCoerced(big));
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<RealValue>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    let type_label = type_name(_py, obj);
    let msg = format!("must be real number, not {type_label}");
    raise_exception::<Option<RealValue>>(_py, "TypeError", &msg)
}

fn coerce_real_named(_py: &PyToken<'_>, val_bits: u64, name: &str) -> Option<RealValue> {
    let obj = obj_from_bits(val_bits);
    if let Some(f) = as_float_extended(obj) {
        return Some(RealValue::Float(f));
    }
    if let Some(i) = to_i64(obj) {
        return Some(RealValue::IntExact(i));
    }
    if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
        return Some(RealValue::BigIntExact(unsafe { bigint_ref(ptr) }.clone()));
    }
    if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
        unsafe {
            let float_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.float_name, b"__float__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(f) = as_float_extended(res_obj) {
                    return Some(RealValue::Float(f));
                }
                let owner = class_name_for_error(type_of_bits(_py, val_bits));
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                return raise_exception::<Option<RealValue>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(RealValue::IntCoerced(i));
                }
                if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = bigint_ref(big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(RealValue::BigIntCoerced(big));
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<RealValue>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    let type_label = type_name(_py, obj);
    let msg = format!("{name}() argument must be a real number, not {type_label}");
    raise_exception::<Option<RealValue>>(_py, "TypeError", &msg)
}

fn coerce_to_f64(_py: &PyToken<'_>, value: RealValue) -> Option<f64> {
    match value {
        RealValue::Float(f) => Some(f),
        RealValue::IntExact(i) | RealValue::IntCoerced(i) => Some(i as f64),
        RealValue::BigIntExact(big) | RealValue::BigIntCoerced(big) => {
            if let Some(val) = big.to_f64() {
                Some(val)
            } else {
                raise_exception::<Option<f64>>(
                    _py,
                    "OverflowError",
                    "int too large to convert to float",
                )
            }
        }
    }
}

fn check_finite_round(_py: &PyToken<'_>, value: f64) -> Option<()> {
    if value.is_nan() {
        raise_exception::<Option<()>>(_py, "ValueError", "math domain error");
        return None;
    }
    if value.is_infinite() {
        raise_exception::<Option<()>>(_py, "OverflowError", "math range error");
        return None;
    }
    Some(())
}

fn int_bits_from_f64_trunc(_py: &PyToken<'_>, value: f64) -> u64 {
    let big = bigint_from_f64_trunc(value);
    if let Some(i) = bigint_to_inline(&big) {
        MoltObject::from_int(i).bits()
    } else {
        bigint_bits(_py, big)
    }
}

enum RoundMode {
    Floor,
    Ceil,
    Trunc,
}

fn round_float_bits(_py: &PyToken<'_>, value: f64, mode: RoundMode) -> Option<u64> {
    check_finite_round(_py, value)?;
    let rounded = match mode {
        RoundMode::Floor => math_floor(value),
        RoundMode::Ceil => math_ceil(value),
        RoundMode::Trunc => math_trunc(value),
    };
    Some(int_bits_from_f64_trunc(_py, rounded))
}

fn tuple2_bits(_py: &PyToken<'_>, first_bits: u64, second_bits: u64) -> u64 {
    let tuple_ptr = alloc_tuple(_py, &[first_bits, second_bits]);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

fn binary_type_error(_py: &PyToken<'_>, lhs_bits: u64, rhs_bits: u64, op: &str) -> u64 {
    let msg = format!(
        "unsupported operand type(s) for {op}: '{}' and '{}'",
        type_name(_py, obj_from_bits(lhs_bits)),
        type_name(_py, obj_from_bits(rhs_bits))
    );
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn math_domain_error(_py: &PyToken<'_>) -> u64 {
    raise_exception::<_>(_py, "ValueError", "math domain error")
}

fn log_domain_error(_py: &PyToken<'_>, got: Option<f64>) -> u64 {
    if let Some(value) = got {
        let rendered = render_float(_py, value);
        let msg = format!("expected a positive input, got {rendered}");
        return raise_exception::<u64>(_py, "ValueError", &msg);
    }
    raise_exception::<u64>(_py, "ValueError", "expected a positive input")
}

fn log1p_domain_error(_py: &PyToken<'_>, value: f64) -> u64 {
    let rendered = render_float(_py, value);
    let msg = format!("expected argument value > -1, got {rendered}");
    raise_exception::<u64>(_py, "ValueError", &msg)
}

fn isqrt_biguint(value: &BigUint) -> BigUint {
    if value.is_zero() {
        return BigUint::zero();
    }
    if value.is_one() {
        return BigUint::one();
    }
    if let Some(n_u64) = value.to_u64() {
        let mut x = (n_u64 as f64).sqrt() as u64;
        while x.saturating_add(1).saturating_mul(x.saturating_add(1)) <= n_u64 {
            x += 1;
        }
        while x.saturating_mul(x) > n_u64 {
            x -= 1;
        }
        return BigUint::from(x);
    }
    let bits = value.bits();
    let mut low = BigUint::zero();
    let mut high = BigUint::one() << bits.div_ceil(2);
    let one = BigUint::one();
    while &low + &one < high {
        let mid = (&low + &high) >> 1;
        let mid_sq = &mid * &mid;
        if mid_sq <= *value {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

fn collect_real_vec(_py: &PyToken<'_>, iter_bits: u64) -> Option<Vec<f64>> {
    let iter_obj = molt_iter(iter_bits);
    if obj_from_bits(iter_obj).is_none() {
        raise_not_iterable::<Option<Vec<f64>>>(_py, iter_bits);
        return None;
    }
    let mut out = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_obj);
        if exception_pending(_py) {
            return None;
        }
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return raise_exception::<Option<Vec<f64>>>(
                _py,
                "TypeError",
                "object is not an iterator",
            );
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<Option<Vec<f64>>>(
                    _py,
                    "TypeError",
                    "object is not an iterator",
                );
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return raise_exception::<Option<Vec<f64>>>(
                    _py,
                    "TypeError",
                    "object is not an iterator",
                );
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            let real = coerce_real(_py, val_bits)?;
            let f = coerce_to_f64(_py, real)?;
            out.push(f);
        }
    }
    Some(out)
}

fn kahan_sum(values: &[f64]) -> f64 {
    let mut sum = 0.0_f64;
    let mut compensation = 0.0_f64;
    for value in values {
        let y = *value - compensation;
        let t = sum + y;
        compensation = (t - sum) - y;
        sum = t;
    }
    sum
}

fn kahan_sum_sq_diff(values: &[f64], mean: f64) -> f64 {
    let mut sum = 0.0_f64;
    let mut compensation = 0.0_f64;
    for value in values {
        let diff = *value - mean;
        let term = diff * diff;
        let y = term - compensation;
        let t = sum + y;
        compensation = (t - sum) - y;
        sum = t;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
unsafe fn sum_f64_simd_x86_avx(values: &[f64]) -> f64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut acc = _mm256_setzero_pd();
    while i + 4 <= values.len() {
        let v = _mm256_loadu_pd(values.as_ptr().add(i));
        acc = _mm256_add_pd(acc, v);
        i += 4;
    }
    let mut lanes = [0.0_f64; 4];
    _mm256_storeu_pd(lanes.as_mut_ptr(), acc);
    let mut sum = lanes.iter().sum::<f64>();
    for &v in &values[i..] {
        sum += v;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn sum_f64_simd_x86_sse2(values: &[f64]) -> f64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut acc = _mm_setzero_pd();
    while i + 2 <= values.len() {
        let v = _mm_loadu_pd(values.as_ptr().add(i));
        acc = _mm_add_pd(acc, v);
        i += 2;
    }
    let mut lanes = [0.0_f64; 2];
    _mm_storeu_pd(lanes.as_mut_ptr(), acc);
    let mut sum = lanes[0] + lanes[1];
    for &v in &values[i..] {
        sum += v;
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn sum_f64_simd_aarch64(values: &[f64]) -> f64 {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut acc = vdupq_n_f64(0.0);
        while i + 2 <= values.len() {
            let v = vld1q_f64(values.as_ptr().add(i));
            acc = vaddq_f64(acc, v);
            i += 2;
        }
        let mut lanes = [0.0_f64; 2];
        vst1q_f64(lanes.as_mut_ptr(), acc);
        let mut sum = lanes[0] + lanes[1];
        for &v in &values[i..] {
            sum += v;
        }
        sum
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn sum_f64_simd_wasm32(values: &[f64]) -> f64 {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        let mut acc = f64x2_splat(0.0);
        while i + 2 <= values.len() {
            let v = v128_load(values.as_ptr().add(i) as *const v128);
            acc = f64x2_add(acc, v);
            i += 2;
        }
        let mut sum = f64x2_extract_lane::<0>(acc) + f64x2_extract_lane::<1>(acc);
        for &v in &values[i..] {
            sum += v;
        }
        sum
    }
}

fn sum_f64_simd(values: &[f64]) -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx") {
            return unsafe { sum_f64_simd_x86_avx(values) };
        }
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { sum_f64_simd_x86_sse2(values) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { sum_f64_simd_aarch64(values) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        if values.len() >= 2 {
            return unsafe { sum_f64_simd_wasm32(values) };
        }
    }
    kahan_sum(values)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx")]
unsafe fn sum_sq_diff_f64_simd_x86_avx(values: &[f64], mean: f64) -> f64 {
    use std::arch::x86_64::*;
    let mean_v = _mm256_set1_pd(mean);
    let mut i = 0usize;
    let mut acc = _mm256_setzero_pd();
    while i + 4 <= values.len() {
        let v = _mm256_loadu_pd(values.as_ptr().add(i));
        let d = _mm256_sub_pd(v, mean_v);
        acc = _mm256_add_pd(acc, _mm256_mul_pd(d, d));
        i += 4;
    }
    let mut lanes = [0.0_f64; 4];
    _mm256_storeu_pd(lanes.as_mut_ptr(), acc);
    let mut sum = lanes.iter().sum::<f64>();
    for &v in &values[i..] {
        let d = v - mean;
        sum += d * d;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn sum_sq_diff_f64_simd_x86_sse2(values: &[f64], mean: f64) -> f64 {
    use std::arch::x86_64::*;
    let mean_v = _mm_set1_pd(mean);
    let mut i = 0usize;
    let mut acc = _mm_setzero_pd();
    while i + 2 <= values.len() {
        let v = _mm_loadu_pd(values.as_ptr().add(i));
        let d = _mm_sub_pd(v, mean_v);
        acc = _mm_add_pd(acc, _mm_mul_pd(d, d));
        i += 2;
    }
    let mut lanes = [0.0_f64; 2];
    _mm_storeu_pd(lanes.as_mut_ptr(), acc);
    let mut sum = lanes[0] + lanes[1];
    for &v in &values[i..] {
        let d = v - mean;
        sum += d * d;
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "neon")]
unsafe fn sum_sq_diff_f64_simd_aarch64(values: &[f64], mean: f64) -> f64 {
    unsafe {
        use std::arch::aarch64::*;
        let mean_v = vdupq_n_f64(mean);
        let mut i = 0usize;
        let mut acc = vdupq_n_f64(0.0);
        while i + 2 <= values.len() {
            let v = vld1q_f64(values.as_ptr().add(i));
            let d = vsubq_f64(v, mean_v);
            acc = vaddq_f64(acc, vmulq_f64(d, d));
            i += 2;
        }
        let mut lanes = [0.0_f64; 2];
        vst1q_f64(lanes.as_mut_ptr(), acc);
        let mut sum = lanes[0] + lanes[1];
        for &v in &values[i..] {
            let d = v - mean;
            sum += d * d;
        }
        sum
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn sum_sq_diff_f64_simd_wasm32(values: &[f64], mean: f64) -> f64 {
    unsafe {
        use std::arch::wasm32::*;
        let mean_v = f64x2_splat(mean);
        let mut i = 0usize;
        let mut acc = f64x2_splat(0.0);
        while i + 2 <= values.len() {
            let v = v128_load(values.as_ptr().add(i) as *const v128);
            let d = f64x2_sub(v, mean_v);
            acc = f64x2_add(acc, f64x2_mul(d, d));
            i += 2;
        }
        let mut sum = f64x2_extract_lane::<0>(acc) + f64x2_extract_lane::<1>(acc);
        for &v in &values[i..] {
            let d = v - mean;
            sum += d * d;
        }
        sum
    }
}

fn sum_sq_diff_f64_simd(values: &[f64], mean: f64) -> f64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx") {
            return unsafe { sum_sq_diff_f64_simd_x86_avx(values, mean) };
        }
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { sum_sq_diff_f64_simd_x86_sse2(values, mean) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { sum_sq_diff_f64_simd_aarch64(values, mean) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        if values.len() >= 2 {
            return unsafe { sum_sq_diff_f64_simd_wasm32(values, mean) };
        }
    }
    kahan_sum_sq_diff(values, mean)
}

fn clamp_slice_step1_index(raw: i64, len: usize) -> usize {
    let len_i = len as i128;
    let mut idx = raw as i128;
    if idx < 0 {
        idx += len_i;
        if idx < 0 {
            idx = 0;
        }
    }
    if idx > len_i {
        idx = len_i;
    }
    idx as usize
}

fn normalize_slice_step1_bounds(
    _py: &PyToken<'_>,
    len: usize,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> Option<(usize, usize)> {
    let index_msg = "slice indices must be integers or None or have an __index__ method";
    let has_start = is_truthy(_py, obj_from_bits(has_start_bits));
    let has_end = is_truthy(_py, obj_from_bits(has_end_bits));
    let start = if has_start {
        let idx = index_i64_from_obj(_py, start_bits, index_msg);
        if exception_pending(_py) {
            return None;
        }
        clamp_slice_step1_index(idx, len)
    } else {
        0
    };
    let end = if has_end {
        let idx = index_i64_from_obj(_py, end_bits, index_msg);
        if exception_pending(_py) {
            return None;
        }
        clamp_slice_step1_index(idx, len)
    } else {
        len
    };
    Some((start, end))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_log(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        match value {
            RealValue::Float(f) => {
                if f.is_nan() {
                    return float_result_bits(_py, f);
                }
                if f.is_infinite() {
                    if f.is_sign_negative() {
                        return log_domain_error(_py, Some(f));
                    }
                    return float_result_bits(_py, f);
                }
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                float_result_bits(_py, math_log(f))
            }
            RealValue::IntExact(i) => {
                if i <= 0 {
                    return log_domain_error(_py, None);
                }
                float_result_bits(_py, math_log(i as f64))
            }
            RealValue::BigIntExact(big) => {
                if big.is_negative() || big.is_zero() {
                    return log_domain_error(_py, None);
                }
                float_result_bits(_py, log_bigint(&big))
            }
            RealValue::IntCoerced(i) => {
                let f = i as f64;
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                float_result_bits(_py, math_log(f))
            }
            RealValue::BigIntCoerced(big) => {
                let Some(f) = big.to_f64() else {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                };
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                float_result_bits(_py, math_log(f))
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_log2(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        match value {
            RealValue::Float(f) => {
                if f.is_nan() {
                    return float_result_bits(_py, f);
                }
                if f.is_infinite() {
                    if f.is_sign_negative() {
                        return log_domain_error(_py, Some(f));
                    }
                    return float_result_bits(_py, f);
                }
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                float_result_bits(_py, math_log2(f))
            }
            RealValue::IntExact(i) => {
                if i <= 0 {
                    return log_domain_error(_py, None);
                }
                float_result_bits(_py, math_log2(i as f64))
            }
            RealValue::BigIntExact(big) => {
                if big.is_negative() || big.is_zero() {
                    return log_domain_error(_py, None);
                }
                float_result_bits(_py, log2_bigint(&big))
            }
            RealValue::IntCoerced(i) => {
                let f = i as f64;
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                float_result_bits(_py, math_log2(f))
            }
            RealValue::BigIntCoerced(big) => {
                let Some(f) = big.to_f64() else {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                };
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                float_result_bits(_py, math_log2(f))
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_log10(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        match value {
            RealValue::Float(f) => {
                if f.is_nan() {
                    return float_result_bits(_py, f);
                }
                if f.is_infinite() {
                    if f.is_sign_negative() {
                        return log_domain_error(_py, Some(f));
                    }
                    return float_result_bits(_py, f);
                }
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                float_result_bits(_py, math_log10(f))
            }
            RealValue::IntExact(i) => {
                if i <= 0 {
                    return log_domain_error(_py, None);
                }
                float_result_bits(_py, math_log10(i as f64))
            }
            RealValue::BigIntExact(big) => {
                if big.is_negative() || big.is_zero() {
                    return log_domain_error(_py, None);
                }
                float_result_bits(_py, log_bigint(&big) / std::f64::consts::LN_10)
            }
            RealValue::IntCoerced(i) => {
                let f = i as f64;
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                float_result_bits(_py, math_log10(f))
            }
            RealValue::BigIntCoerced(big) => {
                if big.is_negative() || big.is_zero() {
                    return log_domain_error(_py, None);
                }
                float_result_bits(_py, log_bigint(&big) / std::f64::consts::LN_10)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_log1p(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                return log1p_domain_error(_py, f);
            }
            return float_result_bits(_py, f);
        }
        if f <= -1.0 {
            return log1p_domain_error(_py, f);
        }
        float_result_bits(_py, math_log1p(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_exp(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                return float_result_bits(_py, 0.0);
            }
            return float_result_bits(_py, f);
        }
        let out = math_exp(f);
        if out.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_expm1(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                return float_result_bits(_py, -1.0);
            }
            return float_result_bits(_py, f);
        }
        let out = math_expm1(f);
        if out.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_fma(x_bits: u64, y_bits: u64, z_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(x_val) = coerce_real(_py, x_bits) else {
            return MoltObject::none().bits();
        };
        let Some(y_val) = coerce_real(_py, y_bits) else {
            return MoltObject::none().bits();
        };
        let Some(z_val) = coerce_real(_py, z_bits) else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_val) else {
            return MoltObject::none().bits();
        };
        let Some(y) = coerce_to_f64(_py, y_val) else {
            return MoltObject::none().bits();
        };
        let Some(z) = coerce_to_f64(_py, z_val) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_fma(x, y, z))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_sin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            let rendered = render_float(_py, f);
            let msg = format!("expected a finite input, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        float_result_bits(_py, math_sin(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_cos(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            let rendered = render_float(_py, f);
            let msg = format!("expected a finite input, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        float_result_bits(_py, math_cos(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_acos(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if !(-1.0..=1.0).contains(&f) {
            let rendered = render_float(_py, f);
            let msg = format!("expected a number in range from -1 up to 1, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        float_result_bits(_py, math_acos(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_tan(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            let rendered = render_float(_py, f);
            let msg = format!("expected a finite input, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        float_result_bits(_py, math_tan(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_asin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if !(-1.0..=1.0).contains(&f) {
            let rendered = render_float(_py, f);
            let msg = format!("expected a number in range from -1 up to 1, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        float_result_bits(_py, math_asin(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_atan(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_atan(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_atan2(y_bits: u64, x_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(y_val) = coerce_real(_py, y_bits) else {
            return MoltObject::none().bits();
        };
        let Some(x_val) = coerce_real(_py, x_bits) else {
            return MoltObject::none().bits();
        };
        let Some(y) = coerce_to_f64(_py, y_val) else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_val) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_atan2(y, x))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_sinh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        let out = math_sinh(f);
        if out.is_infinite() && !f.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_cosh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        let out = math_cosh(f);
        if out.is_infinite() && !f.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_tanh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_tanh(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_asinh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_asinh(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_acosh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f < 1.0 {
            let rendered = render_float(_py, f);
            let msg = format!("expected a number greater than or equal to 1, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        float_result_bits(_py, math_acosh(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_atanh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f <= -1.0 || f >= 1.0 {
            return math_domain_error(_py);
        }
        float_result_bits(_py, math_atanh(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_gamma(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                let rendered = render_float(_py, f);
                let msg = format!("expected a noninteger or positive integer, got {rendered}");
                return raise_exception::<_>(_py, "ValueError", &msg);
            }
            return float_result_bits(_py, f);
        }
        if f <= 0.0 && f.fract() == 0.0 {
            let rendered = render_float(_py, f);
            let msg = format!("expected a noninteger or positive integer, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let out = libm::tgamma(f);
        if out.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_erf(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_erf(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_erfc(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_erfc(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_lgamma(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            return float_result_bits(_py, f.abs());
        }
        if f <= 0.0 && f.fract() == 0.0 {
            let rendered = render_float(_py, f);
            let msg = format!("expected a noninteger or positive integer, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        float_result_bits(_py, math_lgamma(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_isfinite(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "isfinite") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_bool(f.is_finite()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_isinf(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "isinf") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_bool(f.is_infinite()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_isnan(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        let Some(value) = coerce_real_named(_py, val_bits, "isnan") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_bool(f.is_nan()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_fabs(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "fabs") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, f.abs())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_copysign(x_bits: u64, y_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(x_val) = coerce_real_named(_py, x_bits, "copysign") else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_val) else {
            return MoltObject::none().bits();
        };
        let Some(y_val) = coerce_real_named(_py, y_bits, "copysign") else {
            return MoltObject::none().bits();
        };
        let Some(y) = coerce_to_f64(_py, y_val) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, x.copysign(y))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_sqrt(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "sqrt") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                return raise_exception::<_>(_py, "ValueError", "math domain error");
            }
            return float_result_bits(_py, f);
        }
        if f < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "math domain error");
        }
        float_result_bits(_py, math_sqrt(f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_floor(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        if let Some(i) = to_i64(obj) {
            return MoltObject::from_int(i).bits();
        }
        if bigint_ptr_from_bits(val_bits).is_some() {
            return val_bits;
        }
        if let Some(f) = obj.as_float() {
            let Some(bits) = round_float_bits(_py, f, RoundMode::Floor) else {
                return MoltObject::none().bits();
            };
            return bits;
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let floor_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.floor_name, b"__floor__");
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, floor_name_bits) {
                    let callable_bits = molt_is_callable(call_bits);
                    let callable_ok = is_truthy(_py, obj_from_bits(callable_bits));
                    if obj_from_bits(callable_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, callable_bits);
                    }
                    if callable_ok {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        return res_bits;
                    }
                    dec_ref_bits(_py, call_bits);
                }
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        let Some(value) = coerce_real_named(_py, val_bits, "floor") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        let Some(bits) = round_float_bits(_py, f, RoundMode::Floor) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_ceil(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        if let Some(i) = to_i64(obj) {
            return MoltObject::from_int(i).bits();
        }
        if bigint_ptr_from_bits(val_bits).is_some() {
            return val_bits;
        }
        if let Some(f) = obj.as_float() {
            let Some(bits) = round_float_bits(_py, f, RoundMode::Ceil) else {
                return MoltObject::none().bits();
            };
            return bits;
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let ceil_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.ceil_name, b"__ceil__");
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, ceil_name_bits) {
                    let callable_bits = molt_is_callable(call_bits);
                    let callable_ok = is_truthy(_py, obj_from_bits(callable_bits));
                    if obj_from_bits(callable_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, callable_bits);
                    }
                    if callable_ok {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        return res_bits;
                    }
                    dec_ref_bits(_py, call_bits);
                }
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        let Some(value) = coerce_real_named(_py, val_bits, "ceil") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        let Some(bits) = round_float_bits(_py, f, RoundMode::Ceil) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_trunc(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        if let Some(i) = to_i64(obj) {
            return MoltObject::from_int(i).bits();
        }
        if bigint_ptr_from_bits(val_bits).is_some() {
            return val_bits;
        }
        if let Some(f) = obj.as_float() {
            let Some(bits) = round_float_bits(_py, f, RoundMode::Trunc) else {
                return MoltObject::none().bits();
            };
            return bits;
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let trunc_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.trunc_name, b"__trunc__");
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, trunc_name_bits) {
                    let callable_bits = molt_is_callable(call_bits);
                    let callable_ok = is_truthy(_py, obj_from_bits(callable_bits));
                    if obj_from_bits(callable_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, callable_bits);
                    }
                    if callable_ok {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        return res_bits;
                    }
                    dec_ref_bits(_py, call_bits);
                }
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        let Some(value) = coerce_real_named(_py, val_bits, "trunc") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        let Some(bits) = round_float_bits(_py, f, RoundMode::Trunc) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_fmod(x_bits: u64, y_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(x_val) = coerce_real_named(_py, x_bits, "fmod") else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_val) else {
            return MoltObject::none().bits();
        };
        let Some(y_val) = coerce_real_named(_py, y_bits, "fmod") else {
            return MoltObject::none().bits();
        };
        let Some(y) = coerce_to_f64(_py, y_val) else {
            return MoltObject::none().bits();
        };
        if y == 0.0 {
            return raise_exception::<_>(_py, "ValueError", "math domain error");
        }
        if x.is_infinite() {
            return raise_exception::<_>(_py, "ValueError", "math domain error");
        }
        if x.is_nan() || y.is_nan() {
            return float_result_bits(_py, f64::NAN);
        }
        if y.is_infinite() {
            return float_result_bits(_py, x);
        }
        float_result_bits(_py, math_fmod(x, y))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_modf(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "modf") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            let bits = float_result_bits(_py, f);
            return tuple2_bits(_py, bits, bits);
        }
        if f.is_infinite() {
            let frac = float_result_bits(_py, 0.0_f64.copysign(f));
            let int = float_result_bits(_py, f);
            return tuple2_bits(_py, frac, int);
        }
        if f == 0.0 {
            let bits = float_result_bits(_py, f);
            return tuple2_bits(_py, bits, bits);
        }
        let int_part = math_trunc(f);
        let frac_part = f - int_part;
        let frac_bits = float_result_bits(_py, frac_part);
        let int_bits = float_result_bits(_py, int_part);
        tuple2_bits(_py, frac_bits, int_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_frexp(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "frexp") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() || f.is_infinite() || f == 0.0 {
            let frac_bits = float_result_bits(_py, f);
            let exp_bits = MoltObject::from_int(0).bits();
            return tuple2_bits(_py, frac_bits, exp_bits);
        }
        let (mantissa, exp) = math_frexp(f);
        let frac_bits = float_result_bits(_py, mantissa);
        let exp_bits = MoltObject::from_int(exp as i64).bits();
        tuple2_bits(_py, frac_bits, exp_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_ldexp(val_bits: u64, exp_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "ldexp") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        let exp = index_i64_from_obj(_py, exp_bits, "ldexp() second argument must be an integer");
        if f.is_nan() || f.is_infinite() {
            return float_result_bits(_py, f);
        }
        if exp > i32::MAX as i64 {
            if f == 0.0 {
                return float_result_bits(_py, f);
            }
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        if exp < i32::MIN as i64 {
            if f == 0.0 {
                return float_result_bits(_py, f);
            }
            return float_result_bits(_py, 0.0_f64.copysign(f));
        }
        let out = math_ldexp(f, exp as i32);
        if out.is_infinite() && f != 0.0 {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_isclose(a_bits: u64, b_bits: u64, rel_bits: u64, abs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(rel_val) = coerce_real_named(_py, rel_bits, "isclose") else {
            return MoltObject::none().bits();
        };
        let Some(abs_val) = coerce_real_named(_py, abs_bits, "isclose") else {
            return MoltObject::none().bits();
        };
        let Some(rel_tol) = coerce_to_f64(_py, rel_val) else {
            return MoltObject::none().bits();
        };
        let Some(abs_tol) = coerce_to_f64(_py, abs_val) else {
            return MoltObject::none().bits();
        };
        if rel_tol < 0.0 || abs_tol < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "tolerances must be non-negative");
        }
        let Some(a_val) = coerce_real_named(_py, a_bits, "isclose") else {
            return MoltObject::none().bits();
        };
        let Some(b_val) = coerce_real_named(_py, b_bits, "isclose") else {
            return MoltObject::none().bits();
        };
        let Some(a) = coerce_to_f64(_py, a_val) else {
            return MoltObject::none().bits();
        };
        let Some(b) = coerce_to_f64(_py, b_val) else {
            return MoltObject::none().bits();
        };
        if a == b {
            return MoltObject::from_bool(true).bits();
        }
        if a.is_nan() || b.is_nan() {
            return MoltObject::from_bool(false).bits();
        }
        if a.is_infinite() || b.is_infinite() {
            return MoltObject::from_bool(false).bits();
        }
        let diff = (a - b).abs();
        let bound = (rel_tol * a.abs().max(b.abs())).max(abs_tol);
        MoltObject::from_bool(diff <= bound).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_prod(iter_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        let mut total_bits = start_bits;
        let mut total_owned = false;
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    if !total_owned {
                        inc_ref_bits(_py, total_bits);
                    }
                    return total_bits;
                }
                let next_bits = molt_mul(total_bits, val_bits);
                if obj_from_bits(next_bits).is_none() {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return binary_type_error(_py, total_bits, val_bits, "*");
                }
                total_bits = next_bits;
                total_owned = true;
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_fsum(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        let mut partials: Vec<f64> = Vec::new();
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let Some(real) = coerce_real_named(_py, val_bits, "fsum") else {
                    return MoltObject::none().bits();
                };
                let Some(mut x) = coerce_to_f64(_py, real) else {
                    return MoltObject::none().bits();
                };
                let mut j = 0usize;
                let mut i = 0usize;
                while i < partials.len() {
                    let mut y = partials[i];
                    i += 1;
                    if x.abs() < y.abs() {
                        std::mem::swap(&mut x, &mut y);
                    }
                    let hi = x + y;
                    let lo = y - (hi - x);
                    if lo != 0.0 {
                        if j < partials.len() {
                            partials[j] = lo;
                        } else {
                            partials.push(lo);
                        }
                        j += 1;
                    }
                    x = hi;
                }
                if j < partials.len() {
                    partials[j] = x;
                    j += 1;
                    partials.truncate(j);
                } else {
                    partials.push(x);
                }
            }
        }
        let mut sum = 0.0_f64;
        for val in partials.iter().rev() {
            sum += *val;
        }
        float_result_bits(_py, sum)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_gcd(args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let args_obj = obj_from_bits(args_bits);
        let Some(args_ptr) = args_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "gcd() expected arguments");
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "gcd() expected arguments");
            }
            let elems = seq_vec_ref(args_ptr);
            if elems.is_empty() {
                return MoltObject::from_int(0).bits();
            }
            let mut result = BigInt::zero();
            for &val_bits in elems {
                let msg = format!(
                    "gcd() argument must be an integer, not {}",
                    type_name(_py, obj_from_bits(val_bits))
                );
                let Some(mut value) = index_bigint_from_obj(_py, val_bits, &msg) else {
                    return MoltObject::none().bits();
                };
                if value.is_negative() {
                    value = -value;
                }
                if result.is_zero() {
                    result = value;
                } else {
                    result = result.gcd(&value);
                }
            }
            int_bits_from_bigint(_py, result)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_lcm(args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let args_obj = obj_from_bits(args_bits);
        let Some(args_ptr) = args_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "lcm() expected arguments");
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "lcm() expected arguments");
            }
            let elems = seq_vec_ref(args_ptr);
            if elems.is_empty() {
                return MoltObject::from_int(1).bits();
            }
            let mut result = BigInt::one();
            for &val_bits in elems {
                let msg = format!(
                    "lcm() argument must be an integer, not {}",
                    type_name(_py, obj_from_bits(val_bits))
                );
                let Some(mut value) = index_bigint_from_obj(_py, val_bits, &msg) else {
                    return MoltObject::none().bits();
                };
                if value.is_negative() {
                    value = -value;
                }
                if result.is_zero() || value.is_zero() {
                    result = BigInt::zero();
                    continue;
                }
                result = result.lcm(&value);
            }
            int_bits_from_bigint(_py, result)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_factorial(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let msg = format!(
            "factorial() argument must be an integer, not {}",
            type_name(_py, obj_from_bits(val_bits))
        );
        let Some(n_val) = index_bigint_from_obj(_py, val_bits, &msg) else {
            return MoltObject::none().bits();
        };
        if n_val.is_negative() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "factorial() not defined for negative values",
            );
        }
        if n_val.is_zero() {
            return MoltObject::from_int(1).bits();
        }
        let mut result = BigInt::one();
        if let Some(n_u64) = n_val.to_u64() {
            for i in 2..=n_u64 {
                result *= i;
            }
            return int_bits_from_bigint(_py, result);
        }
        let mut i = BigInt::from(2);
        while i <= n_val {
            result *= &i;
            i += 1;
        }
        int_bits_from_bigint(_py, result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_comb(n_bits: u64, k_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n_msg = format!(
            "comb() argument must be an integer, not {}",
            type_name(_py, obj_from_bits(n_bits))
        );
        let k_msg = format!(
            "comb() argument must be an integer, not {}",
            type_name(_py, obj_from_bits(k_bits))
        );
        let Some(n_val) = index_bigint_from_obj(_py, n_bits, &n_msg) else {
            return MoltObject::none().bits();
        };
        let Some(k_val) = index_bigint_from_obj(_py, k_bits, &k_msg) else {
            return MoltObject::none().bits();
        };
        if n_val.is_negative() {
            return raise_exception::<_>(_py, "ValueError", "n must be a non-negative integer");
        }
        if k_val.is_negative() {
            return raise_exception::<_>(_py, "ValueError", "k must be a non-negative integer");
        }
        if k_val > n_val {
            return MoltObject::from_int(0).bits();
        }
        if k_val.is_zero() {
            return MoltObject::from_int(1).bits();
        }
        let n_minus_k = &n_val - &k_val;
        let k_val = if n_minus_k < k_val { n_minus_k } else { k_val };
        if let (Some(n_u64), Some(k_u64)) = (n_val.to_u64(), k_val.to_u64()) {
            let mut result = BigInt::one();
            let start = n_u64 - k_u64;
            for i in 1..=k_u64 {
                let term = start + i;
                result = result * term / i;
            }
            return int_bits_from_bigint(_py, result);
        }
        let mut result = BigInt::one();
        let n_minus_k = &n_val - &k_val;
        let mut i = BigInt::from(1);
        while i <= k_val {
            let term = &n_minus_k + &i;
            result = result * term / &i;
            i += 1;
        }
        int_bits_from_bigint(_py, result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_perm(n_bits: u64, k_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n_msg = format!(
            "perm() argument must be an integer, not {}",
            type_name(_py, obj_from_bits(n_bits))
        );
        let Some(n_val) = index_bigint_from_obj(_py, n_bits, &n_msg) else {
            return MoltObject::none().bits();
        };
        let k_val = if obj_from_bits(k_bits).is_none() {
            n_val.clone()
        } else {
            let k_msg = format!(
                "perm() argument must be an integer, not {}",
                type_name(_py, obj_from_bits(k_bits))
            );
            let Some(val) = index_bigint_from_obj(_py, k_bits, &k_msg) else {
                return MoltObject::none().bits();
            };
            val
        };
        if n_val.is_negative() {
            return raise_exception::<_>(_py, "ValueError", "n must be a non-negative integer");
        }
        if k_val.is_negative() {
            return raise_exception::<_>(_py, "ValueError", "k must be a non-negative integer");
        }
        if k_val > n_val {
            return MoltObject::from_int(0).bits();
        }
        if k_val.is_zero() {
            return MoltObject::from_int(1).bits();
        }
        if let (Some(n_u64), Some(k_u64)) = (n_val.to_u64(), k_val.to_u64()) {
            let mut result = BigInt::one();
            let start = n_u64 - k_u64 + 1;
            for i in start..=n_u64 {
                result *= i;
            }
            return int_bits_from_bigint(_py, result);
        }
        let mut result = BigInt::one();
        let start = &n_val - &k_val + BigInt::from(1);
        let mut i = start.clone();
        while i <= n_val {
            result *= &i;
            i += 1;
        }
        int_bits_from_bigint(_py, result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_degrees(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "degrees") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        let out = f * (180.0 / std::f64::consts::PI);
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_radians(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "radians") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        let out = f * (std::f64::consts::PI / 180.0);
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_hypot(args_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let args_obj = obj_from_bits(args_bits);
        let Some(args_ptr) = args_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "hypot() expected arguments");
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "hypot() expected arguments");
            }
            let elems = seq_vec_ref(args_ptr);
            if elems.is_empty() {
                return float_result_bits(_py, 0.0);
            }
            // Pre-extract all f64 values for SIMD processing
            let mut vals: Vec<f64> = Vec::with_capacity(elems.len());
            for &val_bits in elems {
                let Some(value) = coerce_real_named(_py, val_bits, "hypot") else {
                    return MoltObject::none().bits();
                };
                let Some(f) = coerce_to_f64(_py, value) else {
                    return MoltObject::none().bits();
                };
                vals.push(f);
            }
            // SIMD-accelerated sum-of-squares with sqrt
            let n = vals.len();
            let mut sum_sq = 0.0_f64;
            let mut i = 0usize;
            #[cfg(target_arch = "aarch64")]
            {
                if n >= 2 && std::arch::is_aarch64_feature_detected!("neon") {
                    use std::arch::aarch64::*;
                    let mut vec_sum = vdupq_n_f64(0.0);
                    while i + 2 <= n {
                        let v = vld1q_f64(vals.as_ptr().add(i));
                        vec_sum = vfmaq_f64(vec_sum, v, v);
                        i += 2;
                    }
                    let mut lanes = [0.0f64; 2];
                    vst1q_f64(lanes.as_mut_ptr(), vec_sum);
                    sum_sq = lanes[0] + lanes[1];
                }
            }
            #[cfg(target_arch = "x86_64")]
            {
                if n >= 4 && std::arch::is_x86_feature_detected!("avx2") {
                    unsafe {
                        use std::arch::x86_64::*;
                        let mut vec_sum = _mm256_setzero_pd();
                        while i + 4 <= n {
                            let v = _mm256_loadu_pd(vals.as_ptr().add(i));
                            vec_sum = _mm256_add_pd(vec_sum, _mm256_mul_pd(v, v));
                            i += 4;
                        }
                        let mut lanes = [0.0f64; 4];
                        _mm256_storeu_pd(lanes.as_mut_ptr(), vec_sum);
                        sum_sq = lanes[0] + lanes[1] + lanes[2] + lanes[3];
                    }
                } else if n >= 2 && std::arch::is_x86_feature_detected!("sse2") {
                    unsafe {
                        use std::arch::x86_64::*;
                        let mut vec_sum = _mm_setzero_pd();
                        while i + 2 <= n {
                            let v = _mm_loadu_pd(vals.as_ptr().add(i));
                            vec_sum = _mm_add_pd(vec_sum, _mm_mul_pd(v, v));
                            i += 2;
                        }
                        let mut lanes = [0.0f64; 2];
                        _mm_storeu_pd(lanes.as_mut_ptr(), vec_sum);
                        sum_sq = lanes[0] + lanes[1];
                    }
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                if n >= 2 {
                    unsafe {
                        use std::arch::wasm32::*;
                        let mut vec_sum = f64x2_splat(0.0);
                        while i + 2 <= n {
                            let v = v128_load(vals.as_ptr().add(i) as *const v128);
                            vec_sum = f64x2_add(vec_sum, f64x2_mul(v, v));
                            i += 2;
                        }
                        sum_sq =
                            f64x2_extract_lane::<0>(vec_sum) + f64x2_extract_lane::<1>(vec_sum);
                    }
                }
            }
            for &val in vals.iter().take(n).skip(i) {
                sum_sq += val * val;
            }
            if !sum_sq.is_finite() {
                // Fallback to iterative hypot for numerical stability
                let mut total = 0.0_f64;
                for &f in &vals {
                    total = math_hypot(total, f);
                }
                return float_result_bits(_py, total);
            }
            float_result_bits(_py, sum_sq.sqrt())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_dist(p_bits: u64, q_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(p_vals) = collect_real_vec(_py, p_bits) else {
            return MoltObject::none().bits();
        };
        let Some(q_vals) = collect_real_vec(_py, q_bits) else {
            return MoltObject::none().bits();
        };
        if p_vals.len() != q_vals.len() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "both points must have the same number of dimensions",
            );
        }
        // SIMD-accelerated squared-difference sum for Euclidean distance.
        // For numerical stability, we use the sqrt(sum-of-squares) approach
        // which is safe when values are finite and not near overflow/underflow.
        let n = p_vals.len();
        let mut sum_sq = 0.0_f64;
        let mut i = 0usize;
        #[cfg(target_arch = "aarch64")]
        {
            if n >= 2 && std::arch::is_aarch64_feature_detected!("neon") {
                unsafe {
                    use std::arch::aarch64::*;
                    let mut vec_sum = vdupq_n_f64(0.0);
                    while i + 2 <= n {
                        let vp = vld1q_f64([p_vals[i], p_vals[i + 1]].as_ptr());
                        let vq = vld1q_f64([q_vals[i], q_vals[i + 1]].as_ptr());
                        let diff = vsubq_f64(vp, vq);
                        vec_sum = vfmaq_f64(vec_sum, diff, diff);
                        i += 2;
                    }
                    let mut lanes = [0.0f64; 2];
                    vst1q_f64(lanes.as_mut_ptr(), vec_sum);
                    sum_sq = lanes[0] + lanes[1];
                }
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            if n >= 4 && std::arch::is_x86_feature_detected!("avx2") {
                unsafe {
                    use std::arch::x86_64::*;
                    let mut vec_sum = _mm256_setzero_pd();
                    while i + 4 <= n {
                        let vp = _mm256_loadu_pd(
                            [p_vals[i], p_vals[i + 1], p_vals[i + 2], p_vals[i + 3]].as_ptr(),
                        );
                        let vq = _mm256_loadu_pd(
                            [q_vals[i], q_vals[i + 1], q_vals[i + 2], q_vals[i + 3]].as_ptr(),
                        );
                        let diff = _mm256_sub_pd(vp, vq);
                        vec_sum = _mm256_add_pd(vec_sum, _mm256_mul_pd(diff, diff));
                        i += 4;
                    }
                    let mut lanes = [0.0f64; 4];
                    _mm256_storeu_pd(lanes.as_mut_ptr(), vec_sum);
                    sum_sq = lanes[0] + lanes[1] + lanes[2] + lanes[3];
                }
            } else if n >= 2 && std::arch::is_x86_feature_detected!("sse2") {
                unsafe {
                    use std::arch::x86_64::*;
                    let mut vec_sum = _mm_setzero_pd();
                    while i + 2 <= n {
                        let vp = _mm_loadu_pd([p_vals[i], p_vals[i + 1]].as_ptr());
                        let vq = _mm_loadu_pd([q_vals[i], q_vals[i + 1]].as_ptr());
                        let diff = _mm_sub_pd(vp, vq);
                        vec_sum = _mm_add_pd(vec_sum, _mm_mul_pd(diff, diff));
                        i += 2;
                    }
                    let mut lanes = [0.0f64; 2];
                    _mm_storeu_pd(lanes.as_mut_ptr(), vec_sum);
                    sum_sq = lanes[0] + lanes[1];
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            if n >= 2 {
                unsafe {
                    use std::arch::wasm32::*;
                    let mut vec_sum = f64x2_splat(0.0);
                    while i + 2 <= n {
                        let vp = v128_load([p_vals[i], p_vals[i + 1]].as_ptr() as *const v128);
                        let vq = v128_load([q_vals[i], q_vals[i + 1]].as_ptr() as *const v128);
                        let diff = f64x2_sub(vp, vq);
                        vec_sum = f64x2_add(vec_sum, f64x2_mul(diff, diff));
                        i += 2;
                    }
                    sum_sq = f64x2_extract_lane::<0>(vec_sum) + f64x2_extract_lane::<1>(vec_sum);
                }
            }
        }
        // Scalar tail
        for j in i..n {
            let d = p_vals[j] - q_vals[j];
            sum_sq += d * d;
        }
        // Check for inf/nan: if overflow happened, fall back to iterative hypot
        if !sum_sq.is_finite() {
            let mut total = 0.0_f64;
            for (lhs, rhs) in p_vals.iter().zip(q_vals.iter()) {
                total = math_hypot(total, lhs - rhs);
            }
            return float_result_bits(_py, total);
        }
        float_result_bits(_py, sum_sq.sqrt())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_isqrt(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let msg = format!(
            "isqrt() argument must be an integer, not {}",
            type_name(_py, obj_from_bits(val_bits))
        );
        let Some(value) = index_bigint_from_obj(_py, val_bits, &msg) else {
            return MoltObject::none().bits();
        };
        if value.is_negative() {
            return raise_exception::<_>(_py, "ValueError", "isqrt() argument must be nonnegative");
        }
        let Some(biguint) = value.to_biguint() else {
            return raise_exception::<_>(_py, "ValueError", "isqrt() argument must be nonnegative");
        };
        let root = isqrt_biguint(&biguint);
        int_bits_from_bigint(_py, BigInt::from(root))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_nextafter(x_bits: u64, y_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(x_val) = coerce_real(_py, x_bits) else {
            return MoltObject::none().bits();
        };
        let Some(y_val) = coerce_real(_py, y_bits) else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_val) else {
            return MoltObject::none().bits();
        };
        let Some(y) = coerce_to_f64(_py, y_val) else {
            return MoltObject::none().bits();
        };
        if x.is_nan() || y.is_nan() {
            return float_result_bits(_py, f64::NAN);
        }
        if x == y {
            return float_result_bits(_py, y);
        }
        float_result_bits(_py, math_nextafter(x, y))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_ulp(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return float_result_bits(_py, f);
        }
        if f.is_infinite() {
            return float_result_bits(_py, f64::INFINITY);
        }
        let next = math_nextafter(f, f64::INFINITY);
        float_result_bits(_py, (next - f).abs())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_math_remainder(x_bits: u64, y_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(x_val) = coerce_real(_py, x_bits) else {
            return MoltObject::none().bits();
        };
        let Some(y_val) = coerce_real(_py, y_bits) else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_val) else {
            return MoltObject::none().bits();
        };
        let Some(y) = coerce_to_f64(_py, y_val) else {
            return MoltObject::none().bits();
        };
        if x.is_nan() || y.is_nan() {
            return float_result_bits(_py, f64::NAN);
        }
        if y == 0.0 || x.is_infinite() {
            return math_domain_error(_py);
        }
        float_result_bits(_py, math_remainder(x, y))
    })
}

fn statistics_mean_value(_py: &PyToken<'_>, data_bits: u64) -> Option<f64> {
    let values = collect_real_vec(_py, data_bits)?;
    if values.is_empty() {
        raise_exception::<()>(_py, "ValueError", "mean requires at least one data point");
        return None;
    }
    Some(sum_f64_simd(&values) / values.len() as f64)
}

fn statistics_stdev_value(_py: &PyToken<'_>, data_bits: u64, xbar_bits: u64) -> Option<f64> {
    let variance = statistics_variance_value(_py, data_bits, xbar_bits, false, "stdev")?;
    Some(math_sqrt(variance))
}

fn statistics_variance_value(
    _py: &PyToken<'_>,
    data_bits: u64,
    center_bits: u64,
    population: bool,
    opname: &str,
) -> Option<f64> {
    let values = collect_real_vec(_py, data_bits)?;
    let n = values.len();
    if (!population && n < 2) || (population && n < 1) {
        let msg = if population {
            format!("{opname} requires at least one data point")
        } else {
            format!("{opname} requires at least two data points")
        };
        raise_exception::<()>(_py, "ValueError", &msg);
        return None;
    }
    let mean = if obj_from_bits(center_bits).is_none() {
        sum_f64_simd(&values) / n as f64
    } else {
        let value = coerce_real_named(_py, center_bits, opname)?;
        coerce_to_f64(_py, value)?
    };
    let sum_sq = sum_sq_diff_f64_simd(&values, mean);
    let denominator = if population { n as f64 } else { (n - 1) as f64 };
    let variance = if sum_sq < 0.0 && sum_sq > -f64::EPSILON {
        0.0
    } else {
        sum_sq / denominator
    };
    Some(variance)
}

fn collect_values_vec(_py: &PyToken<'_>, iter_bits: u64) -> Option<Vec<u64>> {
    let iter_obj = molt_iter(iter_bits);
    if obj_from_bits(iter_obj).is_none() {
        raise_not_iterable::<Option<Vec<u64>>>(_py, iter_bits);
        return None;
    }
    let mut out = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_obj);
        if exception_pending(_py) {
            return None;
        }
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return raise_exception::<Option<Vec<u64>>>(
                _py,
                "TypeError",
                "object is not an iterator",
            );
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<Option<Vec<u64>>>(
                    _py,
                    "TypeError",
                    "object is not an iterator",
                );
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return raise_exception::<Option<Vec<u64>>>(
                    _py,
                    "TypeError",
                    "object is not an iterator",
                );
            }
            let val_bits = elems[0];
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            out.push(val_bits);
        }
    }
    Some(out)
}

fn statistics_sorted_values(_py: &PyToken<'_>, data_bits: u64) -> Option<u64> {
    let none_bits = MoltObject::none().bits();
    let false_bits = MoltObject::from_bool(false).bits();
    let sorted_bits = molt_sorted_builtin(data_bits, none_bits, false_bits);
    if exception_pending(_py) || obj_from_bits(sorted_bits).is_none() {
        return None;
    }
    Some(sorted_bits)
}

fn statistics_collect_sorted_real(
    _py: &PyToken<'_>,
    data_bits: u64,
    opname: &str,
) -> Option<Vec<f64>> {
    let mut values = collect_real_vec(_py, data_bits)?;
    if values.is_empty() {
        let msg = format!("{opname} requires at least one data point");
        raise_exception::<()>(_py, "ValueError", &msg);
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(values)
}

fn statistics_mode_value(_py: &PyToken<'_>, data_bits: u64) -> Option<u64> {
    let values = collect_values_vec(_py, data_bits)?;
    if values.is_empty() {
        raise_exception::<()>(_py, "ValueError", "no mode for empty data");
        return None;
    }
    let counts_bits = crate::molt_dict_new(0);
    let counts_ptr = obj_from_bits(counts_bits).as_ptr()?;
    let mut best_bits = values[0];
    let mut best_count: i64 = 0;
    unsafe {
        for &value_bits in &values {
            let current = match dict_get_in_place(_py, counts_ptr, value_bits) {
                Some(bits) => to_i64(obj_from_bits(bits)).unwrap_or(0),
                None => {
                    if exception_pending(_py) {
                        if maybe_ptr_from_bits(counts_bits).is_some() {
                            dec_ref_bits(_py, counts_bits);
                        }
                        return None;
                    }
                    0
                }
            };
            let Some(next) = current.checked_add(1) else {
                if maybe_ptr_from_bits(counts_bits).is_some() {
                    dec_ref_bits(_py, counts_bits);
                }
                return raise_exception::<Option<u64>>(_py, "OverflowError", "mode count overflow");
            };
            dict_set_in_place(
                _py,
                counts_ptr,
                value_bits,
                MoltObject::from_int(next).bits(),
            );
            if exception_pending(_py) {
                if maybe_ptr_from_bits(counts_bits).is_some() {
                    dec_ref_bits(_py, counts_bits);
                }
                return None;
            }
            if next > best_count {
                best_bits = value_bits;
                best_count = next;
            }
        }
    }
    inc_ref_bits(_py, best_bits);
    if maybe_ptr_from_bits(counts_bits).is_some() {
        dec_ref_bits(_py, counts_bits);
    }
    Some(best_bits)
}

fn statistics_multimode_value(_py: &PyToken<'_>, data_bits: u64) -> Option<u64> {
    let values = collect_values_vec(_py, data_bits)?;
    if values.is_empty() {
        let list_ptr = alloc_list(_py, &[]);
        if list_ptr.is_null() {
            return None;
        }
        return Some(MoltObject::from_ptr(list_ptr).bits());
    }
    let counts_bits = crate::molt_dict_new(0);
    let counts_ptr = obj_from_bits(counts_bits).as_ptr()?;
    let mut first_seen: Vec<u64> = Vec::new();
    let mut max_count: i64 = 0;
    unsafe {
        for &value_bits in &values {
            let current_opt = dict_get_in_place(_py, counts_ptr, value_bits);
            let current = match current_opt {
                Some(bits) => to_i64(obj_from_bits(bits)).unwrap_or(0),
                None => {
                    if exception_pending(_py) {
                        if maybe_ptr_from_bits(counts_bits).is_some() {
                            dec_ref_bits(_py, counts_bits);
                        }
                        return None;
                    }
                    first_seen.push(value_bits);
                    0
                }
            };
            let Some(next) = current.checked_add(1) else {
                if maybe_ptr_from_bits(counts_bits).is_some() {
                    dec_ref_bits(_py, counts_bits);
                }
                return raise_exception::<Option<u64>>(
                    _py,
                    "OverflowError",
                    "multimode count overflow",
                );
            };
            dict_set_in_place(
                _py,
                counts_ptr,
                value_bits,
                MoltObject::from_int(next).bits(),
            );
            if exception_pending(_py) {
                if maybe_ptr_from_bits(counts_bits).is_some() {
                    dec_ref_bits(_py, counts_bits);
                }
                return None;
            }
            if next > max_count {
                max_count = next;
            }
        }
        let mut out: Vec<u64> = Vec::new();
        for &value_bits in &first_seen {
            let Some(count_bits) = dict_get_in_place(_py, counts_ptr, value_bits) else {
                if exception_pending(_py) {
                    if maybe_ptr_from_bits(counts_bits).is_some() {
                        dec_ref_bits(_py, counts_bits);
                    }
                    return None;
                }
                continue;
            };
            if to_i64(obj_from_bits(count_bits)).unwrap_or(0) == max_count {
                out.push(value_bits);
            }
        }
        let list_ptr = alloc_list(_py, &out);
        if maybe_ptr_from_bits(counts_bits).is_some() {
            dec_ref_bits(_py, counts_bits);
        }
        if list_ptr.is_null() {
            return None;
        }
        Some(MoltObject::from_ptr(list_ptr).bits())
    }
}

fn statistics_quantiles_value(
    _py: &PyToken<'_>,
    data_bits: u64,
    n_bits: u64,
    inclusive_bits: u64,
) -> Option<u64> {
    let n = index_i64_from_obj(_py, n_bits, "quantiles");
    if exception_pending(_py) {
        return None;
    }
    if n < 1 {
        raise_exception::<()>(_py, "ValueError", "n must be at least 1");
        return None;
    }
    let mut values = collect_real_vec(_py, data_bits)?;
    if values.len() < 2 {
        raise_exception::<()>(_py, "ValueError", "must have at least two data points");
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n_usize = usize::try_from(n).ok()?;
    let inclusive = is_truthy(_py, obj_from_bits(inclusive_bits));
    let mut out_floats: Vec<f64> = Vec::with_capacity(n_usize.saturating_sub(1));
    if !inclusive {
        let m = values.len() + 1;
        for i in 1..n_usize {
            let num = i * m;
            let j = num / n_usize;
            let delta = num % n_usize;
            if j == 0 {
                out_floats.push(values[0]);
                continue;
            }
            if j >= values.len() {
                out_floats.push(*values.last().unwrap_or(&values[0]));
                continue;
            }
            let lo = values[j - 1];
            let hi = values[j];
            out_floats.push(lo + (delta as f64 / n_usize as f64) * (hi - lo));
        }
    } else {
        let m = values.len() - 1;
        for i in 1..n_usize {
            let num = i * m;
            let j = num / n_usize;
            let delta = num % n_usize;
            let lo = values[j];
            let hi = values[j + 1];
            out_floats.push(lo + (delta as f64 / n_usize as f64) * (hi - lo));
        }
    }
    let out_bits: Vec<u64> = out_floats
        .into_iter()
        .map(|v| float_result_bits(_py, v))
        .collect();
    let list_ptr = alloc_list(_py, &out_bits);
    if list_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
}

fn statistics_harmonic_mean_value(_py: &PyToken<'_>, data_bits: u64) -> Option<f64> {
    let values = collect_real_vec(_py, data_bits)?;
    if values.is_empty() {
        raise_exception::<()>(
            _py,
            "ValueError",
            "harmonic_mean requires at least one data point",
        );
        return None;
    }
    if values.iter().any(|v| *v < 0.0) {
        raise_exception::<()>(
            _py,
            "ValueError",
            "harmonic mean does not support negative values",
        );
        return None;
    }
    if values.contains(&0.0) {
        return Some(0.0);
    }
    let denom = values.iter().fold(0.0_f64, |acc, v| acc + (1.0 / *v));
    Some(values.len() as f64 / denom)
}

fn statistics_geometric_mean_value(_py: &PyToken<'_>, data_bits: u64) -> Option<f64> {
    let values = collect_real_vec(_py, data_bits)?;
    if values.is_empty() {
        raise_exception::<()>(
            _py,
            "ValueError",
            "geometric_mean requires at least one data point",
        );
        return None;
    }
    if values.iter().any(|v| *v < 0.0) {
        raise_exception::<()>(
            _py,
            "ValueError",
            "geometric mean does not support negative values",
        );
        return None;
    }
    if values.contains(&0.0) {
        return Some(0.0);
    }
    let sum_logs = values.iter().fold(0.0_f64, |acc, v| acc + math_log(*v));
    Some(math_exp(sum_logs / values.len() as f64))
}

fn statistics_covariance_value(_py: &PyToken<'_>, x_bits: u64, y_bits: u64) -> Option<f64> {
    let x_values = collect_real_vec(_py, x_bits)?;
    let y_values = collect_real_vec(_py, y_bits)?;
    if x_values.len() != y_values.len() {
        raise_exception::<()>(
            _py,
            "ValueError",
            "covariance requires that both inputs have the same length",
        );
        return None;
    }
    if x_values.len() < 2 {
        raise_exception::<()>(
            _py,
            "ValueError",
            "covariance requires at least two data points",
        );
        return None;
    }
    let x_mean = sum_f64_simd(&x_values) / x_values.len() as f64;
    let y_mean = sum_f64_simd(&y_values) / y_values.len() as f64;
    let mut accum = 0.0_f64;
    for (xv, yv) in x_values.iter().zip(y_values.iter()) {
        accum += (*xv - x_mean) * (*yv - y_mean);
    }
    Some(accum / (x_values.len() - 1) as f64)
}

fn statistics_correlation_value(_py: &PyToken<'_>, x_bits: u64, y_bits: u64) -> Option<f64> {
    let x_values = collect_real_vec(_py, x_bits)?;
    let y_values = collect_real_vec(_py, y_bits)?;
    if x_values.len() != y_values.len() {
        raise_exception::<()>(
            _py,
            "ValueError",
            "correlation requires that both inputs have the same length",
        );
        return None;
    }
    if x_values.len() < 2 {
        raise_exception::<()>(
            _py,
            "ValueError",
            "correlation requires at least two data points",
        );
        return None;
    }
    let x_mean = sum_f64_simd(&x_values) / x_values.len() as f64;
    let y_mean = sum_f64_simd(&y_values) / y_values.len() as f64;
    let mut num = 0.0_f64;
    let mut x_var = 0.0_f64;
    let mut y_var = 0.0_f64;
    for (xv, yv) in x_values.iter().zip(y_values.iter()) {
        let dx = *xv - x_mean;
        let dy = *yv - y_mean;
        num += dx * dy;
        x_var += dx * dx;
        y_var += dy * dy;
    }
    let denom = math_sqrt(x_var * y_var);
    if denom == 0.0 {
        raise_exception::<()>(_py, "ValueError", "at least one of the inputs is constant");
        return None;
    }
    Some(num / denom)
}

fn statistics_linear_regression_value(
    _py: &PyToken<'_>,
    x_bits: u64,
    y_bits: u64,
    proportional_bits: u64,
) -> Option<(f64, f64)> {
    let x_values = collect_real_vec(_py, x_bits)?;
    let y_values = collect_real_vec(_py, y_bits)?;
    if x_values.len() != y_values.len() {
        raise_exception::<()>(
            _py,
            "ValueError",
            "x and y must have the same number of data points",
        );
        return None;
    }
    if x_values.len() < 2 {
        raise_exception::<()>(
            _py,
            "ValueError",
            "linear_regression requires at least two data points",
        );
        return None;
    }
    if is_truthy(_py, obj_from_bits(proportional_bits)) {
        let mut sxx = 0.0_f64;
        let mut sxy = 0.0_f64;
        for (xv, yv) in x_values.iter().zip(y_values.iter()) {
            sxx += *xv * *xv;
            sxy += *xv * *yv;
        }
        if sxx == 0.0 {
            raise_exception::<()>(_py, "ValueError", "x is constant");
            return None;
        }
        return Some((sxy / sxx, 0.0));
    }
    let x_mean = sum_f64_simd(&x_values) / x_values.len() as f64;
    let y_mean = sum_f64_simd(&y_values) / y_values.len() as f64;
    let mut sxx = 0.0_f64;
    let mut sxy = 0.0_f64;
    for (xv, yv) in x_values.iter().zip(y_values.iter()) {
        let dx = *xv - x_mean;
        sxx += dx * dx;
        sxy += dx * (*yv - y_mean);
    }
    if sxx == 0.0 {
        raise_exception::<()>(_py, "ValueError", "x is constant");
        return None;
    }
    let slope = sxy / sxx;
    let intercept = y_mean - slope * x_mean;
    Some((slope, intercept))
}

fn statistics_coerce_elem_fast_f64(_py: &PyToken<'_>, val_bits: u64, name: &str) -> Option<f64> {
    let val = obj_from_bits(val_bits);
    if let Some(i) = val.as_int() {
        return Some(i as f64);
    }
    if let Some(f) = val.as_float() {
        return Some(f);
    }
    let real = coerce_real_named(_py, val_bits, name)?;
    coerce_to_f64(_py, real)
}

const STATISTICS_RANDOM_N: usize = 624;
const STATISTICS_RANDOM_M: usize = 397;
const STATISTICS_RANDOM_MATRIX_A: u32 = 0x9908_B0DF;
const STATISTICS_RANDOM_UPPER_MASK: u32 = 0x8000_0000;
const STATISTICS_RANDOM_LOWER_MASK: u32 = 0x7FFF_FFFF;
const STATISTICS_RANDOM_RECIP_BPF: f64 = 1.0 / 9_007_199_254_740_992.0;
const STATISTICS_NORMAL_DIST_INV_CDF_MODE_MARKER: &[u8] = b"__statistics_inv_cdf_mode__";

#[derive(Clone, Copy, Eq, PartialEq)]
enum StatisticsNormalDistSamplesMode {
    Gauss,
    InvCdf,
}

#[derive(Clone)]
struct StatisticsRandomRng {
    mt: [u32; STATISTICS_RANDOM_N],
    index: usize,
    gauss_next: Option<f64>,
}

impl StatisticsRandomRng {
    fn from_seed_key(seed_key: &[u32]) -> Self {
        let mut out = Self {
            mt: [0; STATISTICS_RANDOM_N],
            index: STATISTICS_RANDOM_N,
            gauss_next: None,
        };
        out.init_by_array(seed_key);
        out.index = STATISTICS_RANDOM_N;
        out.gauss_next = None;
        out
    }

    fn init_genrand(&mut self, seed: u32) {
        self.mt[0] = seed;
        for i in 1..STATISTICS_RANDOM_N {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
    }

    fn init_by_array(&mut self, init_key: &[u32]) {
        self.init_genrand(19_650_218);
        let mut i = 1usize;
        let mut j = 0usize;
        let key_length = init_key.len();
        let k = STATISTICS_RANDOM_N.max(key_length);

        for _ in 0..k {
            let prev = self.mt[i - 1];
            let mixed = self.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_664_525));
            self.mt[i] = mixed.wrapping_add(init_key[j]).wrapping_add(j as u32);

            i += 1;
            j += 1;
            if i >= STATISTICS_RANDOM_N {
                self.mt[0] = self.mt[STATISTICS_RANDOM_N - 1];
                i = 1;
            }
            if j >= key_length {
                j = 0;
            }
        }

        for _ in 0..(STATISTICS_RANDOM_N - 1) {
            let prev = self.mt[i - 1];
            let mixed = self.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_566_083_941));
            self.mt[i] = mixed.wrapping_sub(i as u32);
            i += 1;
            if i >= STATISTICS_RANDOM_N {
                self.mt[0] = self.mt[STATISTICS_RANDOM_N - 1];
                i = 1;
            }
        }

        self.mt[0] = STATISTICS_RANDOM_UPPER_MASK;
    }

    fn twist(&mut self) {
        for i in 0..STATISTICS_RANDOM_N {
            let y = (self.mt[i] & STATISTICS_RANDOM_UPPER_MASK)
                | (self.mt[(i + 1) % STATISTICS_RANDOM_N] & STATISTICS_RANDOM_LOWER_MASK);
            let mut value = self.mt[(i + STATISTICS_RANDOM_M) % STATISTICS_RANDOM_N] ^ (y >> 1);
            if y & 1 != 0 {
                value ^= STATISTICS_RANDOM_MATRIX_A;
            }
            self.mt[i] = value;
        }
        self.index = 0;
    }

    fn rand_u32(&mut self) -> u32 {
        if self.index >= STATISTICS_RANDOM_N {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^= y >> 18;
        y
    }

    fn random(&mut self) -> f64 {
        let a = (self.rand_u32() >> 5) as u64;
        let b = (self.rand_u32() >> 6) as u64;
        (a as f64 * 67_108_864.0 + b as f64) * STATISTICS_RANDOM_RECIP_BPF
    }

    fn gauss(&mut self, mu: f64, sigma: f64) -> f64 {
        let z = if let Some(next) = self.gauss_next.take() {
            next
        } else {
            let x2pi = self.random() * core::f64::consts::TAU;
            let g2rad = math_sqrt(-2.0 * math_log(1.0 - self.random()));
            let z = math_cos(x2pi) * g2rad;
            self.gauss_next = Some(math_sin(x2pi) * g2rad);
            z
        };
        mu + z * sigma
    }
}

fn statistics_seed_type_error<T>(_py: &PyToken<'_>) -> Option<T> {
    raise_exception::<Option<T>>(
        _py,
        "TypeError",
        "The only supported seed types are:\nNone, int, float, str, bytes, and bytearray.",
    )
}

fn statistics_seed_bigint(_py: &PyToken<'_>, seed_bits: u64) -> Option<BigInt> {
    let seed_obj = obj_from_bits(seed_bits);
    if let Some(i) = to_i64(seed_obj) {
        return Some(BigInt::from(i).abs());
    }
    if let Some(ptr) = bigint_ptr_from_bits(seed_bits) {
        return Some(unsafe { bigint_ref(ptr).abs() });
    }
    if seed_obj.as_float().is_some() {
        let hash_bits = crate::molt_hash_builtin(seed_bits);
        if exception_pending(_py) {
            return None;
        }
        let hash_obj = obj_from_bits(hash_bits);
        let hash_u64 = if let Some(i) = to_i64(hash_obj) {
            i as u64
        } else if let Some(ptr) = bigint_ptr_from_bits(hash_bits) {
            let hash_big = unsafe { bigint_ref(ptr).clone() };
            let modulus = BigInt::one() << 64;
            hash_big.mod_floor(&modulus).to_u64().unwrap_or(0)
        } else {
            if maybe_ptr_from_bits(hash_bits).is_some() {
                dec_ref_bits(_py, hash_bits);
            }
            return raise_exception::<Option<BigInt>>(
                _py,
                "TypeError",
                "hash() should return an integer",
            );
        };
        if maybe_ptr_from_bits(hash_bits).is_some() {
            dec_ref_bits(_py, hash_bits);
        }
        return Some(BigInt::from(hash_u64));
    }

    let Some(seed_ptr) = seed_obj.as_ptr() else {
        return statistics_seed_type_error(_py);
    };
    let seed_bytes: Vec<u8> = unsafe {
        match object_type_id(seed_ptr) {
            TYPE_ID_STRING => {
                std::slice::from_raw_parts(string_bytes(seed_ptr), string_len(seed_ptr)).to_vec()
            }
            TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                let Some(slice) = bytes_like_slice(seed_ptr) else {
                    return statistics_seed_type_error(_py);
                };
                slice.to_vec()
            }
            _ => return statistics_seed_type_error(_py),
        }
    };
    #[cfg(feature = "stdlib_crypto")]
    {
        let digest = Sha512::digest(&seed_bytes);
        let mut payload = Vec::with_capacity(seed_bytes.len() + digest.len());
        payload.extend_from_slice(&seed_bytes);
        payload.extend_from_slice(&digest);
        Some(BigInt::from(BigUint::from_bytes_be(&payload)))
    }
    #[cfg(not(feature = "stdlib_crypto"))]
    {
        // Without crypto support, fall back to using the raw seed bytes.
        Some(BigInt::from(BigUint::from_bytes_be(&seed_bytes)))
    }
}

fn statistics_seed_key(seed: &BigInt) -> Vec<u32> {
    let mut key = seed
        .abs()
        .to_biguint()
        .map_or_else(Vec::new, |v| v.to_u32_digits());
    if key.is_empty() {
        key.push(0);
    }
    key
}

fn statistics_normal_dist_sample_count(_py: &PyToken<'_>, n_bits: u64) -> Option<usize> {
    let n_type = type_name(_py, obj_from_bits(n_bits));
    let err = format!("'{n_type}' object cannot be interpreted as an integer");
    let n_big = index_bigint_from_obj(_py, n_bits, &err)?;
    if n_big.is_negative() {
        return Some(0);
    }
    if let Some(n) = n_big.to_usize() {
        return Some(n);
    }
    raise_exception::<Option<usize>>(
        _py,
        "OverflowError",
        "Python int too large to convert to C ssize_t",
    )
}

fn statistics_normal_dist_samples_mode(
    _py: &PyToken<'_>,
    seed_bits: u64,
) -> Option<(StatisticsNormalDistSamplesMode, u64)> {
    let seed_obj = obj_from_bits(seed_bits);
    let Some(seed_ptr) = seed_obj.as_ptr() else {
        return Some((StatisticsNormalDistSamplesMode::Gauss, seed_bits));
    };
    let pair = unsafe {
        if object_type_id(seed_ptr) != TYPE_ID_TUPLE {
            None
        } else {
            let elems = seq_vec_ref(seed_ptr);
            if elems.len() == 2 {
                Some((elems[0], elems[1]))
            } else {
                None
            }
        }
    };
    let Some((marker_bits, inner_seed_bits)) = pair else {
        return Some((StatisticsNormalDistSamplesMode::Gauss, seed_bits));
    };
    let marker_obj = obj_from_bits(marker_bits);
    let Some(marker_ptr) = marker_obj.as_ptr() else {
        return Some((StatisticsNormalDistSamplesMode::Gauss, seed_bits));
    };
    let is_inv_cdf_mode = unsafe {
        if object_type_id(marker_ptr) != TYPE_ID_STRING {
            false
        } else {
            let marker_bytes =
                std::slice::from_raw_parts(string_bytes(marker_ptr), string_len(marker_ptr));
            marker_bytes == STATISTICS_NORMAL_DIST_INV_CDF_MODE_MARKER
        }
    };
    if !is_inv_cdf_mode {
        return Some((StatisticsNormalDistSamplesMode::Gauss, seed_bits));
    }
    Some((StatisticsNormalDistSamplesMode::InvCdf, inner_seed_bits))
}

fn statistics_normal_dist_samples_value(
    _py: &PyToken<'_>,
    mu_bits: u64,
    sigma_bits: u64,
    n_bits: u64,
    seed_bits: u64,
    random_fn_bits: u64,
) -> Option<u64> {
    let (mu, sigma) = statistics_normal_dist_params(_py, mu_bits, sigma_bits)?;
    let (mode, effective_seed_bits) = statistics_normal_dist_samples_mode(_py, seed_bits)?;
    let count = statistics_normal_dist_sample_count(_py, n_bits)?;
    let mut out_bits: Vec<u64> = Vec::with_capacity(count);
    let gauss_mu_bits = float_result_bits(_py, mu);
    let gauss_sigma_bits = float_result_bits(_py, sigma);

    let mut seeded_rng = if obj_from_bits(effective_seed_bits).is_none() {
        None
    } else {
        let seed_big = statistics_seed_bigint(_py, effective_seed_bits)?;
        Some(StatisticsRandomRng::from_seed_key(&statistics_seed_key(
            &seed_big,
        )))
    };

    for _ in 0..count {
        let sample = match mode {
            StatisticsNormalDistSamplesMode::Gauss => {
                if let Some(rng) = seeded_rng.as_mut() {
                    rng.gauss(mu, sigma)
                } else {
                    let sample_bits = unsafe {
                        call_callable2(_py, random_fn_bits, gauss_mu_bits, gauss_sigma_bits)
                    };
                    if exception_pending(_py) {
                        return None;
                    }
                    let sample_obj = obj_from_bits(sample_bits);
                    let Some(sample_real) = coerce_real_named(_py, sample_bits, "sample") else {
                        if sample_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, sample_bits);
                        }
                        return None;
                    };
                    let Some(sample_float) = coerce_to_f64(_py, sample_real) else {
                        if sample_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, sample_bits);
                        }
                        return None;
                    };
                    if sample_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, sample_bits);
                    }
                    sample_float
                }
            }
            StatisticsNormalDistSamplesMode::InvCdf => {
                let probability = if let Some(rng) = seeded_rng.as_mut() {
                    rng.random()
                } else {
                    let p_bits = unsafe { call_callable0(_py, random_fn_bits) };
                    if exception_pending(_py) {
                        return None;
                    }
                    let p_obj = obj_from_bits(p_bits);
                    let Some(p_real) = coerce_real_named(_py, p_bits, "p") else {
                        if p_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, p_bits);
                        }
                        return None;
                    };
                    let Some(p_float) = coerce_to_f64(_py, p_real) else {
                        if p_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, p_bits);
                        }
                        return None;
                    };
                    if p_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, p_bits);
                    }
                    p_float
                };
                if probability <= 0.0 || probability >= 1.0 {
                    return raise_exception::<Option<u64>>(
                        _py,
                        "ValueError",
                        "inv_cdf undefined for these parameters",
                    );
                }
                statistics_normal_dist_inv_cdf_raw(probability, mu, sigma)
            }
        };
        out_bits.push(float_result_bits(_py, sample));
    }

    let list_ptr = alloc_list(_py, &out_bits);
    if list_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
}

fn statistics_normal_dist_params(
    _py: &PyToken<'_>,
    mu_bits: u64,
    sigma_bits: u64,
) -> Option<(f64, f64)> {
    let mu_real = coerce_real_named(_py, mu_bits, "mu")?;
    let mu = coerce_to_f64(_py, mu_real)?;
    let sigma_real = coerce_real_named(_py, sigma_bits, "sigma")?;
    let sigma = coerce_to_f64(_py, sigma_real)?;
    if sigma < 0.0 {
        return raise_exception::<Option<(f64, f64)>>(
            _py,
            "ValueError",
            "sigma must be non-negative",
        );
    }
    Some((mu, sigma))
}

fn horner_eval(x: f64, coeffs: &[f64]) -> f64 {
    let mut acc = 0.0;
    for &coeff in coeffs {
        acc = acc * x + coeff;
    }
    acc
}

fn statistics_normal_dist_inv_cdf_raw(p: f64, mu: f64, sigma: f64) -> f64 {
    const A: [f64; 8] = [
        2.5090809287301227e3,
        3.343_057_558_358_813e4,
        6.726_577_092_700_87e4,
        4.592_195_393_154_987e4,
        1.373_169_376_550_946e4,
        1.9715909503065514e3,
        1.3314166789178438e2,
        3.3871328727963666,
    ];
    const B: [f64; 8] = [
        5.226_495_278_852_854e3,
        2.8729085735721943e4,
        3.930_789_580_009_271e4,
        2.1213794301586596e4,
        5.394_196_021_424_751e3,
        6.871_870_074_920_579e2,
        4.231_333_070_160_091e1,
        1.0,
    ];
    const C: [f64; 8] = [
        7.745_450_142_783_414e-4,
        2.2723844989269185e-2,
        2.417_807_251_774_506e-1,
        1.2704582524523684,
        3.6478483247632046,
        5.769_497_221_460_691,
        4.630_337_846_156_546,
        1.4234371107496836,
    ];
    const D: [f64; 8] = [
        1.0507500716444168e-9,
        5.475_938_084_995_344e-4,
        1.5198666563616457e-2,
        1.4810397642748008e-1,
        6.897_673_349_851e-1,
        1.6763848301838038,
        2.053_191_626_637_759,
        1.0,
    ];
    const E: [f64; 8] = [
        2.0103343992922881e-7,
        2.7115555687434876e-5,
        1.2426609473880784e-3,
        2.6532189526576123e-2,
        2.9656057182850489e-1,
        1.7848265399172913,
        5.463_784_911_164_114,
        6.657_904_643_501_103,
    ];
    const F: [f64; 8] = [
        2.0442631033899397e-15,
        1.421_511_758_316_446e-7,
        1.8463183175100547e-5,
        7.868_691_311_456_133e-4,
        1.4875361290850615e-2,
        1.369_298_809_227_358e-1,
        5.998_322_065_558_88e-1,
        1.0,
    ];

    let q = p - 0.5;
    if q.abs() <= 0.425 {
        let r = 0.180625 - q * q;
        let x = (horner_eval(r, &A) * q) / horner_eval(r, &B);
        return mu + (x * sigma);
    }

    let mut r = if q <= 0.0 { p } else { 1.0 - p };
    r = math_sqrt(-math_log(r));
    let x = if r <= 5.0 {
        let rr = r - 1.6;
        horner_eval(rr, &C) / horner_eval(rr, &D)
    } else {
        let rr = r - 5.0;
        horner_eval(rr, &E) / horner_eval(rr, &F)
    };
    let x = if q < 0.0 { -x } else { x };
    mu + (x * sigma)
}

fn statistics_normal_dist_cdf_raw(x: f64, mu: f64, sigma: f64) -> f64 {
    0.5 * (1.0 + math_erf((x - mu) / (sigma * core::f64::consts::SQRT_2)))
}

fn materialize_statistics_slice(
    _py: &PyToken<'_>,
    data_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> Option<u64> {
    let none_bits = MoltObject::none().bits();
    let start_obj = if is_truthy(_py, obj_from_bits(has_start_bits)) {
        start_bits
    } else {
        none_bits
    };
    let end_obj = if is_truthy(_py, obj_from_bits(has_end_bits)) {
        end_bits
    } else {
        none_bits
    };
    let slice_bits = crate::molt_slice_new(start_obj, end_obj, none_bits);
    if exception_pending(_py) {
        return None;
    }
    let sliced_bits = crate::molt_index(data_bits, slice_bits);
    if maybe_ptr_from_bits(slice_bits).is_some() {
        dec_ref_bits(_py, slice_bits);
    }
    if exception_pending(_py) {
        return None;
    }
    Some(sliced_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_mean(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(mean) = statistics_mean_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, mean)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_stdev(data_bits: u64, xbar_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(stdev) = statistics_stdev_value(_py, data_bits, xbar_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, stdev)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_variance(data_bits: u64, xbar_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(variance) =
            statistics_variance_value(_py, data_bits, xbar_bits, false, "variance")
        else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, variance)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_pvariance(data_bits: u64, mu_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(variance) = statistics_variance_value(_py, data_bits, mu_bits, true, "pvariance")
        else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, variance)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_pstdev(data_bits: u64, mu_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(variance) = statistics_variance_value(_py, data_bits, mu_bits, true, "pstdev")
        else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, math_sqrt(variance))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_fmean(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(values) = collect_real_vec(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        if values.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "fmean requires at least one data point",
            );
        }
        float_result_bits(_py, sum_f64_simd(&values) / values.len() as f64)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_median(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(values) = statistics_collect_sorted_real(_py, data_bits, "median") else {
            return MoltObject::none().bits();
        };
        let n = values.len();
        let mid = n / 2;
        let out = if n % 2 == 1 {
            values[mid]
        } else {
            (values[mid - 1] + values[mid]) / 2.0
        };
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_median_low(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sorted_bits) = statistics_sorted_values(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        let sorted = obj_from_bits(sorted_bits);
        let Some(sorted_ptr) = sorted.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(sorted_ptr) != TYPE_ID_LIST {
                if maybe_ptr_from_bits(sorted_bits).is_some() {
                    dec_ref_bits(_py, sorted_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "median_low expected sorted list payload",
                );
            }
            let elems = seq_vec_ref(sorted_ptr);
            if elems.is_empty() {
                if maybe_ptr_from_bits(sorted_bits).is_some() {
                    dec_ref_bits(_py, sorted_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "median_low requires at least one data point",
                );
            }
            let idx = (elems.len() - 1) / 2;
            let out_bits = elems[idx];
            inc_ref_bits(_py, out_bits);
            if maybe_ptr_from_bits(sorted_bits).is_some() {
                dec_ref_bits(_py, sorted_bits);
            }
            out_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_median_high(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sorted_bits) = statistics_sorted_values(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        let sorted = obj_from_bits(sorted_bits);
        let Some(sorted_ptr) = sorted.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(sorted_ptr) != TYPE_ID_LIST {
                if maybe_ptr_from_bits(sorted_bits).is_some() {
                    dec_ref_bits(_py, sorted_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "median_high expected sorted list payload",
                );
            }
            let elems = seq_vec_ref(sorted_ptr);
            if elems.is_empty() {
                if maybe_ptr_from_bits(sorted_bits).is_some() {
                    dec_ref_bits(_py, sorted_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "median_high requires at least one data point",
                );
            }
            let idx = elems.len() / 2;
            let out_bits = elems[idx];
            inc_ref_bits(_py, out_bits);
            if maybe_ptr_from_bits(sorted_bits).is_some() {
                dec_ref_bits(_py, sorted_bits);
            }
            out_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_median_grouped(data_bits: u64, interval_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(values) = statistics_collect_sorted_real(_py, data_bits, "median_grouped") else {
            return MoltObject::none().bits();
        };
        let Some(interval_real) = coerce_real_named(_py, interval_bits, "median_grouped") else {
            return MoltObject::none().bits();
        };
        let Some(interval) = coerce_to_f64(_py, interval_real) else {
            return MoltObject::none().bits();
        };
        let n = values.len();
        let mid = n / 2;
        let x = if n % 2 == 1 {
            values[mid]
        } else {
            (values[mid - 1] + values[mid]) / 2.0
        };
        let lower = x - (interval / 2.0);
        let cf = values.iter().filter(|v| **v < x).count() as f64;
        let f = values
            .iter()
            .filter(|v| (**v - x).abs() <= f64::EPSILON)
            .count() as f64;
        if f == 0.0 {
            return raise_exception::<_>(_py, "ValueError", "no grouped median for empty class");
        }
        float_result_bits(_py, lower + interval * ((n as f64 / 2.0 - cf) / f))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_mode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(bits) = statistics_mode_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_multimode(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(bits) = statistics_multimode_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_quantiles(
    data_bits: u64,
    n_bits: u64,
    inclusive_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(bits) = statistics_quantiles_value(_py, data_bits, n_bits, inclusive_bits) else {
            return MoltObject::none().bits();
        };
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_harmonic_mean(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = statistics_harmonic_mean_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_geometric_mean(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = statistics_geometric_mean_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_covariance(x_bits: u64, y_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = statistics_covariance_value(_py, x_bits, y_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_correlation(x_bits: u64, y_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = statistics_correlation_value(_py, x_bits, y_bits) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, value)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_linear_regression(
    x_bits: u64,
    y_bits: u64,
    proportional_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((slope, intercept)) =
            statistics_linear_regression_value(_py, x_bits, y_bits, proportional_bits)
        else {
            return MoltObject::none().bits();
        };
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                float_result_bits(_py, slope),
                float_result_bits(_py, intercept),
            ],
        );
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_new(mu_bits: u64, sigma_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        let tuple_ptr = alloc_tuple(
            _py,
            &[float_result_bits(_py, mu), float_result_bits(_py, sigma)],
        );
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_samples(
    mu_bits: u64,
    sigma_bits: u64,
    n_bits: u64,
    seed_bits: u64,
    random_fn_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = statistics_normal_dist_samples_value(
            _py,
            mu_bits,
            sigma_bits,
            n_bits,
            seed_bits,
            random_fn_bits,
        ) else {
            return MoltObject::none().bits();
        };
        value
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_inv_cdf(
    p_bits: u64,
    mu_bits: u64,
    sigma_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        let Some(p_real) = coerce_real_named(_py, p_bits, "p") else {
            return MoltObject::none().bits();
        };
        let Some(p) = coerce_to_f64(_py, p_real) else {
            return MoltObject::none().bits();
        };
        if p <= 0.0 || p >= 1.0 {
            return raise_exception::<_>(_py, "ValueError", "p must be in the range 0.0 < p < 1.0");
        }
        float_result_bits(_py, statistics_normal_dist_inv_cdf_raw(p, mu, sigma))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_pdf(
    mu_bits: u64,
    sigma_bits: u64,
    x_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        let variance = sigma * sigma;
        if variance == 0.0 {
            return raise_exception::<_>(_py, "ValueError", "pdf() not defined when sigma is zero");
        }
        let Some(x_real) = coerce_real_named(_py, x_bits, "x") else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_real) else {
            return MoltObject::none().bits();
        };
        let diff = x - mu;
        let out = math_exp(diff * diff / (-2.0 * variance))
            / math_sqrt(core::f64::consts::TAU * variance);
        float_result_bits(_py, out)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_cdf(
    mu_bits: u64,
    sigma_bits: u64,
    x_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        if sigma == 0.0 {
            return raise_exception::<_>(_py, "ValueError", "cdf() not defined when sigma is zero");
        }
        let Some(x_real) = coerce_real_named(_py, x_bits, "x") else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_real) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, statistics_normal_dist_cdf_raw(x, mu, sigma))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_zscore(
    mu_bits: u64,
    sigma_bits: u64,
    x_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((mu, sigma)) = statistics_normal_dist_params(_py, mu_bits, sigma_bits) else {
            return MoltObject::none().bits();
        };
        if sigma == 0.0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "zscore() not defined when sigma is zero",
            );
        }
        let Some(x_real) = coerce_real_named(_py, x_bits, "x") else {
            return MoltObject::none().bits();
        };
        let Some(x) = coerce_to_f64(_py, x_real) else {
            return MoltObject::none().bits();
        };
        float_result_bits(_py, (x - mu) / sigma)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_normal_dist_overlap(
    mu_a_bits: u64,
    sigma_a_bits: u64,
    mu_b_bits: u64,
    sigma_b_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some((mut mu_x, mut sigma_x)) =
            statistics_normal_dist_params(_py, mu_a_bits, sigma_a_bits)
        else {
            return MoltObject::none().bits();
        };
        let Some((mut mu_y, mut sigma_y)) =
            statistics_normal_dist_params(_py, mu_b_bits, sigma_b_bits)
        else {
            return MoltObject::none().bits();
        };
        if (sigma_y, mu_y) < (sigma_x, mu_x) {
            core::mem::swap(&mut mu_x, &mut mu_y);
            core::mem::swap(&mut sigma_x, &mut sigma_y);
        }
        let x_var = sigma_x * sigma_x;
        let y_var = sigma_y * sigma_y;
        if x_var == 0.0 || y_var == 0.0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "overlap() not defined when sigma is zero",
            );
        }
        let dv = y_var - x_var;
        let dm = (mu_y - mu_x).abs();
        if dv == 0.0 {
            let out = 1.0 - math_erf(dm / (2.0 * sigma_x * core::f64::consts::SQRT_2));
            return float_result_bits(_py, out);
        }
        let a = mu_x * y_var - mu_y * x_var;
        let inner = dm * dm + dv * math_log(y_var / x_var);
        if inner < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "overlap() domain error");
        }
        let b = sigma_x * sigma_y * math_sqrt(inner);
        let x1 = (a + b) / dv;
        let x2 = (a - b) / dv;
        let delta1 = (statistics_normal_dist_cdf_raw(x1, mu_y, sigma_y)
            - statistics_normal_dist_cdf_raw(x1, mu_x, sigma_x))
        .abs();
        let delta2 = (statistics_normal_dist_cdf_raw(x2, mu_y, sigma_y)
            - statistics_normal_dist_cdf_raw(x2, mu_x, sigma_x))
        .abs();
        float_result_bits(_py, 1.0 - (delta1 + delta2))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_mean_slice(
    data_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = obj_from_bits(data_bits);
        if let Some(data_ptr) = data.as_ptr() {
            unsafe {
                let ty = object_type_id(data_ptr);
                if ty == TYPE_ID_LIST || ty == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(data_ptr);
                    let Some((start, end)) = normalize_slice_step1_bounds(
                        _py,
                        elems.len(),
                        start_bits,
                        end_bits,
                        has_start_bits,
                        has_end_bits,
                    ) else {
                        return MoltObject::none().bits();
                    };
                    if start >= end {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "mean requires at least one data point",
                        );
                    }
                    let mut sum = 0.0_f64;
                    let mut compensation = 0.0_f64;
                    let mut count: usize = 0;
                    for &val_bits in &elems[start..end] {
                        let Some(f) = statistics_coerce_elem_fast_f64(_py, val_bits, "mean") else {
                            return MoltObject::none().bits();
                        };
                        let y = f - compensation;
                        let t = sum + y;
                        compensation = (t - sum) - y;
                        sum = t;
                        count += 1;
                    }
                    return float_result_bits(_py, sum / count as f64);
                }
            }
        }
        let Some(sliced_bits) = materialize_statistics_slice(
            _py,
            data_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        ) else {
            return MoltObject::none().bits();
        };
        let out = match statistics_mean_value(_py, sliced_bits) {
            Some(mean) => float_result_bits(_py, mean),
            None => MoltObject::none().bits(),
        };
        if maybe_ptr_from_bits(sliced_bits).is_some() {
            dec_ref_bits(_py, sliced_bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_statistics_stdev_slice(
    data_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let data = obj_from_bits(data_bits);
        if let Some(data_ptr) = data.as_ptr() {
            unsafe {
                let ty = object_type_id(data_ptr);
                if ty == TYPE_ID_LIST || ty == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(data_ptr);
                    let Some((start, end)) = normalize_slice_step1_bounds(
                        _py,
                        elems.len(),
                        start_bits,
                        end_bits,
                        has_start_bits,
                        has_end_bits,
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let n = end.saturating_sub(start);
                    if n < 2 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "stdev requires at least two data points",
                        );
                    }
                    let mut count = 0.0_f64;
                    let mut mean = 0.0_f64;
                    let mut m2 = 0.0_f64;
                    for &val_bits in &elems[start..end] {
                        let Some(x) = statistics_coerce_elem_fast_f64(_py, val_bits, "stdev")
                        else {
                            return MoltObject::none().bits();
                        };
                        count += 1.0;
                        let delta = x - mean;
                        mean += delta / count;
                        let delta2 = x - mean;
                        m2 += delta * delta2;
                    }
                    let variance = if m2 < 0.0 && m2 > -f64::EPSILON {
                        0.0
                    } else {
                        m2 / (count - 1.0)
                    };
                    return float_result_bits(_py, math_sqrt(variance));
                }
            }
        }
        let Some(sliced_bits) = materialize_statistics_slice(
            _py,
            data_bits,
            start_bits,
            end_bits,
            has_start_bits,
            has_end_bits,
        ) else {
            return MoltObject::none().bits();
        };
        let none_bits = MoltObject::none().bits();
        let out = match statistics_stdev_value(_py, sliced_bits, none_bits) {
            Some(stdev) => float_result_bits(_py, stdev),
            None => MoltObject::none().bits(),
        };
        if maybe_ptr_from_bits(sliced_bits).is_some() {
            dec_ref_bits(_py, sliced_bits);
        }
        out
    })
}
