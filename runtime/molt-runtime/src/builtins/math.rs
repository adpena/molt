use crate::builtins::callable::molt_is_callable;
use crate::builtins::numbers::{index_bigint_from_obj, index_i64_from_obj, int_bits_from_bigint};
use crate::object::ops::{format_obj, type_name};
use crate::PyToken;
use crate::{
    alloc_tuple, attr_lookup_ptr_allow_missing, bigint_bits, bigint_from_f64_trunc,
    bigint_ptr_from_bits, bigint_ref, bigint_to_inline, call_callable0, class_name_for_error,
    dec_ref_bits, exception_pending, inc_ref_bits, intern_static_name, is_truthy,
    maybe_ptr_from_bits, molt_iter, molt_iter_next, molt_mul, obj_from_bits, object_type_id,
    raise_exception, raise_not_iterable, runtime_state, seq_vec_ref, to_i64, type_of_bits,
    MoltObject, TYPE_ID_LIST, TYPE_ID_TUPLE,
};
use num_bigint::{BigInt, BigUint};
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};

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
    if let Some(f) = obj.as_float() {
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
                if let Some(f) = res_obj.as_float() {
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
    if let Some(f) = obj.as_float() {
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
                if let Some(f) = res_obj.as_float() {
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

#[no_mangle]
pub extern "C" fn molt_math_log(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        match value {
            RealValue::Float(f) => {
                if f.is_nan() {
                    return MoltObject::from_float(f).bits();
                }
                if f.is_infinite() {
                    if f.is_sign_negative() {
                        return log_domain_error(_py, Some(f));
                    }
                    return MoltObject::from_float(f).bits();
                }
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                MoltObject::from_float(math_log(f)).bits()
            }
            RealValue::IntExact(i) => {
                if i <= 0 {
                    return log_domain_error(_py, None);
                }
                MoltObject::from_float(math_log(i as f64)).bits()
            }
            RealValue::BigIntExact(big) => {
                if big.is_negative() || big.is_zero() {
                    return log_domain_error(_py, None);
                }
                MoltObject::from_float(log_bigint(&big)).bits()
            }
            RealValue::IntCoerced(i) => {
                let f = i as f64;
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                MoltObject::from_float(math_log(f)).bits()
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
                MoltObject::from_float(math_log(f)).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_math_log2(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        match value {
            RealValue::Float(f) => {
                if f.is_nan() {
                    return MoltObject::from_float(f).bits();
                }
                if f.is_infinite() {
                    if f.is_sign_negative() {
                        return log_domain_error(_py, Some(f));
                    }
                    return MoltObject::from_float(f).bits();
                }
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                MoltObject::from_float(math_log2(f)).bits()
            }
            RealValue::IntExact(i) => {
                if i <= 0 {
                    return log_domain_error(_py, None);
                }
                MoltObject::from_float(math_log2(i as f64)).bits()
            }
            RealValue::BigIntExact(big) => {
                if big.is_negative() || big.is_zero() {
                    return log_domain_error(_py, None);
                }
                MoltObject::from_float(log2_bigint(&big)).bits()
            }
            RealValue::IntCoerced(i) => {
                let f = i as f64;
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                MoltObject::from_float(math_log2(f)).bits()
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
                MoltObject::from_float(math_log2(f)).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_math_log10(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        match value {
            RealValue::Float(f) => {
                if f.is_nan() {
                    return MoltObject::from_float(f).bits();
                }
                if f.is_infinite() {
                    if f.is_sign_negative() {
                        return log_domain_error(_py, Some(f));
                    }
                    return MoltObject::from_float(f).bits();
                }
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                MoltObject::from_float(math_log10(f)).bits()
            }
            RealValue::IntExact(i) => {
                if i <= 0 {
                    return log_domain_error(_py, None);
                }
                MoltObject::from_float(math_log10(i as f64)).bits()
            }
            RealValue::BigIntExact(big) => {
                if big.is_negative() || big.is_zero() {
                    return log_domain_error(_py, None);
                }
                MoltObject::from_float(log_bigint(&big) / std::f64::consts::LN_10).bits()
            }
            RealValue::IntCoerced(i) => {
                let f = i as f64;
                if f <= 0.0 {
                    return log_domain_error(_py, Some(f));
                }
                MoltObject::from_float(math_log10(f)).bits()
            }
            RealValue::BigIntCoerced(big) => {
                if big.is_negative() || big.is_zero() {
                    return log_domain_error(_py, None);
                }
                MoltObject::from_float(log_bigint(&big) / std::f64::consts::LN_10).bits()
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_math_log1p(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                return log1p_domain_error(_py, f);
            }
            return MoltObject::from_float(f).bits();
        }
        if f <= -1.0 {
            return log1p_domain_error(_py, f);
        }
        MoltObject::from_float(math_log1p(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_exp(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                return MoltObject::from_float(0.0).bits();
            }
            return MoltObject::from_float(f).bits();
        }
        let out = math_exp(f);
        if out.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        MoltObject::from_float(out).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_expm1(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                return MoltObject::from_float(-1.0).bits();
            }
            return MoltObject::from_float(f).bits();
        }
        let out = math_expm1(f);
        if out.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        MoltObject::from_float(out).bits()
    })
}

#[no_mangle]
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
        MoltObject::from_float(math_fma(x, y, z)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_sin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            let rendered = render_float(_py, f);
            let msg = format!("expected a finite input, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_float(math_sin(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_cos(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            let rendered = render_float(_py, f);
            let msg = format!("expected a finite input, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_float(math_cos(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_acos(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if !(-1.0..=1.0).contains(&f) {
            let rendered = render_float(_py, f);
            let msg = format!("expected a number in range from -1 up to 1, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_float(math_acos(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_tan(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            let rendered = render_float(_py, f);
            let msg = format!("expected a finite input, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_float(math_tan(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_asin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if !(-1.0..=1.0).contains(&f) {
            let rendered = render_float(_py, f);
            let msg = format!("expected a number in range from -1 up to 1, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_float(math_asin(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_atan(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(math_atan(f)).bits()
    })
}

#[no_mangle]
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
        MoltObject::from_float(math_atan2(y, x)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_sinh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        let out = math_sinh(f);
        if out.is_infinite() && !f.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        MoltObject::from_float(out).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_cosh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        let out = math_cosh(f);
        if out.is_infinite() && !f.is_infinite() {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        MoltObject::from_float(out).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_tanh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(math_tanh(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_asinh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(math_asinh(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_acosh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f < 1.0 {
            let rendered = render_float(_py, f);
            let msg = format!("expected a number greater than or equal to 1, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_float(math_acosh(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_atanh(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f <= -1.0 || f >= 1.0 {
            return math_domain_error(_py);
        }
        MoltObject::from_float(math_atanh(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_gamma(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                let rendered = render_float(_py, f);
                let msg = format!("expected a noninteger or positive integer, got {rendered}");
                return raise_exception::<_>(_py, "ValueError", &msg);
            }
            return MoltObject::from_float(f).bits();
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
        MoltObject::from_float(out).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_erf(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(math_erf(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_erfc(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(math_erfc(f)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_lgamma(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            return MoltObject::from_float(f.abs()).bits();
        }
        if f <= 0.0 && f.fract() == 0.0 {
            let rendered = render_float(_py, f);
            let msg = format!("expected a noninteger or positive integer, got {rendered}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        MoltObject::from_float(math_lgamma(f)).bits()
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_math_isnan(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "isnan") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_bool(f.is_nan()).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_fabs(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "fabs") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(f.abs()).bits()
    })
}

#[no_mangle]
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
        MoltObject::from_float(x.copysign(y)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_sqrt(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "sqrt") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            if f.is_sign_negative() {
                return raise_exception::<_>(_py, "ValueError", "math domain error");
            }
            return MoltObject::from_float(f).bits();
        }
        if f < 0.0 {
            return raise_exception::<_>(_py, "ValueError", "math domain error");
        }
        MoltObject::from_float(math_sqrt(f)).bits()
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
            return MoltObject::from_float(f64::NAN).bits();
        }
        if y.is_infinite() {
            return MoltObject::from_float(x).bits();
        }
        MoltObject::from_float(math_fmod(x, y)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_modf(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "modf") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            let bits = MoltObject::from_float(f).bits();
            return tuple2_bits(_py, bits, bits);
        }
        if f.is_infinite() {
            let frac = MoltObject::from_float(0.0_f64.copysign(f)).bits();
            let int = MoltObject::from_float(f).bits();
            return tuple2_bits(_py, frac, int);
        }
        if f == 0.0 {
            let bits = MoltObject::from_float(f).bits();
            return tuple2_bits(_py, bits, bits);
        }
        let int_part = math_trunc(f);
        let frac_part = f - int_part;
        let frac_bits = MoltObject::from_float(frac_part).bits();
        let int_bits = MoltObject::from_float(int_part).bits();
        tuple2_bits(_py, frac_bits, int_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_math_frexp(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "frexp") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() || f.is_infinite() || f == 0.0 {
            let frac_bits = MoltObject::from_float(f).bits();
            let exp_bits = MoltObject::from_int(0).bits();
            return tuple2_bits(_py, frac_bits, exp_bits);
        }
        let (mantissa, exp) = math_frexp(f);
        let frac_bits = MoltObject::from_float(mantissa).bits();
        let exp_bits = MoltObject::from_int(exp as i64).bits();
        tuple2_bits(_py, frac_bits, exp_bits)
    })
}

#[no_mangle]
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
            return MoltObject::from_float(f).bits();
        }
        if exp > i32::MAX as i64 {
            if f == 0.0 {
                return MoltObject::from_float(f).bits();
            }
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        if exp < i32::MIN as i64 {
            if f == 0.0 {
                return MoltObject::from_float(f).bits();
            }
            return MoltObject::from_float(0.0_f64.copysign(f)).bits();
        }
        let out = math_ldexp(f, exp as i32);
        if out.is_infinite() && f != 0.0 {
            return raise_exception::<_>(_py, "OverflowError", "math range error");
        }
        MoltObject::from_float(out).bits()
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
        MoltObject::from_float(sum).bits()
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_math_degrees(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "degrees") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        let out = f * (180.0 / std::f64::consts::PI);
        MoltObject::from_float(out).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_radians(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real_named(_py, val_bits, "radians") else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        let out = f * (std::f64::consts::PI / 180.0);
        MoltObject::from_float(out).bits()
    })
}

#[no_mangle]
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
                return MoltObject::from_float(0.0).bits();
            }
            let mut total = 0.0_f64;
            for &val_bits in elems {
                let Some(value) = coerce_real_named(_py, val_bits, "hypot") else {
                    return MoltObject::none().bits();
                };
                let Some(f) = coerce_to_f64(_py, value) else {
                    return MoltObject::none().bits();
                };
                total = math_hypot(total, f);
            }
            MoltObject::from_float(total).bits()
        }
    })
}

#[no_mangle]
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
        let mut total = 0.0_f64;
        for (lhs, rhs) in p_vals.iter().zip(q_vals.iter()) {
            total = math_hypot(total, lhs - rhs);
        }
        MoltObject::from_float(total).bits()
    })
}

#[no_mangle]
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

#[no_mangle]
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
            return MoltObject::from_float(f64::NAN).bits();
        }
        if x == y {
            return MoltObject::from_float(y).bits();
        }
        MoltObject::from_float(math_nextafter(x, y)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_math_ulp(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = coerce_real(_py, val_bits) else {
            return MoltObject::none().bits();
        };
        let Some(f) = coerce_to_f64(_py, value) else {
            return MoltObject::none().bits();
        };
        if f.is_nan() {
            return MoltObject::from_float(f).bits();
        }
        if f.is_infinite() {
            return MoltObject::from_float(f64::INFINITY).bits();
        }
        let next = math_nextafter(f, f64::INFINITY);
        MoltObject::from_float((next - f).abs()).bits()
    })
}

#[no_mangle]
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
            return MoltObject::from_float(f64::NAN).bits();
        }
        if y == 0.0 || x.is_infinite() {
            return math_domain_error(_py);
        }
        MoltObject::from_float(math_remainder(x, y)).bits()
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
    let values = collect_real_vec(_py, data_bits)?;
    let n = values.len();
    if n < 2 {
        raise_exception::<()>(_py, "ValueError", "stdev requires at least two data points");
        return None;
    }
    let mean = if obj_from_bits(xbar_bits).is_none() {
        sum_f64_simd(&values) / n as f64
    } else {
        let value = coerce_real_named(_py, xbar_bits, "stdev")?;
        coerce_to_f64(_py, value)?
    };
    let sum_sq = sum_sq_diff_f64_simd(&values, mean);
    let variance = if sum_sq < 0.0 && sum_sq > -f64::EPSILON {
        0.0
    } else {
        sum_sq / (n - 1) as f64
    };
    Some(math_sqrt(variance))
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

#[no_mangle]
pub extern "C" fn molt_statistics_mean(data_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(mean) = statistics_mean_value(_py, data_bits) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(mean).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_statistics_stdev(data_bits: u64, xbar_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(stdev) = statistics_stdev_value(_py, data_bits, xbar_bits) else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(stdev).bits()
    })
}

#[no_mangle]
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
                    return MoltObject::from_float(sum / count as f64).bits();
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
            Some(mean) => MoltObject::from_float(mean).bits(),
            None => MoltObject::none().bits(),
        };
        if maybe_ptr_from_bits(sliced_bits).is_some() {
            dec_ref_bits(_py, sliced_bits);
        }
        out
    })
}

#[no_mangle]
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
                    return MoltObject::from_float(math_sqrt(variance)).bits();
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
            Some(stdev) => MoltObject::from_float(stdev).bits(),
            None => MoltObject::none().bits(),
        };
        if maybe_ptr_from_bits(sliced_bits).is_some() {
            dec_ref_bits(_py, sliced_bits);
        }
        out
    })
}
