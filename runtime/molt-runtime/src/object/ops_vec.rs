//! Vectorized sum/prod/min/max operations.
//! Extracted from ops.rs for compilation-unit size reduction.

use super::ops::{range_components_i64, range_len_i128};
use crate::*;
use molt_obj_model::MoltObject;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

fn vec_sum_result(_py: &PyToken<'_>, sum_bits: u64, ok: bool) -> u64 {
    let ok_bits = MoltObject::from_bool(ok).bits();
    let tuple_ptr = alloc_tuple(_py, &[sum_bits, ok_bits]);
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

fn vec_sum_i64_result(_py: &PyToken<'_>, value: i64, ok: bool) -> u64 {
    let value_bits = int_bits_from_i64(_py, value);
    let out = vec_sum_result(_py, value_bits, ok);
    dec_ref_bits(_py, value_bits);
    out
}

fn vec_sum_f64_result(_py: &PyToken<'_>, value: f64, ok: bool) -> u64 {
    vec_sum_result(_py, MoltObject::from_float(value).bits(), ok)
}

fn number_as_f64(obj: MoltObject) -> Option<f64> {
    if let Some(f) = obj.as_float() {
        return Some(f);
    }
    obj.as_int().map(|i| i as f64)
}

fn sum_floats_scalar(elems: &[u64], acc: f64) -> Option<f64> {
    let mut vals: Vec<f64> = Vec::with_capacity(elems.len());
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        vals.push(number_as_f64(obj)?);
    }
    Some(sum_f64_neumaier(&vals, acc))
}

/// Extract all elements as f64 and compute Neumaier compensated sum.
/// Returns None if any element is not a number (falls back to generic path).
/// Uses Neumaier summation instead of SIMD to match CPython >= 3.12 `sum()`.
fn sum_floats_simd(elems: &[u64], acc: f64) -> Option<f64> {
    // Pre-extract all f64 values
    let mut vals: Vec<f64> = Vec::with_capacity(elems.len());
    for &bits in elems {
        vals.push(number_as_f64(MoltObject::from_bits(bits))?);
    }
    Some(sum_f64_neumaier(&vals, acc))
}

fn sum_float_range_arith_checked(start: i64, stop: i64, step: i64, acc: f64) -> Option<f64> {
    let len = range_len_i128(start, stop, step);
    if len <= 0 {
        return Some(acc);
    }
    let n = len as f64;
    let first = start as f64;
    let stride = step as f64;
    let last = first + stride * (n - 1.0);
    let total = acc + (n * (first + last) * 0.5);
    total.is_finite().then_some(total)
}
const VEC_LANE_WARMUP_SAMPLES: u64 = 128;
const VEC_LANE_MISS_RATIO_LIMIT: u64 = 4;
static VEC_SUM_INT_HITS: AtomicU64 = AtomicU64::new(0);
static VEC_SUM_INT_MISSES: AtomicU64 = AtomicU64::new(0);
static VEC_SUM_FLOAT_HITS: AtomicU64 = AtomicU64::new(0);
static VEC_SUM_FLOAT_MISSES: AtomicU64 = AtomicU64::new(0);

fn adaptive_vec_lanes_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| {
        std::env::var("MOLT_ADAPTIVE_VEC_LANES")
            .ok()
            .map(|raw| {
                let norm = raw.trim().to_ascii_lowercase();
                !matches!(norm.as_str(), "0" | "false" | "off" | "no")
            })
            .unwrap_or(true)
    })
}

fn vec_lane_allowed(hits: &AtomicU64, misses: &AtomicU64) -> bool {
    if !adaptive_vec_lanes_enabled() {
        return true;
    }
    let hit = hits.load(Ordering::Relaxed);
    let miss = misses.load(Ordering::Relaxed);
    let samples = hit.saturating_add(miss);
    if samples < VEC_LANE_WARMUP_SAMPLES {
        return true;
    }
    miss <= hit.saturating_mul(VEC_LANE_MISS_RATIO_LIMIT)
}

fn vec_lane_record(hits: &AtomicU64, misses: &AtomicU64, success: bool) {
    if !adaptive_vec_lanes_enabled() {
        return;
    }
    if success {
        hits.fetch_add(1, Ordering::Relaxed);
    } else {
        misses.fetch_add(1, Ordering::Relaxed);
    }
}

fn sum_ints_checked(elems: &[u64], acc: i64) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { sum_ints_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { sum_ints_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { sum_ints_simd_aarch64(elems, acc) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { sum_ints_simd_wasm32(elems, acc) };
    }
    sum_ints_scalar(elems, acc)
}

fn prod_ints_unboxed(elems: &[i64], acc: i64) -> i64 {
    let mut prod = acc;
    if prod == 0 {
        return 0;
    }
    if prod == 1
        && let Some(result) = prod_ints_unboxed_trivial(elems)
    {
        return result;
    }
    for &val in elems {
        if val == 0 {
            return 0;
        }
        prod *= val;
    }
    prod
}

fn prod_ints_checked(elems: &[u64], acc: i64) -> Option<i64> {
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { prod_ints_simd_aarch64(elems, acc) };
        }
    }
    prod_ints_scalar(elems, acc)
}

fn min_ints_checked(elems: &[u64], acc: i64) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { min_ints_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return unsafe { min_ints_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { min_ints_simd_aarch64(elems, acc) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { min_ints_simd_wasm32(elems, acc) };
    }
    min_ints_scalar(elems, acc)
}

fn max_ints_checked(elems: &[u64], acc: i64) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { max_ints_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return unsafe { max_ints_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { max_ints_simd_aarch64(elems, acc) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { max_ints_simd_wasm32(elems, acc) };
    }
    max_ints_scalar(elems, acc)
}

fn sum_ints_trusted(elems: &[u64], acc: i64) -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { sum_ints_trusted_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { sum_ints_trusted_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { sum_ints_trusted_simd_aarch64(elems, acc) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { sum_ints_trusted_simd_wasm32(elems, acc) };
    }
    sum_ints_trusted_scalar(elems, acc)
}

fn prod_ints_trusted(elems: &[u64], acc: i64) -> i64 {
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { prod_ints_trusted_simd_aarch64(elems, acc) };
        }
    }
    prod_ints_trusted_scalar(elems, acc)
}

fn min_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut min_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val < min_val {
            min_val = val;
        }
    }
    min_val
}

#[cfg(target_arch = "x86_64")]
unsafe fn min_ints_trusted_simd_x86_64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_min = _mm_set1_epi64x(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let vec = _mm_set_epi64x(v1, v0);
        let cmp = _mm_cmpgt_epi64(vec_min, vec);
        vec_min = _mm_blendv_epi8(vec_min, vec, cmp);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_min);
    let mut min_val = acc.min(lanes[0]).min(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val < min_val {
            min_val = val;
        }
    }
    min_val
}

#[cfg(target_arch = "x86_64")]
unsafe fn min_ints_trusted_simd_x86_64_avx2(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_min = _mm256_set1_epi64x(acc);
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let v2 = obj2.as_int_unchecked();
        let v3 = obj3.as_int_unchecked();
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        let cmp = _mm256_cmpgt_epi64(vec_min, vec);
        vec_min = _mm256_blendv_epi8(vec_min, vec, cmp);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_min);
    let mut min_val = acc;
    for lane in lanes {
        if lane < min_val {
            min_val = lane;
        }
    }
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val < min_val {
            min_val = val;
        }
    }
    min_val
}

#[cfg(target_arch = "aarch64")]
unsafe fn min_ints_trusted_simd_aarch64(elems: &[u64], acc: i64) -> i64 {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vec_min = vdupq_n_s64(acc);
        while i + 2 <= elems.len() {
            let obj0 = MoltObject::from_bits(elems[i]);
            let obj1 = MoltObject::from_bits(elems[i + 1]);
            let v0 = obj0.as_int_unchecked();
            let v1 = obj1.as_int_unchecked();
            let lanes = [v0, v1];
            let vec = vld1q_s64(lanes.as_ptr());
            let mask = vcgtq_s64(vec_min, vec);
            let vec_min_u = vreinterpretq_u64_s64(vec_min);
            let vec_u = vreinterpretq_u64_s64(vec);
            let blended_u = vbslq_u64(mask, vec_u, vec_min_u);
            vec_min = vreinterpretq_s64_u64(blended_u);
            i += 2;
        }
        let mut lanes = [0i64; 2];
        vst1q_s64(lanes.as_mut_ptr(), vec_min);
        let mut min_val = acc.min(lanes[0]).min(lanes[1]);
        for &bits in &elems[i..] {
            let obj = MoltObject::from_bits(bits);
            let val = obj.as_int_unchecked();
            if val < min_val {
                min_val = val;
            }
        }
        min_val
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn min_ints_trusted_simd_wasm32(elems: &[u64], acc: i64) -> i64 {
    let mut min_val = acc;
    for &bits in elems {
        let val = MoltObject::from_bits(bits).as_int_unchecked();
        if val < min_val {
            min_val = val;
        }
    }
    min_val
}

fn min_ints_trusted(elems: &[u64], acc: i64) -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { min_ints_trusted_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return unsafe { min_ints_trusted_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { min_ints_trusted_simd_aarch64(elems, acc) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { min_ints_trusted_simd_wasm32(elems, acc) };
    }
    min_ints_trusted_scalar(elems, acc)
}

fn max_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut max_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val > max_val {
            max_val = val;
        }
    }
    max_val
}

#[cfg(target_arch = "x86_64")]
unsafe fn max_ints_trusted_simd_x86_64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_max = _mm_set1_epi64x(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let vec = _mm_set_epi64x(v1, v0);
        let cmp = _mm_cmpgt_epi64(vec, vec_max);
        vec_max = _mm_blendv_epi8(vec_max, vec, cmp);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_max);
    let mut max_val = acc.max(lanes[0]).max(lanes[1]);
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val > max_val {
            max_val = val;
        }
    }
    max_val
}

#[cfg(target_arch = "x86_64")]
unsafe fn max_ints_trusted_simd_x86_64_avx2(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_max = _mm256_set1_epi64x(acc);
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let v2 = obj2.as_int_unchecked();
        let v3 = obj3.as_int_unchecked();
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        let cmp = _mm256_cmpgt_epi64(vec, vec_max);
        vec_max = _mm256_blendv_epi8(vec_max, vec, cmp);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_max);
    let mut max_val = acc;
    for lane in lanes {
        if lane > max_val {
            max_val = lane;
        }
    }
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val > max_val {
            max_val = val;
        }
    }
    max_val
}

#[cfg(target_arch = "aarch64")]
unsafe fn max_ints_trusted_simd_aarch64(elems: &[u64], acc: i64) -> i64 {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vec_max = vdupq_n_s64(acc);
        while i + 2 <= elems.len() {
            let obj0 = MoltObject::from_bits(elems[i]);
            let obj1 = MoltObject::from_bits(elems[i + 1]);
            let v0 = obj0.as_int_unchecked();
            let v1 = obj1.as_int_unchecked();
            let lanes = [v0, v1];
            let vec = vld1q_s64(lanes.as_ptr());
            let mask = vcgtq_s64(vec, vec_max);
            let vec_max_u = vreinterpretq_u64_s64(vec_max);
            let vec_u = vreinterpretq_u64_s64(vec);
            let blended_u = vbslq_u64(mask, vec_u, vec_max_u);
            vec_max = vreinterpretq_s64_u64(blended_u);
            i += 2;
        }
        let mut lanes = [0i64; 2];
        vst1q_s64(lanes.as_mut_ptr(), vec_max);
        let mut max_val = acc.max(lanes[0]).max(lanes[1]);
        for &bits in &elems[i..] {
            let obj = MoltObject::from_bits(bits);
            let val = obj.as_int_unchecked();
            if val > max_val {
                max_val = val;
            }
        }
        max_val
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn max_ints_trusted_simd_wasm32(elems: &[u64], acc: i64) -> i64 {
    let mut max_val = acc;
    for &bits in elems {
        let val = MoltObject::from_bits(bits).as_int_unchecked();
        if val > max_val {
            max_val = val;
        }
    }
    max_val
}

fn max_ints_trusted(elems: &[u64], acc: i64) -> i64 {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { max_ints_trusted_simd_x86_64_avx2(elems, acc) };
        }
        if std::arch::is_x86_feature_detected!("sse4.2") {
            return unsafe { max_ints_trusted_simd_x86_64(elems, acc) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { max_ints_trusted_simd_aarch64(elems, acc) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { max_ints_trusted_simd_wasm32(elems, acc) };
    }
    max_ints_trusted_scalar(elems, acc)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_int(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        if !vec_lane_allowed(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES) {
            return vec_sum_i64_result(_py, acc, false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                vec_lane_record(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES, false);
                return vec_sum_i64_result(_py, acc, false);
            }
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                vec_lane_record(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES, false);
                return vec_sum_i64_result(_py, acc, false);
            };
            if let Some(sum) = sum_ints_checked(elems, acc) {
                vec_lane_record(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES, true);
                return vec_sum_i64_result(_py, sum, true);
            }
        }
        vec_lane_record(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES, false);
        vec_sum_i64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        if !vec_lane_allowed(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES) {
            return vec_sum_i64_result(_py, acc, false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                vec_lane_record(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES, false);
                return vec_sum_i64_result(_py, acc, false);
            }
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                vec_lane_record(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES, false);
                return vec_sum_i64_result(_py, acc, false);
            };
            let sum = sum_ints_trusted(elems, acc);
            vec_lane_record(&VEC_SUM_INT_HITS, &VEC_SUM_INT_MISSES, true);
            vec_sum_i64_result(_py, sum, true)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_prod_int(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_INTARRAY {
                let elems = intarray_slice(ptr);
                let prod = prod_ints_unboxed(elems, acc);
                return vec_sum_result(_py, MoltObject::from_int(prod).bits(), true);
            }
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            if let Some(prod) = prod_ints_checked(elems, acc) {
                return vec_sum_result(_py, MoltObject::from_int(prod).bits(), true);
            }
        }
        vec_sum_result(_py, MoltObject::from_int(acc).bits(), false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_prod_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_INTARRAY {
                let elems = intarray_slice(ptr);
                let prod = prod_ints_unboxed(elems, acc);
                return vec_sum_result(_py, MoltObject::from_int(prod).bits(), true);
            }
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let prod = prod_ints_trusted(elems, acc);
            vec_sum_result(_py, MoltObject::from_int(prod).bits(), true)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_min_int(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            if let Some(val) = min_ints_checked(elems, acc) {
                return vec_sum_result(_py, MoltObject::from_int(val).bits(), true);
            }
        }
        vec_sum_result(_py, MoltObject::from_int(acc).bits(), false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_min_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let val = min_ints_trusted(elems, acc);
            vec_sum_result(_py, MoltObject::from_int(val).bits(), true)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_max_int(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            if let Some(val) = max_ints_checked(elems, acc) {
                return vec_sum_result(_py, MoltObject::from_int(val).bits(), true);
            }
        }
        vec_sum_result(_py, MoltObject::from_int(acc).bits(), false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_max_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let val = max_ints_trusted(elems, acc);
            vec_sum_result(_py, MoltObject::from_int(val).bits(), true)
        }
    })
}

fn sum_int_range_arith_checked(start: i64, stop: i64, step: i64, acc: i64) -> Option<i64> {
    let len = range_len_i128(start, stop, step);
    if len <= 0 {
        return Some(acc);
    }
    let n = len;
    let first = i128::from(start);
    let stride = i128::from(step);
    let last = first.checked_add(stride.checked_mul(n.checked_sub(1)?)?)?;
    let two_term_sum = first.checked_add(last)?;
    let range_sum = n.checked_mul(two_term_sum)?.checked_div(2)?;
    let total = i128::from(acc).checked_add(range_sum)?;
    i64::try_from(total).ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_int_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start_obj = obj_from_bits(start_bits);
        let start = match start_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_i64_result(_py, acc, false),
        };
        if start < 0 {
            return vec_sum_i64_result(_py, acc, false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_i64_result(_py, acc, false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_i64_result(_py, acc, false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            if let Some(sum) = sum_ints_checked(slice, acc) {
                return vec_sum_i64_result(_py, sum, true);
            }
        }
        vec_sum_i64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_int_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start_obj = obj_from_bits(start_bits);
        let start = match start_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_i64_result(_py, acc, false),
        };
        if start < 0 {
            return vec_sum_i64_result(_py, acc, false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_i64_result(_py, acc, false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_i64_result(_py, acc, false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            let sum = sum_ints_trusted(slice, acc);
            vec_sum_i64_result(_py, sum, true)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_int_range_iter(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc = match obj_from_bits(acc_bits).as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let ptr = match obj_from_bits(seq_bits).as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_i64_result(_py, acc, false),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_RANGE {
                return vec_sum_i64_result(_py, acc, false);
            }
            let Some((start, stop, step)) = range_components_i64(ptr) else {
                return vec_sum_i64_result(_py, acc, false);
            };
            if let Some(sum) = sum_int_range_arith_checked(start, stop, step, acc) {
                return vec_sum_i64_result(_py, sum, true);
            }
        }
        vec_sum_i64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_int_range_iter_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc = match obj_from_bits(acc_bits).as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let ptr = match obj_from_bits(seq_bits).as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_i64_result(_py, acc, false),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_RANGE {
                return vec_sum_i64_result(_py, acc, false);
            }
            let Some((start, stop, step)) = range_components_i64(ptr) else {
                return vec_sum_i64_result(_py, acc, false);
            };
            if let Some(sum) = sum_int_range_arith_checked(start, stop, step, acc) {
                return vec_sum_i64_result(_py, sum, true);
            }
        }
        vec_sum_i64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_float(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc = match number_as_f64(obj_from_bits(acc_bits)) {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        if !vec_lane_allowed(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES) {
            return vec_sum_f64_result(_py, acc, false);
        }
        let ptr = match obj_from_bits(seq_bits).as_ptr() {
            Some(ptr) => ptr,
            None => {
                vec_lane_record(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES, false);
                return vec_sum_f64_result(_py, acc, false);
            }
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                vec_lane_record(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES, false);
                return vec_sum_f64_result(_py, acc, false);
            };
            if let Some(sum) = sum_floats_simd(elems, acc) {
                vec_lane_record(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES, true);
                return vec_sum_f64_result(_py, sum, true);
            }
        }
        vec_lane_record(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES, false);
        vec_sum_f64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_float_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc = match number_as_f64(obj_from_bits(acc_bits)) {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        if !vec_lane_allowed(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES) {
            return vec_sum_f64_result(_py, acc, false);
        }
        let ptr = match obj_from_bits(seq_bits).as_ptr() {
            Some(ptr) => ptr,
            None => {
                vec_lane_record(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES, false);
                return vec_sum_f64_result(_py, acc, false);
            }
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                vec_lane_record(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES, false);
                return vec_sum_f64_result(_py, acc, false);
            };
            if let Some(sum) = sum_floats_simd(elems, acc) {
                vec_lane_record(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES, true);
                return vec_sum_f64_result(_py, sum, true);
            }
        }
        vec_lane_record(&VEC_SUM_FLOAT_HITS, &VEC_SUM_FLOAT_MISSES, false);
        vec_sum_f64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_float_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc = match number_as_f64(obj_from_bits(acc_bits)) {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start = match obj_from_bits(start_bits).as_int() {
            Some(val) => val,
            None => return vec_sum_f64_result(_py, acc, false),
        };
        if start < 0 {
            return vec_sum_f64_result(_py, acc, false);
        }
        let ptr = match obj_from_bits(seq_bits).as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_f64_result(_py, acc, false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_f64_result(_py, acc, false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            if let Some(sum) = sum_floats_scalar(slice, acc) {
                return vec_sum_f64_result(_py, sum, true);
            }
        }
        vec_sum_f64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_float_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc = match number_as_f64(obj_from_bits(acc_bits)) {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start = match obj_from_bits(start_bits).as_int() {
            Some(val) => val,
            None => return vec_sum_f64_result(_py, acc, false),
        };
        if start < 0 {
            return vec_sum_f64_result(_py, acc, false);
        }
        let ptr = match obj_from_bits(seq_bits).as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_f64_result(_py, acc, false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_f64_result(_py, acc, false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            if let Some(sum) = sum_floats_scalar(slice, acc) {
                return vec_sum_f64_result(_py, sum, true);
            }
        }
        vec_sum_f64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_float_range_iter(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc = match number_as_f64(obj_from_bits(acc_bits)) {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let ptr = match obj_from_bits(seq_bits).as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_f64_result(_py, acc, false),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_RANGE {
                return vec_sum_f64_result(_py, acc, false);
            }
            let Some((start, stop, step)) = range_components_i64(ptr) else {
                return vec_sum_f64_result(_py, acc, false);
            };
            if let Some(sum) = sum_float_range_arith_checked(start, stop, step, acc) {
                return vec_sum_f64_result(_py, sum, true);
            }
        }
        vec_sum_f64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_sum_float_range_iter_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc = match number_as_f64(obj_from_bits(acc_bits)) {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let ptr = match obj_from_bits(seq_bits).as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_f64_result(_py, acc, false),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_RANGE {
                return vec_sum_f64_result(_py, acc, false);
            }
            let Some((start, stop, step)) = range_components_i64(ptr) else {
                return vec_sum_f64_result(_py, acc, false);
            };
            if let Some(sum) = sum_float_range_arith_checked(start, stop, step, acc) {
                return vec_sum_f64_result(_py, sum, true);
            }
        }
        vec_sum_f64_result(_py, acc, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_prod_int_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start_obj = obj_from_bits(start_bits);
        let start = match start_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        if start < 0 {
            return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_INTARRAY {
                let elems = intarray_slice(ptr);
                let start_idx = (start as usize).min(elems.len());
                let slice = &elems[start_idx..];
                let prod = prod_ints_unboxed(slice, acc);
                return vec_sum_result(_py, MoltObject::from_int(prod).bits(), true);
            }
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            if let Some(prod) = prod_ints_checked(slice, acc) {
                return vec_sum_result(_py, MoltObject::from_int(prod).bits(), true);
            }
        }
        vec_sum_result(_py, MoltObject::from_int(acc).bits(), false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_prod_int_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start_obj = obj_from_bits(start_bits);
        let start = match start_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        if start < 0 {
            return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_INTARRAY {
                let elems = intarray_slice(ptr);
                let start_idx = (start as usize).min(elems.len());
                let slice = &elems[start_idx..];
                let prod = prod_ints_unboxed(slice, acc);
                return vec_sum_result(_py, MoltObject::from_int(prod).bits(), true);
            }
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            let prod = prod_ints_trusted(slice, acc);
            vec_sum_result(_py, MoltObject::from_int(prod).bits(), true)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_min_int_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start_obj = obj_from_bits(start_bits);
        let start = match start_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        if start < 0 {
            return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            if let Some(val) = min_ints_checked(slice, acc) {
                return vec_sum_result(_py, MoltObject::from_int(val).bits(), true);
            }
        }
        vec_sum_result(_py, MoltObject::from_int(acc).bits(), false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_min_int_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start_obj = obj_from_bits(start_bits);
        let start = match start_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        if start < 0 {
            return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            let val = min_ints_trusted(slice, acc);
            vec_sum_result(_py, MoltObject::from_int(val).bits(), true)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_max_int_range(seq_bits: u64, acc_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start_obj = obj_from_bits(start_bits);
        let start = match start_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        if start < 0 {
            return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            if let Some(val) = max_ints_checked(slice, acc) {
                return vec_sum_result(_py, MoltObject::from_int(val).bits(), true);
            }
        }
        vec_sum_result(_py, MoltObject::from_int(acc).bits(), false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vec_max_int_range_trusted(
    seq_bits: u64,
    acc_bits: u64,
    start_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let acc_obj = obj_from_bits(acc_bits);
        let acc = match acc_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::none().bits(), false),
        };
        let start_obj = obj_from_bits(start_bits);
        let start = match start_obj.as_int() {
            Some(val) => val,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        if start < 0 {
            return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
        }
        let seq_obj = obj_from_bits(seq_bits);
        let ptr = match seq_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false),
        };
        unsafe {
            let type_id = object_type_id(ptr);
            let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                seq_vec_ref(ptr)
            } else {
                return vec_sum_result(_py, MoltObject::from_int(acc).bits(), false);
            };
            let start_idx = (start as usize).min(elems.len());
            let slice = &elems[start_idx..];
            let val = max_ints_trusted(slice, acc);
            vec_sum_result(_py, MoltObject::from_int(val).bits(), true)
        }
    })
}

/// Neumaier compensated summation on pre-extracted f64 values.
/// Matches CPython >= 3.12 `sum()` for float sequences.
fn sum_f64_neumaier(vals: &[f64], acc: f64) -> f64 {
    let mut sum = acc;
    let mut comp = 0.0_f64;
    for &x in vals {
        let t = sum + x;
        if sum.abs() >= x.abs() {
            comp += (sum - t) + x;
        } else {
            comp += (x - t) + sum;
        }
        sum = t;
    }
    sum + comp
}

fn sum_ints_scalar(elems: &[u64], acc: i64) -> Option<i64> {
    let mut sum = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        if let Some(val) = obj.as_int() {
            sum += val;
        } else {
            return None;
        }
    }
    Some(sum)
}

#[cfg(target_arch = "aarch64")]
unsafe fn sum_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vec_sum = vdupq_n_s64(0);
        while i + 2 <= elems.len() {
            let obj0 = MoltObject::from_bits(elems[i]);
            let obj1 = MoltObject::from_bits(elems[i + 1]);
            let v0 = obj0.as_int()?;
            let v1 = obj1.as_int()?;
            let lanes = [v0, v1];
            let vec = vld1q_s64(lanes.as_ptr());
            vec_sum = vaddq_s64(vec_sum, vec);
            i += 2;
        }
        let mut lanes = [0i64; 2];
        vst1q_s64(lanes.as_mut_ptr(), vec_sum);
        let mut sum = acc + lanes[0] + lanes[1];
        for &bits in &elems[i..] {
            let obj = MoltObject::from_bits(bits);
            let val = obj.as_int()?;
            sum += val;
        }
        Some(sum)
    }
}

fn prod_ints_scalar(elems: &[u64], acc: i64) -> Option<i64> {
    let mut prod = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        if let Some(val) = obj.as_int() {
            prod *= val;
        } else {
            return None;
        }
    }
    Some(prod)
}

fn prod_ints_unboxed_trivial(_elems: &[i64]) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { prod_ints_unboxed_avx2_trivial(_elems) };
        }
    }
    None
}

#[cfg(target_arch = "aarch64")]
unsafe fn prod_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    prod_ints_scalar(elems, acc)
}

fn min_ints_scalar(elems: &[u64], acc: i64) -> Option<i64> {
    let mut min_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        if let Some(val) = obj.as_int() {
            if val < min_val {
                min_val = val;
            }
        } else {
            return None;
        }
    }
    Some(min_val)
}

#[cfg(target_arch = "aarch64")]
unsafe fn min_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vec_min = vdupq_n_s64(acc);
        while i + 2 <= elems.len() {
            let obj0 = MoltObject::from_bits(elems[i]);
            let obj1 = MoltObject::from_bits(elems[i + 1]);
            let v0 = obj0.as_int()?;
            let v1 = obj1.as_int()?;
            let lanes = [v0, v1];
            let vec = vld1q_s64(lanes.as_ptr());
            let mask = vcgtq_s64(vec_min, vec);
            let vec_min_u = vreinterpretq_u64_s64(vec_min);
            let vec_u = vreinterpretq_u64_s64(vec);
            let blended_u = vbslq_u64(mask, vec_u, vec_min_u);
            vec_min = vreinterpretq_s64_u64(blended_u);
            i += 2;
        }
        let mut lanes = [0i64; 2];
        vst1q_s64(lanes.as_mut_ptr(), vec_min);
        let mut min_val = acc.min(lanes[0]).min(lanes[1]);
        for &bits in &elems[i..] {
            let obj = MoltObject::from_bits(bits);
            let val = obj.as_int()?;
            if val < min_val {
                min_val = val;
            }
        }
        Some(min_val)
    }
}

fn max_ints_scalar(elems: &[u64], acc: i64) -> Option<i64> {
    let mut max_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        if let Some(val) = obj.as_int() {
            if val > max_val {
                max_val = val;
            }
        } else {
            return None;
        }
    }
    Some(max_val)
}

#[cfg(target_arch = "aarch64")]
unsafe fn max_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vec_max = vdupq_n_s64(acc);
        while i + 2 <= elems.len() {
            let obj0 = MoltObject::from_bits(elems[i]);
            let obj1 = MoltObject::from_bits(elems[i + 1]);
            let v0 = obj0.as_int()?;
            let v1 = obj1.as_int()?;
            let lanes = [v0, v1];
            let vec = vld1q_s64(lanes.as_ptr());
            let mask = vcgtq_s64(vec, vec_max);
            let vec_max_u = vreinterpretq_u64_s64(vec_max);
            let vec_u = vreinterpretq_u64_s64(vec);
            let blended_u = vbslq_u64(mask, vec_u, vec_max_u);
            vec_max = vreinterpretq_s64_u64(blended_u);
            i += 2;
        }
        let mut lanes = [0i64; 2];
        vst1q_s64(lanes.as_mut_ptr(), vec_max);
        let mut max_val = acc.max(lanes[0]).max(lanes[1]);
        for &bits in &elems[i..] {
            let obj = MoltObject::from_bits(bits);
            let val = obj.as_int()?;
            if val > max_val {
                max_val = val;
            }
        }
        Some(max_val)
    }
}

fn sum_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut sum = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        sum += obj.as_int_unchecked();
    }
    sum
}

#[cfg(target_arch = "aarch64")]
unsafe fn sum_ints_trusted_simd_aarch64(elems: &[u64], acc: i64) -> i64 {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vec_sum = vdupq_n_s64(0);
        while i + 2 <= elems.len() {
            let obj0 = MoltObject::from_bits(elems[i]);
            let obj1 = MoltObject::from_bits(elems[i + 1]);
            let v0 = obj0.as_int_unchecked();
            let v1 = obj1.as_int_unchecked();
            let lanes = [v0, v1];
            let vec = vld1q_s64(lanes.as_ptr());
            vec_sum = vaddq_s64(vec_sum, vec);
            i += 2;
        }
        let mut lanes = [0i64; 2];
        vst1q_s64(lanes.as_mut_ptr(), vec_sum);
        let mut sum = acc + lanes[0] + lanes[1];
        for &bits in &elems[i..] {
            let obj = MoltObject::from_bits(bits);
            sum += obj.as_int_unchecked();
        }
        sum
    }
}

fn prod_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut prod = acc;
    if prod == 0 {
        return 0;
    }
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int_unchecked();
        if val == 0 {
            return 0;
        }
        prod *= val;
    }
    prod
}

#[cfg(target_arch = "aarch64")]
unsafe fn prod_ints_trusted_simd_aarch64(elems: &[u64], acc: i64) -> i64 {
    prod_ints_trusted_scalar(elems, acc)
}
