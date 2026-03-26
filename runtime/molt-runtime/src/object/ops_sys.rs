// System, GC, time, signal, traceback, profiling, and SIMD aggregate operations.
// Split from ops.rs for compilation-unit size reduction.

use crate::*;
use crate::object::ops_string::{push_wtf8_codepoint, wtf8_codepoint_at};
use crate::state::runtime_state::PythonVersionInfo;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::CStr;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
#[cfg(not(target_arch = "wasm32"))]
use std::ffi::CString;

#[unsafe(no_mangle)]
pub extern "C" fn molt_profile_dump() {
    crate::with_gil_entry!(_py, {
        if !profile_enabled(_py) {
            return;
        }
        let call_dispatch = CALL_DISPATCH_COUNT.load(AtomicOrdering::Relaxed);
        let cache_hit = runtime_state(_py)
            .string_count_cache_hit
            .load(AtomicOrdering::Relaxed);
        let cache_miss = runtime_state(_py)
            .string_count_cache_miss
            .load(AtomicOrdering::Relaxed);
        let struct_stores = STRUCT_FIELD_STORE_COUNT.load(AtomicOrdering::Relaxed);
        let attr_lookups = ATTR_LOOKUP_COUNT.load(AtomicOrdering::Relaxed);
        let handle_resolves = HANDLE_RESOLVE_COUNT.load(AtomicOrdering::Relaxed);
        let layout_guard = LAYOUT_GUARD_COUNT.load(AtomicOrdering::Relaxed);
        let layout_guard_fail = LAYOUT_GUARD_FAIL.load(AtomicOrdering::Relaxed);
        let allocs = ALLOC_COUNT.load(AtomicOrdering::Relaxed);
        let alloc_objects = ALLOC_OBJECT_COUNT.load(AtomicOrdering::Relaxed);
        let alloc_exceptions = ALLOC_EXCEPTION_COUNT.load(AtomicOrdering::Relaxed);
        let alloc_dicts = ALLOC_DICT_COUNT.load(AtomicOrdering::Relaxed);
        let alloc_tuples = ALLOC_TUPLE_COUNT.load(AtomicOrdering::Relaxed);
        let alloc_strings = ALLOC_STRING_COUNT.load(AtomicOrdering::Relaxed);
        let alloc_callargs = ALLOC_CALLARGS_COUNT.load(AtomicOrdering::Relaxed);
        let alloc_bytes_callargs = ALLOC_BYTES_CALLARGS.load(AtomicOrdering::Relaxed);
        let tb_builds = TRACEBACK_BUILD_COUNT.load(AtomicOrdering::Relaxed);
        let tb_frames = TRACEBACK_BUILD_FRAMES.load(AtomicOrdering::Relaxed);
        let tb_suppressed = TRACEBACK_SUPPRESS_COUNT.load(AtomicOrdering::Relaxed);
        let async_polls = ASYNC_POLL_COUNT.load(AtomicOrdering::Relaxed);
        let async_pending = ASYNC_PENDING_COUNT.load(AtomicOrdering::Relaxed);
        let async_wakeups = ASYNC_WAKEUP_COUNT.load(AtomicOrdering::Relaxed);
        let async_sleep_reg = ASYNC_SLEEP_REGISTER_COUNT.load(AtomicOrdering::Relaxed);
        let call_bind_ic_hit = CALL_BIND_IC_HIT_COUNT.load(AtomicOrdering::Relaxed);
        let call_bind_ic_miss = CALL_BIND_IC_MISS_COUNT.load(AtomicOrdering::Relaxed);
        let call_indirect_noncallable_deopt =
            CALL_INDIRECT_NONCALLABLE_DEOPT_COUNT.load(AtomicOrdering::Relaxed);
        let invoke_ffi_bridge_capability_denied =
            INVOKE_FFI_BRIDGE_CAPABILITY_DENIED_COUNT.load(AtomicOrdering::Relaxed);
        let guard_tag_type_mismatch_deopt =
            GUARD_TAG_TYPE_MISMATCH_DEOPT_COUNT.load(AtomicOrdering::Relaxed);
        let guard_dict_shape_layout_mismatch_deopt =
            GUARD_DICT_SHAPE_LAYOUT_MISMATCH_DEOPT_COUNT.load(AtomicOrdering::Relaxed);
        let guard_dict_shape_layout_fail_null_obj =
            GUARD_DICT_SHAPE_LAYOUT_FAIL_NULL_OBJ_COUNT.load(AtomicOrdering::Relaxed);
        let guard_dict_shape_layout_fail_non_object =
            GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_OBJECT_COUNT.load(AtomicOrdering::Relaxed);
        let guard_dict_shape_layout_fail_class_mismatch =
            GUARD_DICT_SHAPE_LAYOUT_FAIL_CLASS_MISMATCH_COUNT.load(AtomicOrdering::Relaxed);
        let guard_dict_shape_layout_fail_non_type_class =
            GUARD_DICT_SHAPE_LAYOUT_FAIL_NON_TYPE_CLASS_COUNT.load(AtomicOrdering::Relaxed);
        let guard_dict_shape_layout_fail_expected_version_invalid =
            GUARD_DICT_SHAPE_LAYOUT_FAIL_EXPECTED_VERSION_INVALID_COUNT
                .load(AtomicOrdering::Relaxed);
        let guard_dict_shape_layout_fail_version_mismatch =
            GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT.load(AtomicOrdering::Relaxed);
        let attr_site_name_hit = ATTR_SITE_NAME_CACHE_HIT_COUNT.load(AtomicOrdering::Relaxed);
        let attr_site_name_miss = ATTR_SITE_NAME_CACHE_MISS_COUNT.load(AtomicOrdering::Relaxed);
        let split_ws_ascii = SPLIT_WS_ASCII_FAST_PATH_COUNT.load(AtomicOrdering::Relaxed);
        let split_ws_unicode = SPLIT_WS_UNICODE_PATH_COUNT.load(AtomicOrdering::Relaxed);
        let dict_str_int_prehash_hit = DICT_STR_INT_PREHASH_HIT_COUNT.load(AtomicOrdering::Relaxed);
        let dict_str_int_prehash_miss =
            DICT_STR_INT_PREHASH_MISS_COUNT.load(AtomicOrdering::Relaxed);
        let dict_str_int_prehash_deopt =
            DICT_STR_INT_PREHASH_DEOPT_COUNT.load(AtomicOrdering::Relaxed);
        let taq_ingest_calls = TAQ_INGEST_CALL_COUNT.load(AtomicOrdering::Relaxed);
        let taq_ingest_skip_marker = TAQ_INGEST_SKIP_MARKER_COUNT.load(AtomicOrdering::Relaxed);
        let ascii_i64_parse_fail = ASCII_I64_PARSE_FAIL_COUNT.load(AtomicOrdering::Relaxed);
        let alloc_bytes_total = ALLOC_BYTES_TOTAL.load(AtomicOrdering::Relaxed);
        let alloc_bytes_string = ALLOC_BYTES_STRING.load(AtomicOrdering::Relaxed);
        let alloc_bytes_dict = ALLOC_BYTES_DICT.load(AtomicOrdering::Relaxed);
        let alloc_bytes_tuple = ALLOC_BYTES_TUPLE.load(AtomicOrdering::Relaxed);
        let alloc_bytes_list = ALLOC_BYTES_LIST.load(AtomicOrdering::Relaxed);
        // Take a final RSS sample before dumping.
        sample_peak_rss();
        let peak_rss = PEAK_RSS_BYTES.load(AtomicOrdering::Relaxed);
        let current_rss = current_rss_bytes();
        eprintln!(
            "molt_profile call_dispatch={} string_count_cache_hit={} string_count_cache_miss={} struct_field_store={} attr_lookup={} handle_resolve={} layout_guard={} layout_guard_fail={} alloc_count={} alloc_object={} alloc_exception={} alloc_dict={} alloc_tuple={} alloc_string={} alloc_callargs={} alloc_bytes_callargs={} tb_builds={} tb_frames={} tb_suppressed={} async_polls={} async_pending={} async_wakeups={} async_sleep_register={} call_bind_ic_hit={} call_bind_ic_miss={} call_indirect_noncallable_deopt={} invoke_ffi_bridge_capability_denied={} guard_tag_type_mismatch_deopt={} guard_dict_shape_layout_mismatch_deopt={} attr_site_name_hit={} attr_site_name_miss={} split_ws_ascii={} split_ws_unicode={} dict_str_int_prehash_hit={} dict_str_int_prehash_miss={} dict_str_int_prehash_deopt={} taq_ingest_calls={} taq_ingest_skip_marker={} ascii_i64_parse_fail={} alloc_bytes_total={} alloc_bytes_string={} alloc_bytes_dict={} alloc_bytes_tuple={} alloc_bytes_list={} peak_rss_bytes={} current_rss_bytes={}",
            call_dispatch,
            cache_hit,
            cache_miss,
            struct_stores,
            attr_lookups,
            handle_resolves,
            layout_guard,
            layout_guard_fail,
            allocs,
            alloc_objects,
            alloc_exceptions,
            alloc_dicts,
            alloc_tuples,
            alloc_strings,
            alloc_callargs,
            alloc_bytes_callargs,
            tb_builds,
            tb_frames,
            tb_suppressed,
            async_polls,
            async_pending,
            async_wakeups,
            async_sleep_reg,
            call_bind_ic_hit,
            call_bind_ic_miss,
            call_indirect_noncallable_deopt,
            invoke_ffi_bridge_capability_denied,
            guard_tag_type_mismatch_deopt,
            guard_dict_shape_layout_mismatch_deopt,
            attr_site_name_hit,
            attr_site_name_miss,
            split_ws_ascii,
            split_ws_unicode,
            dict_str_int_prehash_hit,
            dict_str_int_prehash_miss,
            dict_str_int_prehash_deopt,
            taq_ingest_calls,
            taq_ingest_skip_marker,
            ascii_i64_parse_fail,
            alloc_bytes_total,
            alloc_bytes_string,
            alloc_bytes_dict,
            alloc_bytes_tuple,
            alloc_bytes_list,
            peak_rss,
            current_rss,
        );
        let payload = serde_json::json!({
            "schema_version": 1,
            "kind": "runtime_feedback",
            "profile": {
                "call_dispatch": call_dispatch,
                "string_count_cache_hit": cache_hit,
                "string_count_cache_miss": cache_miss,
                "struct_field_store": struct_stores,
                "attr_lookup": attr_lookups,
                "handle_resolve": handle_resolves,
                "layout_guard": layout_guard,
                "layout_guard_fail": layout_guard_fail,
                "alloc_count": allocs,
                "alloc_object": alloc_objects,
                "alloc_exception": alloc_exceptions,
                "alloc_dict": alloc_dicts,
                "alloc_tuple": alloc_tuples,
                "alloc_string": alloc_strings,
                "alloc_callargs": alloc_callargs,
                "alloc_bytes_callargs": alloc_bytes_callargs,
                "tb_builds": tb_builds,
                "tb_frames": tb_frames,
                "tb_suppressed": tb_suppressed,
                "async_polls": async_polls,
                "async_pending": async_pending,
                "async_wakeups": async_wakeups,
                "async_sleep_register": async_sleep_reg,
                "alloc_bytes_total": alloc_bytes_total,
                "alloc_bytes_string": alloc_bytes_string,
                "alloc_bytes_dict": alloc_bytes_dict,
                "alloc_bytes_tuple": alloc_bytes_tuple,
                "alloc_bytes_list": alloc_bytes_list,
            },
            "memory": {
                "peak_rss_bytes": peak_rss,
                "current_rss_bytes": current_rss,
            },
            "hot_paths": {
                "call_bind_ic_hit": call_bind_ic_hit,
                "call_bind_ic_miss": call_bind_ic_miss,
                "attr_site_name_hit": attr_site_name_hit,
                "attr_site_name_miss": attr_site_name_miss,
                "split_ws_ascii": split_ws_ascii,
                "split_ws_unicode": split_ws_unicode,
                "dict_str_int_prehash_hit": dict_str_int_prehash_hit,
                "dict_str_int_prehash_miss": dict_str_int_prehash_miss,
                "dict_str_int_prehash_deopt": dict_str_int_prehash_deopt,
                "taq_ingest_calls": taq_ingest_calls,
                "taq_ingest_skip_marker": taq_ingest_skip_marker,
                "ascii_i64_parse_fail": ascii_i64_parse_fail,
            },
            "deopt_reasons": {
                "call_indirect_noncallable": call_indirect_noncallable_deopt,
                "invoke_ffi_bridge_capability_denied": invoke_ffi_bridge_capability_denied,
                "guard_tag_type_mismatch": guard_tag_type_mismatch_deopt,
                "guard_dict_shape_layout_mismatch": guard_dict_shape_layout_mismatch_deopt,
                "guard_dict_shape_layout_fail_null_obj": guard_dict_shape_layout_fail_null_obj,
                "guard_dict_shape_layout_fail_non_object": guard_dict_shape_layout_fail_non_object,
                "guard_dict_shape_layout_fail_class_mismatch": guard_dict_shape_layout_fail_class_mismatch,
                "guard_dict_shape_layout_fail_non_type_class": guard_dict_shape_layout_fail_non_type_class,
                "guard_dict_shape_layout_fail_expected_version_invalid": guard_dict_shape_layout_fail_expected_version_invalid,
                "guard_dict_shape_layout_fail_version_mismatch": guard_dict_shape_layout_fail_version_mismatch,
            },
        });
        if env_flag_enabled("MOLT_PROFILE_JSON") {
            eprintln!("molt_profile_json {}", payload);
        }
        maybe_emit_runtime_feedback_file(&payload);
    })
}

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

// ---------------------------------------------------------------------------
// SIMD-accelerated float sum: SSE2 (2×f64), AVX2 (4×f64), NEON (2×f64)
// ---------------------------------------------------------------------------

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

#[cfg(target_arch = "x86_64")]
unsafe fn sum_f64_simd_x86_64(vals: &[f64], acc: f64) -> f64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm_set1_pd(0.0);
    while i + 2 <= vals.len() {
        let vec = _mm_loadu_pd(vals.as_ptr().add(i));
        vec_sum = _mm_add_pd(vec_sum, vec);
        i += 2;
    }
    let mut lanes = [0.0f64; 2];
    _mm_storeu_pd(lanes.as_mut_ptr(), vec_sum);
    let mut sum = acc + lanes[0] + lanes[1];
    for &v in &vals[i..] {
        sum += v;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_f64_simd_x86_64_avx2(vals: &[f64], acc: f64) -> f64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm256_setzero_pd();
    while i + 4 <= vals.len() {
        let vec = _mm256_loadu_pd(vals.as_ptr().add(i));
        vec_sum = _mm256_add_pd(vec_sum, vec);
        i += 4;
    }
    let mut lanes = [0.0f64; 4];
    _mm256_storeu_pd(lanes.as_mut_ptr(), vec_sum);
    let mut sum = acc + lanes[0] + lanes[1] + lanes[2] + lanes[3];
    for &v in &vals[i..] {
        sum += v;
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[allow(dead_code)]
unsafe fn sum_f64_simd_aarch64(vals: &[f64], acc: f64) -> f64 {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vec_sum = vdupq_n_f64(0.0);
        while i + 2 <= vals.len() {
            let vec = vld1q_f64(vals.as_ptr().add(i));
            vec_sum = vaddq_f64(vec_sum, vec);
            i += 2;
        }
        let mut lanes = [0.0f64; 2];
        vst1q_f64(lanes.as_mut_ptr(), vec_sum);
        let mut sum = acc + lanes[0] + lanes[1];
        for &v in &vals[i..] {
            sum += v;
        }
        sum
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn sum_f64_simd_wasm32(vals: &[f64], acc: f64) -> f64 {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        let mut vec_sum = f64x2_splat(0.0);
        while i + 2 <= vals.len() {
            let vec = v128_load(vals.as_ptr().add(i) as *const v128);
            vec_sum = f64x2_add(vec_sum, vec);
            i += 2;
        }
        let mut sum = acc + f64x2_extract_lane::<0>(vec_sum) + f64x2_extract_lane::<1>(vec_sum);
        for &v in &vals[i..] {
            sum += v;
        }
        sum
    }
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

// ---------------------------------------------------------------------------
// SIMD-accelerated sequence element identity comparison
// Batch-compare NaN-boxed u64 arrays to quickly find first mismatch index.
// ---------------------------------------------------------------------------

/// Compare two u64 slices for element-wise bitwise equality using SIMD.
/// Returns the index of the first mismatch, or `len` if all elements match.
/// This is an identity check (bits ==), not semantic equality (obj_eq).
pub(crate) fn simd_find_first_mismatch(lhs: &[u64], rhs: &[u64]) -> usize {
    let len = lhs.len().min(rhs.len());
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { find_first_mismatch_avx2(lhs, rhs, len) };
        }
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { find_first_mismatch_sse2(lhs, rhs, len) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { find_first_mismatch_neon(lhs, rhs, len) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { find_first_mismatch_wasm32(lhs, rhs, len) };
    }
    find_first_mismatch_scalar(lhs, rhs, len)
}

#[cfg(target_arch = "wasm32")]
unsafe fn find_first_mismatch_wasm32(lhs: &[u64], rhs: &[u64], len: usize) -> usize {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        while i + 2 <= len {
            let l_vec = v128_load(lhs.as_ptr().add(i) as *const v128);
            let r_vec = v128_load(rhs.as_ptr().add(i) as *const v128);
            let cmp = u8x16_eq(l_vec, r_vec);
            if u8x16_bitmask(cmp) != 0xFFFF {
                if lhs[i] != rhs[i] {
                    return i;
                }
                return i + 1;
            }
            i += 2;
        }
        for j in i..len {
            if lhs[j] != rhs[j] {
                return j;
            }
        }
        len
    }
}

fn find_first_mismatch_scalar(lhs: &[u64], rhs: &[u64], len: usize) -> usize {
    for i in 0..len {
        if lhs[i] != rhs[i] {
            return i;
        }
    }
    len
}

#[cfg(target_arch = "x86_64")]
unsafe fn find_first_mismatch_sse2(lhs: &[u64], rhs: &[u64], len: usize) -> usize {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    // Process 2 u64s (128 bits) per iteration
    while i + 2 <= len {
        let l_vec = _mm_loadu_si128(lhs.as_ptr().add(i) as *const __m128i);
        let r_vec = _mm_loadu_si128(rhs.as_ptr().add(i) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(l_vec, r_vec);
        let mask = _mm_movemask_epi8(cmp);
        if mask != 0xFFFF {
            // Mismatch in this 128-bit block — find which u64
            if lhs[i] != rhs[i] {
                return i;
            }
            return i + 1;
        }
        i += 2;
    }
    // Remainder
    for j in i..len {
        if lhs[j] != rhs[j] {
            return j;
        }
    }
    len
}

#[cfg(target_arch = "x86_64")]
unsafe fn find_first_mismatch_avx2(lhs: &[u64], rhs: &[u64], len: usize) -> usize {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    // Process 4 u64s (256 bits) per iteration
    while i + 4 <= len {
        let l_vec = _mm256_loadu_si256(lhs.as_ptr().add(i) as *const __m256i);
        let r_vec = _mm256_loadu_si256(rhs.as_ptr().add(i) as *const __m256i);
        let cmp = _mm256_cmpeq_epi64(l_vec, r_vec);
        let mask = _mm256_movemask_epi8(cmp);
        if mask != -1i32 {
            // Mismatch in this 256-bit block — find which u64
            for j in 0..4 {
                if lhs[i + j] != rhs[i + j] {
                    return i + j;
                }
            }
        }
        i += 4;
    }
    // Remainder with SSE2
    while i + 2 <= len {
        let l_vec = _mm_loadu_si128(lhs.as_ptr().add(i) as *const __m128i);
        let r_vec = _mm_loadu_si128(rhs.as_ptr().add(i) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(l_vec, r_vec);
        let mask = _mm_movemask_epi8(cmp);
        if mask != 0xFFFF {
            if lhs[i] != rhs[i] {
                return i;
            }
            return i + 1;
        }
        i += 2;
    }
    for j in i..len {
        if lhs[j] != rhs[j] {
            return j;
        }
    }
    len
}

#[cfg(target_arch = "aarch64")]
unsafe fn find_first_mismatch_neon(lhs: &[u64], rhs: &[u64], len: usize) -> usize {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        // Process 2 u64s (128 bits) per iteration
        while i + 2 <= len {
            let l_vec = vld1q_u64(lhs.as_ptr().add(i));
            let r_vec = vld1q_u64(rhs.as_ptr().add(i));
            let cmp = vceqq_u64(l_vec, r_vec);
            // Both lanes must be all-ones (0xFFFFFFFFFFFFFFFF) for equality
            let lane0 = vgetq_lane_u64(cmp, 0);
            let lane1 = vgetq_lane_u64(cmp, 1);
            if lane0 != u64::MAX {
                return i;
            }
            if lane1 != u64::MAX {
                return i + 1;
            }
            i += 2;
        }
        for j in i..len {
            if lhs[j] != rhs[j] {
                return j;
            }
        }
        len
    }
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
    let hit = hits.load(AtomicOrdering::Relaxed);
    let miss = misses.load(AtomicOrdering::Relaxed);
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
        hits.fetch_add(1, AtomicOrdering::Relaxed);
    } else {
        misses.fetch_add(1, AtomicOrdering::Relaxed);
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm_setzero_si128();
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let vec = _mm_set_epi64x(v1, v0);
        vec_sum = _mm_add_epi64(vec_sum, vec);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_sum);
    let mut sum = acc + lanes[0] + lanes[1];
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        sum += val;
    }
    Some(sum)
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm256_setzero_si256();
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let v2 = obj2.as_int()?;
        let v3 = obj3.as_int()?;
        let vec = _mm256_set_epi64x(v3, v2, v1, v0);
        vec_sum = _mm256_add_epi64(vec_sum, vec);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_sum);
    let mut sum = acc + lanes.iter().sum::<i64>();
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        sum += val;
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

#[cfg(target_arch = "wasm32")]
unsafe fn sum_ints_simd_wasm32(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        let mut vec_sum = i64x2_splat(0);
        while i + 2 <= elems.len() {
            let obj0 = MoltObject::from_bits(elems[i]);
            let obj1 = MoltObject::from_bits(elems[i + 1]);
            let v0 = obj0.as_int()?;
            let v1 = obj1.as_int()?;
            let arr = [v0, v1];
            let vec = v128_load(arr.as_ptr() as *const v128);
            vec_sum = i64x2_add(vec_sum, vec);
            i += 2;
        }
        let mut sum = acc + i64x2_extract_lane::<0>(vec_sum) + i64x2_extract_lane::<1>(vec_sum);
        for &bits in &elems[i..] {
            let obj = MoltObject::from_bits(bits);
            let val = obj.as_int()?;
            sum += val;
        }
        Some(sum)
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

fn prod_ints_unboxed_trivial(_elems: &[i64]) -> Option<i64> {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { prod_ints_unboxed_avx2_trivial(_elems) };
        }
    }
    None
}

#[cfg(target_arch = "x86_64")]
unsafe fn prod_ints_unboxed_avx2_trivial(elems: &[i64]) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut idx = 0usize;
    let ones = _mm256_set1_epi64x(1);
    let zeros = _mm256_setzero_si256();
    let mut all_ones = true;
    while idx + 4 <= elems.len() {
        let vec = _mm256_loadu_si256(elems.as_ptr().add(idx) as *const __m256i);
        let eq_zero = _mm256_cmpeq_epi64(vec, zeros);
        if _mm256_movemask_epi8(eq_zero) != 0 {
            return Some(0);
        }
        if all_ones {
            let eq_one = _mm256_cmpeq_epi64(vec, ones);
            if _mm256_movemask_epi8(eq_one) != -1 {
                all_ones = false;
            }
        }
        idx += 4;
    }
    for &val in &elems[idx..] {
        if val == 0 {
            return Some(0);
        }
        if val != 1 {
            all_ones = false;
        }
    }
    if all_ones {
        return Some(1);
    }
    None
}

#[cfg(target_arch = "aarch64")]
unsafe fn prod_ints_simd_aarch64(elems: &[u64], acc: i64) -> Option<i64> {
    prod_ints_scalar(elems, acc)
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

#[cfg(target_arch = "x86_64")]
unsafe fn min_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_min = _mm_set1_epi64x(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
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
        let val = obj.as_int()?;
        if val < min_val {
            min_val = val;
        }
    }
    Some(min_val)
}

#[cfg(target_arch = "x86_64")]
unsafe fn min_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_min = _mm256_set1_epi64x(acc);
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let v2 = obj2.as_int()?;
        let v3 = obj3.as_int()?;
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
        let val = obj.as_int()?;
        if val < min_val {
            min_val = val;
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

#[cfg(target_arch = "wasm32")]
unsafe fn min_ints_simd_wasm32(elems: &[u64], acc: i64) -> Option<i64> {
    let mut min_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        if val < min_val {
            min_val = val;
        }
    }
    Some(min_val)
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

#[cfg(target_arch = "x86_64")]
unsafe fn max_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_max = _mm_set1_epi64x(acc);
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
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
        let val = obj.as_int()?;
        if val > max_val {
            max_val = val;
        }
    }
    Some(max_val)
}

#[cfg(target_arch = "x86_64")]
unsafe fn max_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_max = _mm256_set1_epi64x(acc);
    while i + 4 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let obj2 = MoltObject::from_bits(elems[i + 2]);
        let obj3 = MoltObject::from_bits(elems[i + 3]);
        let v0 = obj0.as_int()?;
        let v1 = obj1.as_int()?;
        let v2 = obj2.as_int()?;
        let v3 = obj3.as_int()?;
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
        let val = obj.as_int()?;
        if val > max_val {
            max_val = val;
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

#[cfg(target_arch = "wasm32")]
unsafe fn max_ints_simd_wasm32(elems: &[u64], acc: i64) -> Option<i64> {
    let mut max_val = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        let val = obj.as_int()?;
        if val > max_val {
            max_val = val;
        }
    }
    Some(max_val)
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

fn sum_ints_trusted_scalar(elems: &[u64], acc: i64) -> i64 {
    let mut sum = acc;
    for &bits in elems {
        let obj = MoltObject::from_bits(bits);
        sum += obj.as_int_unchecked();
    }
    sum
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_ints_trusted_simd_x86_64(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm_setzero_si128();
    while i + 2 <= elems.len() {
        let obj0 = MoltObject::from_bits(elems[i]);
        let obj1 = MoltObject::from_bits(elems[i + 1]);
        let v0 = obj0.as_int_unchecked();
        let v1 = obj1.as_int_unchecked();
        let vec = _mm_set_epi64x(v1, v0);
        vec_sum = _mm_add_epi64(vec_sum, vec);
        i += 2;
    }
    let mut lanes = [0i64; 2];
    _mm_storeu_si128(lanes.as_mut_ptr() as *mut __m128i, vec_sum);
    let mut sum = acc + lanes[0] + lanes[1];
    for &bits in &elems[i..] {
        let obj = MoltObject::from_bits(bits);
        sum += obj.as_int_unchecked();
    }
    sum
}

#[cfg(target_arch = "x86_64")]
unsafe fn sum_ints_trusted_simd_x86_64_avx2(elems: &[u64], acc: i64) -> i64 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vec_sum = _mm256_setzero_si256();
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
        vec_sum = _mm256_add_epi64(vec_sum, vec);
        i += 4;
    }
    let mut lanes = [0i64; 4];
    _mm256_storeu_si256(lanes.as_mut_ptr() as *mut __m256i, vec_sum);
    let mut sum = acc + lanes.iter().sum::<i64>();
    for &bits in &elems[i..] {
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

#[cfg(target_arch = "wasm32")]
unsafe fn sum_ints_trusted_simd_wasm32(elems: &[u64], acc: i64) -> i64 {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        let mut vec_sum = i64x2_splat(0);
        while i + 2 <= elems.len() {
            let v0 = MoltObject::from_bits(elems[i]).as_int_unchecked();
            let v1 = MoltObject::from_bits(elems[i + 1]).as_int_unchecked();
            let arr = [v0, v1];
            let vec = v128_load(arr.as_ptr() as *const v128);
            vec_sum = i64x2_add(vec_sum, vec);
            i += 2;
        }
        let mut sum = acc + i64x2_extract_lane::<0>(vec_sum) + i64x2_extract_lane::<1>(vec_sum);
        for &bits in &elems[i..] {
            sum += MoltObject::from_bits(bits).as_int_unchecked();
        }
        sum
    }
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

pub(crate) enum SliceError {
    Type,
    Value,
}

pub(crate) fn slice_error(_py: &PyToken<'_>, err: SliceError) -> u64 {
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    match err {
        SliceError::Type => raise_exception::<_>(
            _py,
            "TypeError",
            "slice indices must be integers or None or have an __index__ method",
        ),
        SliceError::Value => raise_exception::<_>(_py, "ValueError", "slice step cannot be zero"),
    }
}

pub(crate) fn decode_slice_bound(
    _py: &PyToken<'_>,
    obj: MoltObject,
    len: isize,
    default: isize,
) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(default);
    }
    let msg = "slice indices must be integers or None or have an __index__ method";
    let Some(mut idx) = index_bigint_from_obj(_py, obj.bits(), msg) else {
        return Err(SliceError::Type);
    };
    let len_big = BigInt::from(len);
    if idx.is_negative() {
        idx += &len_big;
    }
    if idx < BigInt::zero() {
        return Ok(0);
    }
    if idx > len_big {
        return Ok(len);
    }
    Ok(idx.to_isize().unwrap_or(len))
}

fn decode_slice_bound_neg(
    _py: &PyToken<'_>,
    obj: MoltObject,
    len: isize,
    default: isize,
) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(default);
    }
    let msg = "slice indices must be integers or None or have an __index__ method";
    let Some(mut idx) = index_bigint_from_obj(_py, obj.bits(), msg) else {
        return Err(SliceError::Type);
    };
    let len_big = BigInt::from(len);
    if idx.is_negative() {
        idx += &len_big;
    }
    let neg_one = BigInt::from(-1);
    if idx < neg_one {
        return Ok(-1);
    }
    if idx >= len_big {
        return Ok(len - 1);
    }
    Ok(idx.to_isize().unwrap_or(len - 1))
}

fn decode_slice_step(_py: &PyToken<'_>, obj: MoltObject) -> Result<isize, SliceError> {
    if obj.is_none() {
        return Ok(1);
    }
    let msg = "slice indices must be integers or None or have an __index__ method";
    let Some(step) = index_bigint_from_obj(_py, obj.bits(), msg) else {
        return Err(SliceError::Type);
    };
    if step.is_zero() {
        return Err(SliceError::Value);
    }
    if let Some(step) = step.to_i64() {
        return Ok(step as isize);
    }
    if step.is_negative() {
        return Ok(-(i64::MAX as isize));
    }
    Ok(i64::MAX as isize)
}

pub(crate) fn normalize_slice_indices(
    _py: &PyToken<'_>,
    len: isize,
    start_obj: MoltObject,
    stop_obj: MoltObject,
    step_obj: MoltObject,
) -> Result<(isize, isize, isize), SliceError> {
    let step = decode_slice_step(_py, step_obj)?;
    if step > 0 {
        let start = decode_slice_bound(_py, start_obj, len, 0)?;
        let stop = decode_slice_bound(_py, stop_obj, len, len)?;
        return Ok((start, stop, step));
    }
    let start_default = if len == 0 { -1 } else { len - 1 };
    let stop_default = -1;
    let start = decode_slice_bound_neg(_py, start_obj, len, start_default)?;
    let stop = decode_slice_bound_neg(_py, stop_obj, len, stop_default)?;
    Ok((start, stop, step))
}

pub(crate) fn collect_slice_indices(start: isize, stop: isize, step: isize) -> Vec<usize> {
    let mut out = Vec::new();
    if step > 0 {
        let mut i = start;
        while i < stop {
            out.push(i as usize);
            let Some(next) = i.checked_add(step) else {
                break;
            };
            i = next;
        }
    } else {
        let mut i = start;
        while i > stop {
            out.push(i as usize);
            let Some(next) = i.checked_add(step) else {
                break;
            };
            i = next;
        }
    }
    out
}

pub(crate) fn collect_iterable_values(_py: &PyToken<'_>, bits: u64, err_msg: &str) -> Option<Vec<u64>> {
    let iter_bits = molt_iter(bits);
    if obj_from_bits(iter_bits).is_none() {
        if exception_pending(_py) {
            return None;
        }
        return raise_exception::<_>(_py, "TypeError", err_msg);
    }
    let mut out = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        if exception_pending(_py) {
            return None;
        }
        let pair_ptr = obj_from_bits(pair_bits).as_ptr()?;
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return None;
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                return None;
            }
            let done_bits = elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            out.push(elems[0]);
        }
    }
    Some(out)
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_len(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let count = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                    return MoltObject::from_int(count).bits();
                }
                if type_id == TYPE_ID_BYTES {
                    return MoltObject::from_int(bytes_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    return MoltObject::from_int(bytes_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    if memoryview_ndim(ptr) == 0 {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "0-dim memory has no length",
                        );
                    }
                    return MoltObject::from_int(memoryview_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_LIST {
                    return MoltObject::from_int(list_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_TUPLE {
                    return MoltObject::from_int(tuple_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_INTARRAY {
                    return MoltObject::from_int(intarray_len(ptr) as i64).bits();
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    return MoltObject::from_int(dict_len(dict_ptr) as i64).bits();
                }
                if type_id == TYPE_ID_SET {
                    return MoltObject::from_int(set_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_FROZENSET {
                    return MoltObject::from_int(set_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_DICT_KEYS_VIEW
                    || type_id == TYPE_ID_DICT_VALUES_VIEW
                    || type_id == TYPE_ID_DICT_ITEMS_VIEW
                {
                    return MoltObject::from_int(dict_view_len(ptr) as i64).bits();
                }
                if type_id == TYPE_ID_RANGE {
                    let Some((start, stop, step)) = range_components_bigint(ptr) else {
                        return MoltObject::none().bits();
                    };
                    let len = range_len_bigint(&start, &stop, &step);
                    return int_bits_from_bigint(_py, len);
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__len__") {
                    let call_bits = attr_lookup_ptr(_py, ptr, name_bits);
                    dec_ref_bits(_py, name_bits);
                    if let Some(call_bits) = call_bits {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(i) = to_i64(res_obj) {
                            if i < 0 {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "__len__() should return >= 0",
                                );
                            }
                            return MoltObject::from_int(i).bits();
                        }
                        if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                            let big = bigint_ref(big_ptr);
                            if big.is_negative() {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "__len__() should return >= 0",
                                );
                            }
                            let Some(len) = big.to_usize() else {
                                return raise_exception::<_>(
                                    _py,
                                    "OverflowError",
                                    "cannot fit 'int' into an index-sized integer",
                                );
                            };
                            if len > i64::MAX as usize {
                                return raise_exception::<_>(
                                    _py,
                                    "OverflowError",
                                    "cannot fit 'int' into an index-sized integer",
                                );
                            }
                            return MoltObject::from_int(len as i64).bits();
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        let msg =
                            format!("'{}' object cannot be interpreted as an integer", res_type);
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("object of type '{type_name}' has no len()");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hash_builtin(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hash = hash_bits_signed(_py, val);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        int_bits_from_i64(_py, hash)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_hash(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        let hash = if let Some(ptr) = obj.as_ptr() {
            hash_pointer(ptr as u64)
        } else {
            hash_pointer(val)
        };
        int_bits_from_i64(_py, hash)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_id(val: u64) -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, val as i64) })
}

fn ord_length_error(_py: &PyToken<'_>, len: usize) -> u64 {
    let msg = format!("ord() expected a character, but string of length {len} found");
    raise_exception::<_>(_py, "TypeError", &msg)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ord(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let char_count = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                    if char_count != 1 {
                        return ord_length_error(_py, char_count as usize);
                    }
                    let Some(code) = wtf8_codepoint_at(bytes, 0) else {
                        return MoltObject::none().bits();
                    };
                    return MoltObject::from_int(code as i64).bits();
                }
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    if len != 1 {
                        return ord_length_error(_py, len);
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    return MoltObject::from_int(bytes[0] as i64).bits();
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("ord() expected string of length 1, but {type_name} found");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_chr(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, val, &msg) else {
            return MoltObject::none().bits();
        };
        if value.is_negative() || value > BigInt::from(0x10FFFF) {
            return raise_exception::<_>(_py, "ValueError", "chr() arg not in range(0x110000)");
        }
        let Some(code) = value.to_u32() else {
            return raise_exception::<_>(_py, "ValueError", "chr() arg not in range(0x110000)");
        };
        let mut out_bytes = Vec::with_capacity(4);
        push_wtf8_codepoint(&mut out_bytes, code);
        let out = alloc_string(_py, &out_bytes);
        if out.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_missing() -> u64 {
    crate::with_gil_entry!(_py, {
        let bits = missing_bits(_py);
        inc_ref_bits(_py, bits);
        bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_not_implemented() -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ellipsis() -> u64 {
    crate::with_gil_entry!(_py, { ellipsis_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pending() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::pending().bits() })
}

#[derive(Clone, Copy)]
struct GcState {
    enabled: bool,
    thresholds: (i64, i64, i64),
    debug_flags: i64,
    count: (i64, i64, i64),
}

fn gc_state() -> &'static Mutex<GcState> {
    static GC_STATE: OnceLock<Mutex<GcState>> = OnceLock::new();
    GC_STATE.get_or_init(|| {
        Mutex::new(GcState {
            enabled: true,
            thresholds: (0, 0, 0),
            debug_flags: 0,
            count: (0, 0, 0),
        })
    })
}

fn gc_int_arg(_py: &PyToken<'_>, bits: u64, label: &str) -> Result<i64, u64> {
    if let Some(value) = to_i64(obj_from_bits(bits)) {
        return Ok(value);
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(bits) {
        let big = unsafe { bigint_ref(big_ptr) };
        let Some(value) = big.to_i64() else {
            let msg = format!("{label} value out of range");
            return Err(raise_exception::<_>(_py, "OverflowError", &msg));
        };
        return Ok(value);
    }
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("'{type_name}' object cannot be interpreted as an integer");
    Err(raise_exception::<_>(_py, "TypeError", &msg))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_collect(generation_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let generation = match gc_int_arg(_py, generation_bits, "generation") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if generation < 0 {
            return raise_exception::<_>(_py, "ValueError", "generation must be non-negative");
        }
        let collected = crate::object::weakref::weakref_collect_for_gc(_py) as i64;
        let mut state = gc_state().lock().unwrap();
        state.count = (0, 0, 0);
        MoltObject::from_int(collected).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_enable() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut state = gc_state().lock().unwrap();
        state.enabled = true;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_disable() -> u64 {
    crate::with_gil_entry!(_py, {
        let mut state = gc_state().lock().unwrap();
        state.enabled = false;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_isenabled() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = gc_state().lock().unwrap();
        MoltObject::from_bool(state.enabled).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_set_threshold(th0_bits: u64, th1_bits: u64, th2_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let th0 = match gc_int_arg(_py, th0_bits, "threshold0") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let th1 = match gc_int_arg(_py, th1_bits, "threshold1") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let th2 = match gc_int_arg(_py, th2_bits, "threshold2") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut state = gc_state().lock().unwrap();
        state.thresholds = (th0, th1, th2);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_get_threshold() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = gc_state().lock().unwrap();
        let (th0, th1, th2) = state.thresholds;
        let th0_bits = MoltObject::from_int(th0).bits();
        let th1_bits = MoltObject::from_int(th1).bits();
        let th2_bits = MoltObject::from_int(th2).bits();
        let tuple_ptr = alloc_tuple(_py, &[th0_bits, th1_bits, th2_bits]);
        dec_ref_bits(_py, th0_bits);
        dec_ref_bits(_py, th1_bits);
        dec_ref_bits(_py, th2_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_set_debug(flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let flags = match gc_int_arg(_py, flags_bits, "flags") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut state = gc_state().lock().unwrap();
        state.debug_flags = flags;
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_get_debug() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = gc_state().lock().unwrap();
        MoltObject::from_int(state.debug_flags).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gc_get_count() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = gc_state().lock().unwrap();
        let (c0, c1, c2) = state.count;
        let c0_bits = MoltObject::from_int(c0).bits();
        let c1_bits = MoltObject::from_int(c1).bits();
        let c2_bits = MoltObject::from_int(c2).bits();
        let tuple_ptr = alloc_tuple(_py, &[c0_bits, c1_bits, c2_bits]);
        dec_ref_bits(_py, c0_bits);
        dec_ref_bits(_py, c1_bits);
        dec_ref_bits(_py, c2_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getrecursionlimit() -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_int(recursion_limit_get() as i64).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_setrecursionlimit(limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(limit_bits);
        let limit = if let Some(value) = to_i64(obj) {
            if value < 1 {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "recursion limit must be greater or equal than 1",
                );
            }
            value as usize
        } else if let Some(big_ptr) = bigint_ptr_from_bits(limit_bits) {
            let big = unsafe { bigint_ref(big_ptr) };
            if big.is_negative() {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "recursion limit must be greater or equal than 1",
                );
            }
            let Some(value) = big.to_usize() else {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            };
            value
        } else {
            let type_name = class_name_for_error(type_of_bits(_py, limit_bits));
            let msg = format!("'{type_name}' object cannot be interpreted as an integer");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let depth = RECURSION_DEPTH.with(|depth| depth.get());
        if limit <= depth {
            let msg = format!(
                "cannot set the recursion limit to {limit} at the recursion depth {depth}: the limit is too low"
            );
            return raise_exception::<_>(_py, "RecursionError", &msg);
        }
        recursion_limit_set(limit);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getargv() -> u64 {
    crate::with_gil_entry!(_py, {
        let args_guard = runtime_state(_py).argv.lock().unwrap();
        // On WASM, molt_set_argv may not have been called (no C main stub).
        // Fall back to std::env::args() so WASI args are still visible.
        let env_args_storage;
        let args: &Vec<Vec<u8>> = if args_guard.is_empty() {
            env_args_storage = std::env::args()
                .map(|s| s.into_bytes())
                .collect::<Vec<_>>();
            &env_args_storage
        } else {
            &args_guard
        };
        let mut elems = Vec::with_capacity(args.len());
        for arg in args.iter() {
            let ptr = alloc_string(_py, arg);
            if ptr.is_null() {
                for bits in elems {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            elems.push(MoltObject::from_ptr(ptr).bits());
        }
        let list_ptr = alloc_list(_py, &elems);
        if list_ptr.is_null() {
            for bits in elems {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        for bits in elems {
            dec_ref_bits(_py, bits);
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

fn trace_sys_version() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var("MOLT_TRACE_SYS_VERSION").as_deref() == Ok("1"))
}

fn env_sys_version_info() -> Option<PythonVersionInfo> {
    let raw = std::env::var("MOLT_SYS_VERSION_INFO").ok()?;
    if trace_sys_version() {
        eprintln!("molt sys version: env raw={raw}");
    }
    let mut parts = raw.split(',');
    let major = parts.next()?.trim().parse::<i64>().ok()?;
    let minor = parts.next()?.trim().parse::<i64>().ok()?;
    let micro = parts.next()?.trim().parse::<i64>().ok()?;
    let releaselevel = parts.next()?.trim().to_string();
    let serial = parts.next()?.trim().parse::<i64>().ok()?;
    if major < 0 || minor < 0 || micro < 0 || serial < 0 {
        return None;
    }
    if releaselevel.is_empty() {
        return None;
    }
    let info = PythonVersionInfo {
        major,
        minor,
        micro,
        releaselevel,
        serial,
    };
    if trace_sys_version() {
        eprintln!(
            "molt sys version: parsed {}.{}.{} {} {}",
            info.major, info.minor, info.micro, info.releaselevel, info.serial
        );
    }
    Some(info)
}

fn default_sys_version_info() -> PythonVersionInfo {
    env_sys_version_info().unwrap_or_else(|| PythonVersionInfo {
        major: 3,
        minor: 12,
        micro: 0,
        releaselevel: "final".to_string(),
        serial: 0,
    })
}

fn format_sys_version(info: &PythonVersionInfo) -> String {
    let base = format!("{}.{}.{}", info.major, info.minor, info.micro);
    let suffix = match info.releaselevel.as_str() {
        "alpha" => format!("a{}", info.serial),
        "beta" => format!("b{}", info.serial),
        "candidate" => format!("rc{}", info.serial),
        "final" => String::new(),
        other => format!("{other}{}", info.serial),
    };
    if suffix.is_empty() {
        format!("{base} (molt)")
    } else {
        format!("{base}{suffix} (molt)")
    }
}

const DEFAULT_SYS_API_VERSION: i64 = 1013;
const SYS_HEX_RELEASELEVEL_ALPHA: i64 = 0xA;
const SYS_HEX_RELEASELEVEL_BETA: i64 = 0xB;
const SYS_HEX_RELEASELEVEL_CANDIDATE: i64 = 0xC;
const SYS_HEX_RELEASELEVEL_FINAL: i64 = 0xF;

fn releaselevel_hex_nibble(releaselevel: &str) -> i64 {
    match releaselevel {
        "alpha" => SYS_HEX_RELEASELEVEL_ALPHA,
        "beta" => SYS_HEX_RELEASELEVEL_BETA,
        "candidate" | "rc" => SYS_HEX_RELEASELEVEL_CANDIDATE,
        "final" => SYS_HEX_RELEASELEVEL_FINAL,
        _ => SYS_HEX_RELEASELEVEL_FINAL,
    }
}

fn sys_hexversion_from_info(info: &PythonVersionInfo) -> i64 {
    let major = (info.major & 0xFF) << 24;
    let minor = (info.minor & 0xFF) << 16;
    let micro = (info.micro & 0xFF) << 8;
    let releaselevel = releaselevel_hex_nibble(&info.releaselevel) << 4;
    let serial = info.serial & 0xF;
    major | minor | micro | releaselevel | serial
}

fn sys_api_version() -> i64 {
    std::env::var("MOLT_SYS_API_VERSION")
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .unwrap_or(DEFAULT_SYS_API_VERSION)
}

fn sys_abiflags() -> String {
    std::env::var("MOLT_SYS_ABIFLAGS").unwrap_or_default()
}

fn sys_implementation_name() -> String {
    match std::env::var("MOLT_SYS_IMPLEMENTATION_NAME") {
        Ok(raw) if !raw.trim().is_empty() => raw,
        _ => "molt".to_string(),
    }
}

fn sys_cache_tag(name: &str, info: &PythonVersionInfo) -> String {
    match std::env::var("MOLT_SYS_CACHE_TAG") {
        Ok(raw) if !raw.is_empty() => raw,
        _ => format!("{name}-{}{}", info.major, info.minor),
    }
}

const DEFAULT_SYS_FLAGS_INT_MAX_STR_DIGITS: i64 = 0;

fn env_flag_level(var: &str) -> Option<i64> {
    let raw = std::env::var(var).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Some(1);
    }
    match trimmed.parse::<i64>() {
        Ok(value) if value > 0 => Some(value),
        Ok(_) => Some(0),
        Err(_) => Some(1),
    }
}

fn env_flag_bool(var: &str) -> Option<i64> {
    env_flag_level(var).map(|value| if value == 0 { 0 } else { 1 })
}

fn env_non_negative_i64(var: &str) -> Option<i64> {
    std::env::var(var)
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|value| *value >= 0)
}

fn sys_flags_hash_randomization() -> i64 {
    match std::env::var("PYTHONHASHSEED") {
        Ok(value) => {
            if value == "random" {
                return 1;
            }
            let seed: u32 = value.parse().unwrap_or_else(|_| fatal_hash_seed(&value));
            if seed == 0 { 0 } else { 1 }
        }
        Err(_) => 1,
    }
}

fn current_sys_version_info(state: &RuntimeState) -> (PythonVersionInfo, bool) {
    let mut guard = state.sys_version_info.lock().unwrap();
    if let Some(existing) = guard.as_ref() {
        (existing.clone(), false)
    } else {
        let init = default_sys_version_info();
        *guard = Some(init.clone());
        (init, true)
    }
}

fn alloc_sys_version_info_tuple(_py: &PyToken<'_>, info: &PythonVersionInfo) -> Option<u64> {
    let release_ptr = alloc_string(_py, info.releaselevel.as_bytes());
    if release_ptr.is_null() {
        return None;
    }
    let release_bits = MoltObject::from_ptr(release_ptr).bits();
    let elems = [
        MoltObject::from_int(info.major).bits(),
        MoltObject::from_int(info.minor).bits(),
        MoltObject::from_int(info.micro).bits(),
        release_bits,
        MoltObject::from_int(info.serial).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        dec_ref_bits(_py, release_bits);
        return None;
    }
    for bits in elems {
        dec_ref_bits(_py, bits);
    }
    Some(MoltObject::from_ptr(tuple_ptr).bits())
}

fn dict_set_bytes_key(_py: &PyToken<'_>, dict_ptr: *mut u8, key: &[u8], value_bits: u64) -> bool {
    let key_ptr = alloc_string(_py, key);
    if key_ptr.is_null() {
        return false;
    }
    let key_bits = MoltObject::from_ptr(key_ptr).bits();
    unsafe {
        dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
    }
    dec_ref_bits(_py, key_bits);
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_set_version_info(
    major_bits: u64,
    minor_bits: u64,
    micro_bits: u64,
    releaselevel_bits: u64,
    serial_bits: u64,
    version_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let major = index_i64_from_obj(_py, major_bits, "major must be int");
        let minor = index_i64_from_obj(_py, minor_bits, "minor must be int");
        let micro = index_i64_from_obj(_py, micro_bits, "micro must be int");
        let serial = index_i64_from_obj(_py, serial_bits, "serial must be int");
        if major < 0 || minor < 0 || micro < 0 || serial < 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "sys.version_info must be non-negative integers",
            );
        }

        let Some(release_ptr) = obj_from_bits(releaselevel_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "sys.version_info releaselevel must be str",
            );
        };
        unsafe {
            if object_type_id(release_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "sys.version_info releaselevel must be str",
                );
            }
        }
        let release_bytes = unsafe {
            std::slice::from_raw_parts(string_bytes(release_ptr), string_len(release_ptr))
        };
        let releaselevel = String::from_utf8_lossy(release_bytes).into_owned();

        let Some(version_ptr) = obj_from_bits(version_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "sys.version must be str");
        };
        unsafe {
            if object_type_id(version_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "sys.version must be str");
            }
        }
        let version_bytes = unsafe {
            std::slice::from_raw_parts(string_bytes(version_ptr), string_len(version_ptr))
        };
        let mut version = String::from_utf8_lossy(version_bytes).into_owned();

        let mut info = PythonVersionInfo {
            major,
            minor,
            micro,
            releaselevel,
            serial,
        };
        let mut info_overridden_from_env = false;
        if let Some(env_info) = env_sys_version_info() {
            if env_info != info {
                info_overridden_from_env = true;
                if trace_sys_version() {
                    eprintln!(
                        "molt sys version: overriding set payload with env {}.{}.{} {} {}",
                        env_info.major,
                        env_info.minor,
                        env_info.micro,
                        env_info.releaselevel,
                        env_info.serial
                    );
                }
            }
            info = env_info;
        }

        let mut version_from_env = false;
        if let Ok(env_version) = std::env::var("MOLT_SYS_VERSION")
            && !env_version.is_empty()
        {
            version = env_version;
            version_from_env = true;
        }
        if !version_from_env && (version.is_empty() || info_overridden_from_env) {
            version = format_sys_version(&info);
        }
        if trace_sys_version() {
            eprintln!(
                "molt sys version: set called {}.{}.{} {} {}",
                info.major, info.minor, info.micro, info.releaselevel, info.serial
            );
        }

        let state = runtime_state(_py);
        let default_info = default_sys_version_info();
        {
            let mut guard = state.sys_version_info.lock().unwrap();
            if let Some(existing) = guard.as_ref()
                && existing != &info
                && existing != &default_info
            {
                return raise_exception::<_>(_py, "RuntimeError", "sys.version_info already set");
            }
            *guard = Some(info.clone());
        }
        {
            let mut guard = state.sys_version.lock().unwrap();
            if let Some(existing) = guard.as_ref()
                && existing != &version
            {
                return raise_exception::<_>(_py, "RuntimeError", "sys.version already set");
            }
            *guard = Some(version.clone());
        }
        // If the sys module already exists, keep its version metadata in sync.
        let sys_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            cache.lock().unwrap().get("sys").copied()
        };
        if trace_sys_version() {
            eprintln!("molt sys version: sys module cached={}", sys_bits.is_some());
        }
        if let Some(bits) = sys_bits
            && let Some(sys_ptr) = obj_from_bits(bits).as_ptr()
        {
            unsafe {
                let dict_bits = module_dict_bits(sys_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
                    let version_info_bits = molt_sys_version_info();
                    let version_bits = molt_sys_version();
                    let hexversion_bits = molt_sys_hexversion();
                    let api_version_bits = molt_sys_api_version();
                    let abiflags_bits = molt_sys_abiflags();
                    let implementation_bits = molt_sys_implementation_payload();
                    let version_info_key = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.sys_version_info,
                        b"version_info",
                    );
                    let version_key = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.sys_version,
                        b"version",
                    );
                    dict_set_in_place(_py, dict_ptr, version_info_key, version_info_bits);
                    dict_set_in_place(_py, dict_ptr, version_key, version_bits);
                    let wrote_hexversion =
                        dict_set_bytes_key(_py, dict_ptr, b"hexversion", hexversion_bits);
                    let wrote_api_version =
                        dict_set_bytes_key(_py, dict_ptr, b"api_version", api_version_bits);
                    let wrote_abiflags =
                        dict_set_bytes_key(_py, dict_ptr, b"abiflags", abiflags_bits);
                    let wrote_implementation =
                        dict_set_bytes_key(_py, dict_ptr, b"implementation", implementation_bits);
                    dec_ref_bits(_py, version_info_key);
                    dec_ref_bits(_py, version_key);
                    dec_ref_bits(_py, version_info_bits);
                    dec_ref_bits(_py, version_bits);
                    dec_ref_bits(_py, hexversion_bits);
                    dec_ref_bits(_py, api_version_bits);
                    dec_ref_bits(_py, abiflags_bits);
                    dec_ref_bits(_py, implementation_bits);
                    if !(wrote_hexversion
                        && wrote_api_version
                        && wrote_abiflags
                        && wrote_implementation)
                    {
                        return MoltObject::none().bits();
                    }
                    if trace_sys_version() {
                        eprintln!("molt sys version: sys dict updated");
                    }
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_version_info() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = runtime_state(_py);
        let (info, initialized) = current_sys_version_info(state);
        if trace_sys_version() {
            eprintln!(
                "molt sys version: get info {}.{}.{} {} {} init={}",
                info.major, info.minor, info.micro, info.releaselevel, info.serial, initialized
            );
        }
        alloc_sys_version_info_tuple(_py, &info).unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_version() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = runtime_state(_py);
        let (info, _) = current_sys_version_info(state);
        let version = {
            let mut guard = state.sys_version.lock().unwrap();
            if let Some(existing) = guard.as_ref() {
                existing.clone()
            } else {
                let computed = format_sys_version(&info);
                *guard = Some(computed.clone());
                computed
            }
        };
        let ptr = alloc_string(_py, version.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_hexversion() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = runtime_state(_py);
        let (info, _) = current_sys_version_info(state);
        MoltObject::from_int(sys_hexversion_from_info(&info)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_api_version() -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_int(sys_api_version()).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_abiflags() -> u64 {
    crate::with_gil_entry!(_py, {
        let abiflags = sys_abiflags();
        let ptr = alloc_string(_py, abiflags.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_implementation_payload() -> u64 {
    crate::with_gil_entry!(_py, {
        let state = runtime_state(_py);
        let (info, _) = current_sys_version_info(state);
        let name = sys_implementation_name();
        let cache_tag = sys_cache_tag(&name, &info);
        let hexversion_bits = MoltObject::from_int(sys_hexversion_from_info(&info)).bits();

        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let cache_tag_ptr = alloc_string(_py, cache_tag.as_bytes());
        if cache_tag_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
            return MoltObject::none().bits();
        }

        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let cache_tag_bits = MoltObject::from_ptr(cache_tag_ptr).bits();
        let Some(version_bits) = alloc_sys_version_info_tuple(_py, &info) else {
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, cache_tag_bits);
            return MoltObject::none().bits();
        };

        let keys_and_values: [(&[u8], u64); 4] = [
            (b"name", name_bits),
            (b"cache_tag", cache_tag_bits),
            (b"version", version_bits),
            (b"hexversion", hexversion_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = vec![name_bits, cache_tag_bits, version_bits, hexversion_bits];

        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_flags_payload() -> u64 {
    crate::with_gil_entry!(_py, {
        let keys_and_values: [(&[u8], i64); 19] = [
            (b"debug", env_flag_bool("PYTHONDEBUG").unwrap_or(0)),
            (b"inspect", env_flag_bool("PYTHONINSPECT").unwrap_or(0)),
            (b"interactive", 0),
            (b"optimize", env_flag_level("PYTHONOPTIMIZE").unwrap_or(0)),
            (
                b"dont_write_bytecode",
                env_flag_bool("PYTHONDONTWRITEBYTECODE").unwrap_or(0),
            ),
            (
                b"no_user_site",
                env_flag_bool("PYTHONNOUSERSITE").unwrap_or(0),
            ),
            (b"no_site", 0),
            (b"ignore_environment", 0),
            (b"verbose", env_flag_level("PYTHONVERBOSE").unwrap_or(0)),
            (b"bytes_warning", 0),
            (b"quiet", 0),
            (b"hash_randomization", sys_flags_hash_randomization()),
            (b"isolated", 0),
            (b"dev_mode", env_flag_bool("PYTHONDEVMODE").unwrap_or(0)),
            (b"utf8_mode", env_flag_bool("PYTHONUTF8").unwrap_or(0)),
            (
                b"warn_default_encoding",
                env_flag_bool("PYTHONWARNDEFAULTENCODING").unwrap_or(0),
            ),
            (b"safe_path", env_flag_bool("PYTHONSAFEPATH").unwrap_or(0)),
            (
                b"int_max_str_digits",
                env_non_negative_i64("PYTHONINTMAXSTRDIGITS")
                    .unwrap_or(DEFAULT_SYS_FLAGS_INT_MAX_STR_DIGITS),
            ),
            (b"gil", 1),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);

        for (key, value) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let value_bits = MoltObject::from_int(value).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_executable() -> u64 {
    crate::with_gil_entry!(_py, {
        let executable = match std::env::var("MOLT_SYS_EXECUTABLE") {
            Ok(val) if !val.is_empty() => val.into_bytes(),
            _ => runtime_state(_py)
                .argv
                .lock()
                .unwrap()
                .first()
                .cloned()
                .unwrap_or_default(),
        };
        let ptr = alloc_string(_py, &executable);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `argv` points to `argc` null-terminated strings.
pub unsafe extern "C" fn molt_set_argv(argc: i32, argv: *const *const u8) {
    unsafe {
        crate::with_gil_entry!(_py, {
            let mut args = Vec::new();
            if argc > 0 && !argv.is_null() {
                for idx in 0..argc {
                    let ptr = *argv.add(idx as usize);
                    if ptr.is_null() {
                        args.push(Vec::new());
                        continue;
                    }
                    let bytes = CStr::from_ptr(ptr as *const i8).to_bytes();
                    let (decoded, _) = decode_bytes_text("utf-8", "surrogateescape", bytes)
                        .expect("argv decode must succeed for utf-8+surrogateescape");
                    args.push(decoded);
                }
            }
            let trace_argv = matches!(std::env::var("MOLT_TRACE_ARGV").ok().as_deref(), Some("1"));
            if trace_argv {
                eprintln!("molt_set_argv argc={argc} argv0={:?}", args.first());
            }
            *runtime_state(_py).argv.lock().unwrap() = args;
        })
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
/// # Safety
/// Caller must ensure `argv` points to `argc` null-terminated UTF-16 strings.
pub unsafe extern "C" fn molt_set_argv_utf16(argc: i32, argv: *const *const u16) {
    crate::with_gil_entry!(_py, {
        let mut args = Vec::new();
        if argc > 0 && !argv.is_null() {
            for idx in 0..argc {
                let ptr = *argv.add(idx as usize);
                if ptr.is_null() {
                    args.push(Vec::new());
                    continue;
                }
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                let mut raw = Vec::with_capacity(slice.len() * 2);
                for &unit in slice {
                    raw.push((unit & 0x00FF) as u8);
                    raw.push((unit >> 8) as u8);
                }
                let (decoded, _) = decode_bytes_text("utf-16-le", "surrogatepass", &raw)
                    .expect("argv decode must succeed for utf-16-le+surrogatepass");
                args.push(decoded);
            }
        }
        *runtime_state(_py).argv.lock().unwrap() = args;
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getpid() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(target_arch = "wasm32")]
        {
            let pid = unsafe { crate::molt_getpid_host() };
            let pid = if pid < 0 { 0 } else { pid };
            MoltObject::from_int(pid).bits()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            MoltObject::from_int(std::process::id() as i64).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_signal_raise(sig_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(sig) = to_i64(obj_from_bits(sig_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "signal number must be int");
        };
        if sig < i32::MIN as i64 || sig > i32::MAX as i64 {
            return raise_exception::<_>(_py, "ValueError", "signal number out of range");
        }
        let sig_i32 = sig as i32;
        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            let rc = unsafe { libc::raise(sig_i32) };
            if rc != 0 {
                return raise_exception::<_>(
                    _py,
                    "OSError",
                    &std::io::Error::last_os_error().to_string(),
                );
            }
            MoltObject::none().bits()
        }
        #[cfg(any(not(unix), target_arch = "wasm32"))]
        {
            if sig_i32 == 2 {
                return raise_exception::<_>(_py, "KeyboardInterrupt", "signal interrupt");
            }
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_monotonic() -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_float(monotonic_now_secs(_py)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_perf_counter() -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_float(monotonic_now_secs(_py)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_monotonic_ns() -> u64 {
    crate::with_gil_entry!(_py, {
        int_bits_from_bigint(_py, BigInt::from(monotonic_now_nanos(_py)))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_perf_counter_ns() -> u64 {
    crate::with_gil_entry!(_py, {
        int_bits_from_bigint(_py, BigInt::from(monotonic_now_nanos(_py)))
    })
}

#[cfg(all(not(target_arch = "wasm32"), unix))]
fn process_time_duration() -> Result<std::time::Duration, String> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if ts.tv_sec < 0 || ts.tv_nsec < 0 {
        return Err("process time before epoch".to_string());
    }
    Ok(std::time::Duration::new(
        ts.tv_sec as u64,
        ts.tv_nsec as u32,
    ))
}

#[cfg(all(not(target_arch = "wasm32"), windows))]
fn process_time_duration() -> Result<std::time::Duration, String> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut kernel = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut user = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let handle = unsafe { GetCurrentProcess() };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let kernel_100ns = ((kernel.dwHighDateTime as u64) << 32) | kernel.dwLowDateTime as u64;
    let user_100ns = ((user.dwHighDateTime as u64) << 32) | user.dwLowDateTime as u64;
    let total_100ns = kernel_100ns.saturating_add(user_100ns);
    let secs = total_100ns / 10_000_000;
    let nanos = (total_100ns % 10_000_000) * 100;
    Ok(std::time::Duration::new(secs, nanos as u32))
}

#[cfg(any(target_arch = "wasm32", not(any(unix, windows))))]
fn process_time_duration() -> Result<std::time::Duration, String> {
    Err("process_time unavailable".to_string())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_process_time() -> u64 {
    crate::with_gil_entry!(_py, {
        match process_time_duration() {
            Ok(duration) => MoltObject::from_float(duration.as_secs_f64()).bits(),
            Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_process_time_ns() -> u64 {
    crate::with_gil_entry!(_py, {
        match process_time_duration() {
            Ok(duration) => int_bits_from_bigint(_py, BigInt::from(duration.as_nanos())),
            Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_time() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if require_time_wall_capability::<u64>(_py).is_err() {
                return MoltObject::none().bits();
            }
        }
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(now) => now,
            Err(_) => {
                return raise_exception::<_>(_py, "OSError", "system time before epoch");
            }
        };
        MoltObject::from_float(now.as_secs_f64()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_time_ns() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if require_time_wall_capability::<u64>(_py).is_err() {
                return MoltObject::none().bits();
            }
        }
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(now) => now,
            Err(_) => {
                return raise_exception::<_>(_py, "OSError", "system time before epoch");
            }
        };
        int_bits_from_bigint(_py, BigInt::from(now.as_nanos()))
    })
}

#[derive(Clone, Copy, Debug)]
struct TimeParts {
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
    wday: i32,
    yday: i32,
    isdst: i32,
}

fn time_parts_to_tuple(_py: &PyToken<'_>, parts: TimeParts) -> u64 {
    let elems = [
        MoltObject::from_int(parts.year as i64).bits(),
        MoltObject::from_int(parts.month as i64).bits(),
        MoltObject::from_int(parts.day as i64).bits(),
        MoltObject::from_int(parts.hour as i64).bits(),
        MoltObject::from_int(parts.minute as i64).bits(),
        MoltObject::from_int(parts.second as i64).bits(),
        MoltObject::from_int(parts.wday as i64).bits(),
        MoltObject::from_int(parts.yday as i64).bits(),
        MoltObject::from_int(parts.isdst as i64).bits(),
    ];
    let tuple_ptr = alloc_tuple(_py, &elems);
    if tuple_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn time_parts_from_tm(tm: &libc::tm) -> TimeParts {
    let wday = (tm.tm_wday + 6).rem_euclid(7);
    TimeParts {
        year: tm.tm_year + 1900,
        month: tm.tm_mon + 1,
        day: tm.tm_mday,
        hour: tm.tm_hour,
        minute: tm.tm_min,
        second: tm.tm_sec,
        wday,
        yday: tm.tm_yday + 1,
        isdst: tm.tm_isdst,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn tm_from_time_parts(_py: &PyToken<'_>, parts: TimeParts) -> Result<libc::tm, u64> {
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    tm.tm_sec = parts.second;
    tm.tm_min = parts.minute;
    tm.tm_hour = parts.hour;
    tm.tm_mday = parts.day;
    tm.tm_mon = parts.month - 1;
    tm.tm_year = parts.year - 1900;
    tm.tm_wday = (parts.wday + 1).rem_euclid(7);
    tm.tm_yday = parts.yday - 1;
    tm.tm_isdst = parts.isdst;
    if tm.tm_mon < 0 || tm.tm_mon > 11 {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "strftime() argument 2 out of range",
        ));
    }
    Ok(tm)
}

#[cfg(target_arch = "wasm32")]
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[cfg(target_arch = "wasm32")]
fn day_of_year(year: i32, month: i32, day: i32) -> i32 {
    const DAYS_BEFORE_MONTH: [[i32; 13]; 2] = [
        [0, 0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334],
        [0, 0, 31, 60, 91, 121, 152, 182, 213, 244, 274, 305, 335],
    ];
    let leap = if is_leap_year(year) { 1 } else { 0 };
    let m = month.clamp(1, 12) as usize;
    DAYS_BEFORE_MONTH[leap][m] + day
}

#[cfg(target_arch = "wasm32")]
fn civil_from_days(days: i64) -> (i32, i32, i32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = (yoe + era * 400) as i32;
    let doy = (doe - (365 * yoe + yoe / 4 - yoe / 100)) as i32;
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1);
    let m = (mp + if mp < 10 { 3 } else { -9 });
    if m <= 2 {
        y += 1;
    }
    (y, m, d)
}

#[cfg(target_arch = "wasm32")]
fn time_parts_from_epoch_utc(secs: i64) -> TimeParts {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = (rem / 3600) as i32;
    let minute = ((rem % 3600) / 60) as i32;
    let second = (rem % 60) as i32;
    let (year, month, day) = civil_from_days(days);
    let yday = day_of_year(year, month, day);
    let wday = ((days + 3).rem_euclid(7)) as i32;
    TimeParts {
        year,
        month,
        day,
        hour,
        minute,
        second,
        wday,
        yday,
        isdst: 0,
    }
}

#[cfg(target_arch = "wasm32")]
fn timezone_west_wasm() -> Result<i64, String> {
    let offset = unsafe { crate::molt_time_timezone_host() };
    if offset == i64::MIN {
        return Err("timezone unavailable".to_string());
    }
    Ok(offset)
}

#[cfg(target_arch = "wasm32")]
fn local_offset_west_wasm(secs: i64) -> Result<i64, String> {
    let offset = unsafe { crate::molt_time_local_offset_host(secs) };
    if offset == i64::MIN {
        return Err("localtime failed".to_string());
    }
    Ok(offset)
}

#[cfg(target_arch = "wasm32")]
fn tzname_label_wasm(which: i32) -> Result<String, String> {
    let mut buf = vec![0u8; 256];
    let mut out_len: u32 = 0;
    let status = unsafe {
        crate::molt_time_tzname_host(
            which,
            buf.as_mut_ptr() as u32,
            buf.len() as u32,
            (&mut out_len as *mut u32) as u32,
        )
    };
    if status != 0 {
        return Err("tzname unavailable".to_string());
    }
    let out_len = usize::try_from(out_len).map_err(|_| "tzname unavailable".to_string())?;
    if out_len > buf.len() {
        return Err("tzname unavailable".to_string());
    }
    buf.truncate(out_len);
    String::from_utf8(buf).map_err(|_| "tzname unavailable".to_string())
}

#[cfg(target_arch = "wasm32")]
fn tzname_wasm() -> Result<(String, String), String> {
    let std_name = tzname_label_wasm(0)?;
    let dst_name = tzname_label_wasm(1)?;
    Ok((std_name, dst_name))
}

fn current_epoch_secs_i64() -> Result<i64, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system time before epoch".to_string())?;
    Ok(i64::try_from(now.as_secs()).unwrap_or(i64::MAX))
}

fn parse_time_seconds(_py: &PyToken<'_>, secs_bits: u64) -> Result<i64, u64> {
    let obj = obj_from_bits(secs_bits);
    if obj.is_none() {
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(now) => now,
            Err(_) => {
                return Err(raise_exception::<_>(
                    _py,
                    "OSError",
                    "system time before epoch",
                ));
            }
        };
        let secs = now.as_secs();
        let secs = i64::try_from(secs).unwrap_or(i64::MAX);
        return Ok(secs);
    }
    let Some(val) = to_f64(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, secs_bits));
        let msg = format!("an integer is required (got type {type_name})");
        return Err(raise_exception::<_>(_py, "TypeError", &msg));
    };
    if !val.is_finite() {
        return Err(raise_exception::<_>(
            _py,
            "OverflowError",
            "timestamp out of range for platform time_t",
        ));
    }
    let secs = val.trunc();
    let (min, max) = time_t_bounds();
    if secs < min as f64 || secs > max as f64 {
        return Err(raise_exception::<_>(
            _py,
            "OverflowError",
            "timestamp out of range for platform time_t",
        ));
    }
    Ok(secs as i64)
}

#[cfg(not(target_arch = "wasm32"))]
fn time_t_bounds() -> (i128, i128) {
    let size = std::mem::size_of::<libc::time_t>();
    if size == 4 {
        (i32::MIN as i128, i32::MAX as i128)
    } else {
        (i64::MIN as i128, i64::MAX as i128)
    }
}

#[cfg(target_arch = "wasm32")]
fn time_t_bounds() -> (i128, i128) {
    (i64::MIN as i128, i64::MAX as i128)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let mut y = year as i64;
    let m = month as i64;
    let d = day as i64;
    y -= if m <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(not(target_arch = "wasm32"))]
fn tm_to_epoch_seconds(tm: &libc::tm) -> i64 {
    let year = tm.tm_year + 1900;
    let month = tm.tm_mon + 1;
    let day = tm.tm_mday;
    let days = days_from_civil(year, month, day);
    let seconds = (tm.tm_hour as i64) * 3600 + (tm.tm_min as i64) * 60 + (tm.tm_sec as i64);
    days.saturating_mul(86_400).saturating_add(seconds)
}

#[cfg(not(target_arch = "wasm32"))]
fn offset_west_from_secs(secs: i64) -> Result<i64, String> {
    let secs = secs as libc::time_t;
    let local_tm = localtime_tm(secs)?;
    let utc_tm = gmtime_tm(secs)?;
    let local_secs = tm_to_epoch_seconds(&local_tm);
    let utc_secs = tm_to_epoch_seconds(&utc_tm);
    Ok(utc_secs.saturating_sub(local_secs))
}

fn parse_time_tuple(_py: &PyToken<'_>, tuple_bits: u64) -> Result<TimeParts, u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "strftime() argument 2 must be tuple",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            let type_name = class_name_for_error(type_of_bits(_py, tuple_bits));
            let msg = format!("strftime() argument 2 must be tuple, not {type_name}");
            return Err(raise_exception::<_>(_py, "TypeError", &msg));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() != 9 {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "time tuple must have exactly 9 elements",
            ));
        }
        let mut vals = [0i64; 9];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    "strftime() argument 2 out of range",
                ));
            }
            *slot = val;
        }
        let year = vals[0] as i32;
        let month = vals[1] as i32;
        let day = vals[2] as i32;
        let hour = vals[3] as i32;
        let minute = vals[4] as i32;
        let second = vals[5] as i32;
        let wday = vals[6] as i32;
        let yday = vals[7] as i32;
        let isdst = vals[8] as i32;
        if !(1..=12).contains(&month)
            || !(1..=31).contains(&day)
            || !(0..=23).contains(&hour)
            || !(0..=59).contains(&minute)
            || !(0..=60).contains(&second)
            || !(0..=6).contains(&wday)
            || !(1..=366).contains(&yday)
        {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "strftime() argument 2 out of range",
            ));
        }
        if ![-1, 0, 1].contains(&isdst) {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "strftime() argument 2 out of range",
            ));
        }
        Ok(TimeParts {
            year,
            month,
            day,
            hour,
            minute,
            second,
            wday,
            yday,
            isdst,
        })
    }
}

fn asctime_from_parts(parts: TimeParts) -> Result<String, String> {
    const WEEKDAY_ABBR: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTH_ABBR: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    if !(0..=6).contains(&parts.wday)
        || !(1..=12).contains(&parts.month)
        || !(1..=31).contains(&parts.day)
    {
        return Err("time tuple elements out of range".to_string());
    }
    let wday = WEEKDAY_ABBR[parts.wday as usize];
    let month = MONTH_ABBR[(parts.month - 1) as usize];
    Ok(format!(
        "{wday} {month} {:2} {:02}:{:02}:{:02} {:04}",
        parts.day, parts.hour, parts.minute, parts.second, parts.year
    ))
}

fn parse_mktime_tuple(_py: &PyToken<'_>, tuple_bits: u64) -> Result<TimeParts, u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "Tuple or struct_time argument required",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "Tuple or struct_time argument required",
            ));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() != 9 {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "mktime(): illegal time tuple argument",
            ));
        }
        let mut vals = [0i64; 9];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "mktime(): argument out of range",
                ));
            }
            *slot = val;
        }
        Ok(TimeParts {
            year: vals[0] as i32,
            month: vals[1] as i32,
            day: vals[2] as i32,
            hour: vals[3] as i32,
            minute: vals[4] as i32,
            second: vals[5] as i32,
            wday: vals[6] as i32,
            yday: vals[7] as i32,
            isdst: vals[8] as i32,
        })
    }
}

fn parse_timegm_tuple(
    _py: &PyToken<'_>,
    tuple_bits: u64,
) -> Result<(i32, i32, i32, i32, i32, i32), u64> {
    let obj = obj_from_bits(tuple_bits);
    let Some(ptr) = obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "Tuple or struct_time argument required",
        ));
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TUPLE {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "Tuple or struct_time argument required",
            ));
        }
        let elems = seq_vec_ref(ptr);
        if elems.len() < 6 {
            let msg = format!(
                "not enough values to unpack (expected 6, got {})",
                elems.len()
            );
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        }
        let mut vals = [0i64; 6];
        for (idx, slot) in vals.iter_mut().enumerate() {
            let bits = elems[idx];
            let Some(val) = to_i64(obj_from_bits(bits)) else {
                let type_name = class_name_for_error(type_of_bits(_py, bits));
                let msg = format!("an integer is required (got type {type_name})");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            };
            if val < i32::MIN as i64 || val > i32::MAX as i64 {
                return Err(raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "timegm(): argument out of range",
                ));
            }
            *slot = val;
        }
        Ok((
            vals[0] as i32,
            vals[1] as i32,
            vals[2] as i32,
            vals[3] as i32,
            vals[4] as i32,
            vals[5] as i32,
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn localtime_tm(secs: libc::time_t) -> Result<libc::tm, String> {
    #[cfg(unix)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        if libc::localtime_r(&secs as *const libc::time_t, &mut out).is_null() {
            return Err("localtime failed".to_string());
        }
        Ok(out)
    }
    #[cfg(windows)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        let rc = libc::localtime_s(&mut out as *mut libc::tm, &secs as *const libc::time_t);
        if rc != 0 {
            return Err("localtime failed".to_string());
        }
        Ok(out)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn gmtime_tm(secs: libc::time_t) -> Result<libc::tm, String> {
    #[cfg(unix)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        if libc::gmtime_r(&secs as *const libc::time_t, &mut out).is_null() {
            return Err("gmtime failed".to_string());
        }
        Ok(out)
    }
    #[cfg(windows)]
    unsafe {
        let mut out = std::mem::zeroed::<libc::tm>();
        let rc = libc::gmtime_s(&mut out as *mut libc::tm, &secs as *const libc::time_t);
        if rc != 0 {
            return Err("gmtime failed".to_string());
        }
        Ok(out)
    }
}

#[cfg(target_arch = "wasm32")]
fn strftime_wasm(format: &str, parts: TimeParts) -> Result<String, String> {
    const WEEKDAY_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const WEEKDAY_LONG: [&str; 7] = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    const MONTH_SHORT: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const MONTH_LONG: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];

    fn push_num(out: &mut String, val: i32, width: usize, pad: char) {
        let mut buf = [pad as u8; 12];
        let mut idx = buf.len();
        let mut n = val.unsigned_abs();
        if n == 0 {
            idx -= 1;
            buf[idx] = b'0';
        } else {
            while n > 0 {
                let digit = (n % 10) as u8;
                idx -= 1;
                buf[idx] = b'0' + digit;
                n /= 10;
            }
        }
        let len = buf.len() - idx;
        let needed = width.saturating_sub(len + if val < 0 { 1 } else { 0 });
        for _ in 0..needed {
            out.push(pad);
        }
        if val < 0 {
            out.push('-');
        }
        out.push_str(std::str::from_utf8(&buf[idx..]).unwrap_or("0"));
    }

    fn jan1_wday_mon0(yday: i32, wday_mon0: i32) -> i32 {
        let offset = (yday - 1).rem_euclid(7);
        (wday_mon0 - offset).rem_euclid(7)
    }

    fn week_number_sun(yday: i32, jan1_wday_mon0: i32) -> i32 {
        let jan1_sun0 = (jan1_wday_mon0 + 1).rem_euclid(7);
        let first_sunday = 1 + (7 - jan1_sun0).rem_euclid(7);
        if yday < first_sunday {
            0
        } else {
            1 + (yday - first_sunday) / 7
        }
    }

    fn week_number_mon(yday: i32, jan1_wday_mon0: i32) -> i32 {
        let first_monday = 1 + (7 - jan1_wday_mon0).rem_euclid(7);
        if yday < first_monday {
            0
        } else {
            1 + (yday - first_monday) / 7
        }
    }

    fn weeks_in_year(year: i32, jan1_wday_mon0: i32) -> i32 {
        let jan1_mon1 = jan1_wday_mon0 + 1;
        if jan1_mon1 == 4 || (is_leap_year(year) && jan1_mon1 == 3) {
            53
        } else {
            52
        }
    }

    fn iso_week_date(year: i32, yday: i32, wday_mon0: i32) -> (i32, i32, i32) {
        let weekday = wday_mon0 + 1;
        let mut week = (yday - weekday + 10) / 7;
        let jan1_wday = jan1_wday_mon0(yday, wday_mon0);
        let mut iso_year = year;
        let max_week = weeks_in_year(year, jan1_wday);
        if week < 1 {
            iso_year -= 1;
            let prev_days = if is_leap_year(iso_year) { 366 } else { 365 };
            let prev_jan1 = (jan1_wday - (prev_days % 7)).rem_euclid(7);
            week = weeks_in_year(iso_year, prev_jan1);
        } else if week > max_week {
            iso_year += 1;
            week = 1;
        }
        (iso_year, week, weekday)
    }

    let mut out = String::with_capacity(format.len() + 16);
    let mut iter = format.chars();
    while let Some(ch) = iter.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        let Some(spec) = iter.next() else {
            out.push('%');
            break;
        };
        match spec {
            '%' => out.push('%'),
            'a' => out.push_str(WEEKDAY_SHORT[parts.wday as usize]),
            'A' => out.push_str(WEEKDAY_LONG[parts.wday as usize]),
            'b' | 'h' => out.push_str(MONTH_SHORT[(parts.month - 1) as usize]),
            'B' => out.push_str(MONTH_LONG[(parts.month - 1) as usize]),
            'C' => {
                let century = parts.year.div_euclid(100);
                push_num(&mut out, century, 2, '0');
            }
            'd' => push_num(&mut out, parts.day, 2, '0'),
            'e' => push_num(&mut out, parts.day, 2, ' '),
            'H' => push_num(&mut out, parts.hour, 2, '0'),
            'I' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, '0');
            }
            'k' => push_num(&mut out, parts.hour, 2, ' '),
            'l' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, ' ');
            }
            'j' => push_num(&mut out, parts.yday, 3, '0'),
            'm' => push_num(&mut out, parts.month, 2, '0'),
            'M' => push_num(&mut out, parts.minute, 2, '0'),
            'p' => out.push_str(if parts.hour < 12 { "AM" } else { "PM" }),
            'S' => push_num(&mut out, parts.second, 2, '0'),
            'U' => {
                let jan1 = jan1_wday_mon0(parts.yday, parts.wday);
                let week = week_number_sun(parts.yday, jan1);
                push_num(&mut out, week, 2, '0');
            }
            'W' => {
                let jan1 = jan1_wday_mon0(parts.yday, parts.wday);
                let week = week_number_mon(parts.yday, jan1);
                push_num(&mut out, week, 2, '0');
            }
            'w' => {
                let wday_sun0 = (parts.wday + 1).rem_euclid(7);
                push_num(&mut out, wday_sun0, 1, '0');
            }
            'u' => {
                let wday_mon1 = parts.wday + 1;
                push_num(&mut out, wday_mon1, 1, '0');
            }
            'x' => {
                push_num(&mut out, parts.month, 2, '0');
                out.push('/');
                push_num(&mut out, parts.day, 2, '0');
                out.push('/');
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'X' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
            }
            'y' => {
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'Y' => push_num(&mut out, parts.year, 4, '0'),
            'Z' => out.push_str("UTC"),
            'z' => out.push_str("+0000"),
            'c' => {
                out.push_str(WEEKDAY_SHORT[parts.wday as usize]);
                out.push(' ');
                out.push_str(MONTH_SHORT[(parts.month - 1) as usize]);
                out.push(' ');
                push_num(&mut out, parts.day, 2, ' ');
                out.push(' ');
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
                out.push(' ');
                push_num(&mut out, parts.year, 4, '0');
            }
            'D' => {
                push_num(&mut out, parts.month, 2, '0');
                out.push('/');
                push_num(&mut out, parts.day, 2, '0');
                out.push('/');
                let yy = parts.year.rem_euclid(100);
                push_num(&mut out, yy, 2, '0');
            }
            'F' => {
                push_num(&mut out, parts.year, 4, '0');
                out.push('-');
                push_num(&mut out, parts.month, 2, '0');
                out.push('-');
                push_num(&mut out, parts.day, 2, '0');
            }
            'R' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
            }
            'r' => {
                let mut hour = parts.hour % 12;
                if hour == 0 {
                    hour = 12;
                }
                push_num(&mut out, hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
                out.push(' ');
                out.push_str(if parts.hour < 12 { "AM" } else { "PM" });
            }
            'T' => {
                push_num(&mut out, parts.hour, 2, '0');
                out.push(':');
                push_num(&mut out, parts.minute, 2, '0');
                out.push(':');
                push_num(&mut out, parts.second, 2, '0');
            }
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'G' | 'g' | 'V' => {
                let (iso_year, iso_week, _) = iso_week_date(parts.year, parts.yday, parts.wday);
                match spec {
                    'G' => push_num(&mut out, iso_year, 4, '0'),
                    'g' => {
                        let yy = iso_year.rem_euclid(100);
                        push_num(&mut out, yy, 2, '0');
                    }
                    _ => push_num(&mut out, iso_week, 2, '0'),
                }
            }
            _ => {
                return Err(format!("unsupported strftime directive %{spec}"));
            }
        }
    }
    Ok(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_localtime(secs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(secs_bits);
        #[cfg(not(target_arch = "wasm32"))]
        {
            if obj.is_none() && require_time_wall_capability::<u64>(_py).is_err() {
                return MoltObject::none().bits();
            }
        }
        let secs = match parse_time_seconds(_py, secs_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let secs = secs as libc::time_t;
            let tm = match localtime_tm(secs) {
                Ok(tm) => tm,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let parts = time_parts_from_tm(&tm);
            time_parts_to_tuple(_py, parts)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let offset_west = match local_offset_west_wasm(secs) {
                Ok(value) => value,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let mut parts = time_parts_from_epoch_utc(secs.saturating_sub(offset_west));
            let std_offset_west = timezone_west_wasm().unwrap_or(offset_west);
            parts.isdst = if offset_west != std_offset_west { 1 } else { 0 };
            time_parts_to_tuple(_py, parts)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_gmtime(secs_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(secs_bits);
        #[cfg(not(target_arch = "wasm32"))]
        {
            if obj.is_none() && require_time_wall_capability::<u64>(_py).is_err() {
                return MoltObject::none().bits();
            }
        }
        let secs = match parse_time_seconds(_py, secs_bits) {
            Ok(val) => val,
            Err(bits) => return bits,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let secs = secs as libc::time_t;
            let tm = match gmtime_tm(secs) {
                Ok(tm) => tm,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let parts = time_parts_from_tm(&tm);
            time_parts_to_tuple(_py, parts)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let parts = time_parts_from_epoch_utc(secs);
            time_parts_to_tuple(_py, parts)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_strftime(fmt_bits: u64, time_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let fmt_obj = obj_from_bits(fmt_bits);
        if fmt_obj.is_none() {
            return raise_exception::<_>(_py, "TypeError", "strftime() format must be str");
        }
        let Some(fmt) = string_obj_to_owned(fmt_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, fmt_bits));
            let msg = format!("strftime() format must be str, not {type_name}");
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if fmt.as_bytes().contains(&0) {
            return raise_exception::<_>(_py, "ValueError", "embedded null character");
        }
        let parts = match parse_time_tuple(_py, time_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let tm = match tm_from_time_parts(_py, parts) {
                Ok(tm) => tm,
                Err(bits) => return bits,
            };
            let c_fmt = match CString::new(fmt) {
                Ok(c) => c,
                Err(_) => {
                    return raise_exception::<_>(_py, "ValueError", "embedded null character");
                }
            };
            let mut buf = vec![0u8; 128];
            loop {
                let len = unsafe {
                    libc::strftime(
                        buf.as_mut_ptr() as *mut libc::c_char,
                        buf.len(),
                        c_fmt.as_ptr(),
                        &tm as *const libc::tm,
                    )
                };
                if len == 0 {
                    if buf.len() >= 1_048_576 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "strftime() result too large",
                        );
                    }
                    buf.resize(buf.len() * 2, 0);
                    continue;
                }
                let slice = &buf[..len];
                let Ok(text) = std::str::from_utf8(slice) else {
                    return raise_exception::<_>(
                        _py,
                        "UnicodeError",
                        "strftime() produced non-UTF-8 output",
                    );
                };
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let out = match strftime_wasm(&fmt, parts) {
                Ok(out) => out,
                Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
            };
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn tzname_native() -> Result<(String, String), String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut tzname: [*mut libc::c_char; 2];
        }
        tzset();
        let std_ptr = tzname[0];
        let dst_ptr = tzname[1];
        if std_ptr.is_null() || dst_ptr.is_null() {
            return Err("tzname unavailable".to_string());
        }
        let std_name = CStr::from_ptr(std_ptr).to_string_lossy().into_owned();
        let dst_name = CStr::from_ptr(dst_ptr).to_string_lossy().into_owned();
        Ok((std_name, dst_name))
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("tzname unavailable".to_string());
        }
        let std_len = info
            .StandardName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.StandardName.len());
        let dst_len = info
            .DaylightName
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(info.DaylightName.len());
        let std_name = String::from_utf16_lossy(&info.StandardName[..std_len]);
        let dst_name = String::from_utf16_lossy(&info.DaylightName[..dst_len]);
        return Ok((std_name, dst_name));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn timezone_native() -> Result<i64, String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut timezone: libc::c_long;
        }
        tzset();
        Ok(timezone)
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("timezone unavailable".to_string());
        }
        let bias = info.Bias + info.StandardBias;
        return Ok((bias as i64) * 60);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn daylight_native() -> Result<i64, String> {
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn tzset();
            static mut daylight: libc::c_int;
        }
        tzset();
        Ok(if daylight != 0 { 1 } else { 0 })
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("daylight unavailable".to_string());
        }
        return Ok(if info.DaylightDate.wMonth != 0 { 1 } else { 0 });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn sample_offset_west_native(year: i32, month: i32, day: i32) -> Result<i64, String> {
    let days = days_from_civil(year, month, day);
    let secs = days.saturating_mul(86_400).saturating_add(12 * 3600);
    offset_west_from_secs(secs)
}

#[cfg(not(target_arch = "wasm32"))]
fn altzone_native() -> Result<i64, String> {
    let std_offset = timezone_native()?;
    if daylight_native()? == 0 {
        return Ok(std_offset);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };
        let mut info = TIME_ZONE_INFORMATION {
            Bias: 0,
            StandardName: [0u16; 32],
            StandardDate: std::mem::zeroed(),
            StandardBias: 0,
            DaylightName: [0u16; 32],
            DaylightDate: std::mem::zeroed(),
            DaylightBias: 0,
        };
        let status = GetTimeZoneInformation(&mut info as *mut TIME_ZONE_INFORMATION);
        if status == TIME_ZONE_ID_INVALID {
            return Err("altzone unavailable".to_string());
        }
        let bias = info.Bias + info.DaylightBias;
        return Ok((bias as i64) * 60);
    }
    #[cfg(unix)]
    {
        let now = current_epoch_secs_i64()?;
        let local_tm = localtime_tm(now as libc::time_t)?;
        let year = local_tm.tm_year + 1900;
        let jan = sample_offset_west_native(year, 1, 1).unwrap_or(std_offset);
        let jul = sample_offset_west_native(year, 7, 1).unwrap_or(std_offset);
        if jan != std_offset && jul == std_offset {
            return Ok(jan);
        }
        if jul != std_offset && jan == std_offset {
            return Ok(jul);
        }
        if jan != jul {
            return Ok(std::cmp::min(jan, jul));
        }
        Ok(jan)
    }
}

#[cfg(target_arch = "wasm32")]
fn sample_offset_west_wasm(year: i32, month: i32, day: i32) -> Result<i64, String> {
    let days = days_from_civil(year, month, day);
    let secs = days.saturating_mul(86_400).saturating_add(12 * 3600);
    local_offset_west_wasm(secs)
}

#[cfg(target_arch = "wasm32")]
fn daylight_wasm() -> Result<i64, String> {
    let year = time_parts_from_epoch_utc(current_epoch_secs_i64()?).year;
    let jan = sample_offset_west_wasm(year, 1, 1)?;
    let jul = sample_offset_west_wasm(year, 7, 1)?;
    Ok(if jan != jul { 1 } else { 0 })
}

#[cfg(target_arch = "wasm32")]
fn altzone_wasm() -> Result<i64, String> {
    let std_offset = timezone_west_wasm()?;
    if daylight_wasm()? == 0 {
        return Ok(std_offset);
    }
    let year = time_parts_from_epoch_utc(current_epoch_secs_i64()?).year;
    let jan = sample_offset_west_wasm(year, 1, 1).unwrap_or(std_offset);
    let jul = sample_offset_west_wasm(year, 7, 1).unwrap_or(std_offset);
    if jan != std_offset && jul == std_offset {
        return Ok(jan);
    }
    if jul != std_offset && jan == std_offset {
        return Ok(jul);
    }
    if jan != jul {
        return Ok(std::cmp::min(jan, jul));
    }
    Ok(jan)
}

#[cfg(not(target_arch = "wasm32"))]
fn mktime_native(parts: TimeParts) -> f64 {
    let mut tm = unsafe { std::mem::zeroed::<libc::tm>() };
    tm.tm_sec = parts.second;
    tm.tm_min = parts.minute;
    tm.tm_hour = parts.hour;
    tm.tm_mday = parts.day;
    tm.tm_mon = parts.month - 1;
    tm.tm_year = parts.year - 1900;
    tm.tm_wday = (parts.wday + 1).rem_euclid(7);
    tm.tm_yday = parts.yday - 1;
    tm.tm_isdst = parts.isdst;
    let out = unsafe { libc::mktime(&mut tm as *mut libc::tm) };
    out as f64
}

#[cfg(target_arch = "wasm32")]
fn mktime_wasm(parts: TimeParts) -> Result<f64, String> {
    let days = days_from_civil(parts.year, parts.month, parts.day);
    let local_secs = days
        .saturating_mul(86_400)
        .saturating_add((parts.hour as i64).saturating_mul(3600))
        .saturating_add((parts.minute as i64).saturating_mul(60))
        .saturating_add(parts.second as i64);
    let std_offset = timezone_west_wasm()?;
    let utc_secs = if parts.isdst > 0 {
        let dst_offset = altzone_wasm().unwrap_or(std_offset);
        local_secs.saturating_add(dst_offset)
    } else if parts.isdst == 0 {
        local_secs.saturating_add(std_offset)
    } else {
        let mut guess = local_secs.saturating_add(std_offset);
        for _ in 0..3 {
            let offset = local_offset_west_wasm(guess).unwrap_or(std_offset);
            let next = local_secs.saturating_add(offset);
            if next == guess {
                break;
            }
            guess = next;
        }
        guess
    };
    Ok(utc_secs as f64)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_timezone() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match timezone_native() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            match timezone_west_wasm() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_daylight() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match daylight_native() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            match daylight_wasm() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_altzone() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match altzone_native() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            match altzone_wasm() {
                Ok(val) => MoltObject::from_int(val).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_tzname() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (std_name, dst_name) = match tzname_native() {
                Ok(res) => res,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let std_ptr = alloc_string(_py, std_name.as_bytes());
            if std_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let dst_ptr = alloc_string(_py, dst_name.as_bytes());
            if dst_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(std_ptr).bits());
                return MoltObject::none().bits();
            }
            let std_bits = MoltObject::from_ptr(std_ptr).bits();
            let dst_bits = MoltObject::from_ptr(dst_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[std_bits, dst_bits]);
            dec_ref_bits(_py, std_bits);
            dec_ref_bits(_py, dst_bits);
            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let (std_name, dst_name) = match tzname_wasm() {
                Ok(res) => res,
                Err(msg) => return raise_exception::<_>(_py, "OSError", &msg),
            };
            let std_ptr = alloc_string(_py, std_name.as_bytes());
            if std_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let dst_ptr = alloc_string(_py, dst_name.as_bytes());
            if dst_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(std_ptr).bits());
                return MoltObject::none().bits();
            }
            let std_bits = MoltObject::from_ptr(std_ptr).bits();
            let dst_bits = MoltObject::from_ptr(dst_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[std_bits, dst_bits]);
            dec_ref_bits(_py, std_bits);
            dec_ref_bits(_py, dst_bits);
            if tuple_ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(tuple_ptr).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_asctime(time_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match parse_time_tuple(_py, time_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let text = match asctime_from_parts(parts) {
            Ok(text) => text,
            Err(msg) => return raise_exception::<_>(_py, "ValueError", &msg),
        };
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_mktime(time_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match parse_mktime_tuple(_py, time_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            MoltObject::from_float(mktime_native(parts)).bits()
        }
        #[cfg(target_arch = "wasm32")]
        {
            match mktime_wasm(parts) {
                Ok(out) => MoltObject::from_float(out).bits(),
                Err(msg) => raise_exception::<_>(_py, "OSError", &msg),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_timegm(time_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (year, month, day, hour, minute, second) = match parse_timegm_tuple(_py, time_bits) {
            Ok(parts) => parts,
            Err(bits) => return bits,
        };
        let days = days_from_civil(year, month, day);
        let seconds = days
            .saturating_mul(86_400)
            .saturating_add((hour as i64).saturating_mul(3600))
            .saturating_add((minute as i64).saturating_mul(60))
            .saturating_add(second as i64);
        MoltObject::from_int(seconds).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_time_get_clock_info(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "unknown clock");
        };
        let (name_value, implementation, resolution, monotonic, adjustable) = match name.as_str() {
            "monotonic" | "perf_counter" => (name.as_str(), "molt", 1e-9f64, true, false),
            "process_time" => ("process_time", "molt", 1e-9f64, true, false),
            "time" => {
                #[cfg(not(target_arch = "wasm32"))]
                if require_time_wall_capability::<u64>(_py).is_err() {
                    return MoltObject::none().bits();
                }
                ("time", "molt", 1e-6f64, false, true)
            }
            _ => return raise_exception::<_>(_py, "ValueError", "unknown clock"),
        };
        let name_ptr = alloc_string(_py, name_value.as_bytes());
        if name_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let impl_ptr = alloc_string(_py, implementation.as_bytes());
        if impl_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
            return MoltObject::none().bits();
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let impl_bits = MoltObject::from_ptr(impl_ptr).bits();
        let resolution_bits = MoltObject::from_float(resolution).bits();
        let monotonic_bits = MoltObject::from_bool(monotonic).bits();
        let adjustable_bits = MoltObject::from_bool(adjustable).bits();
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                name_bits,
                impl_bits,
                resolution_bits,
                monotonic_bits,
                adjustable_bits,
            ],
        );
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, impl_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

fn traceback_limit_from_bits(_py: &PyToken<'_>, limit_bits: u64) -> Result<Option<usize>, u64> {
    let obj = obj_from_bits(limit_bits);
    if obj.is_none() {
        return Ok(None);
    }
    let Some(limit) = to_i64(obj) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "limit must be an integer",
        ));
    };
    if limit < 0 {
        return Ok(Some(0));
    }
    Ok(Some(limit as usize))
}

fn traceback_frames(
    _py: &PyToken<'_>,
    tb_bits: u64,
    limit: Option<usize>,
) -> Vec<(String, i64, String)> {
    if obj_from_bits(tb_bits).is_none() {
        return Vec::new();
    }
    let tb_frame_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_frame_name, b"tb_frame");
    let tb_lineno_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.tb_lineno_name,
        b"tb_lineno",
    );
    let tb_next_bits =
        intern_static_name(_py, &runtime_state(_py).interned.tb_next_name, b"tb_next");
    let f_code_bits = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_bits =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let mut out: Vec<(String, i64, String)> = Vec::new();
    let mut current_bits = tb_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if let Some(max) = limit
            && out.len() >= max
        {
            break;
        }
        if depth > 512 {
            break;
        }
        let tb_obj = obj_from_bits(current_bits);
        let Some(tb_ptr) = tb_obj.as_ptr() else {
            break;
        };
        let (frame_bits, line, next_bits, had_tb_fields) = unsafe {
            let dict_bits = instance_dict_bits(tb_ptr);
            let mut frame_bits = MoltObject::none().bits();
            let mut line = 0i64;
            let mut next_bits = MoltObject::none().bits();
            let mut had_tb_fields = false;
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_frame_bits) {
                    frame_bits = bits;
                    had_tb_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_lineno_bits) {
                    if let Some(val) = to_i64(obj_from_bits(bits)) {
                        line = val;
                    }
                    had_tb_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, tb_next_bits) {
                    next_bits = bits;
                    had_tb_fields = true;
                }
            }
            (frame_bits, line, next_bits, had_tb_fields)
        };
        if !had_tb_fields {
            break;
        }
        let (filename, func_name, frame_line) = unsafe {
            let mut filename = "<unknown>".to_string();
            let mut func_name = "<module>".to_string();
            let mut frame_line = line;
            if let Some(frame_ptr) = obj_from_bits(frame_bits).as_ptr() {
                let dict_bits = instance_dict_bits(frame_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_lineno_bits)
                        && let Some(val) = to_i64(obj_from_bits(bits))
                    {
                        frame_line = val;
                    }
                    if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_code_bits)
                        && let Some(code_ptr) = obj_from_bits(bits).as_ptr()
                        && object_type_id(code_ptr) == TYPE_ID_CODE
                    {
                        let filename_bits = code_filename_bits(code_ptr);
                        if let Some(name) = string_obj_to_owned(obj_from_bits(filename_bits)) {
                            filename = name;
                        }
                        let name_bits = code_name_bits(code_ptr);
                        if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits))
                            && !name.is_empty()
                        {
                            func_name = name;
                        }
                    }
                }
            }
            (filename, func_name, frame_line)
        };
        let final_line = if line > 0 { line } else { frame_line };
        out.push((filename, final_line, func_name));
        current_bits = next_bits;
        depth += 1;
    }
    out
}

fn traceback_source_line_native(_py: &PyToken<'_>, filename: &str, lineno: i64) -> String {
    if lineno <= 0 {
        return String::new();
    }
    if !has_capability(_py, "fs.read") {
        return String::new();
    }
    let Ok(file) = std::fs::File::open(filename) else {
        return String::new();
    };
    let reader = BufReader::new(file);
    let target = lineno as usize;
    for (idx, line_result) in reader.lines().enumerate() {
        if idx + 1 == target {
            if let Ok(line) = line_result {
                return line;
            }
            return String::new();
        }
    }
    String::new()
}

fn traceback_line_trim_bounds(line: &str) -> Option<(i64, i64)> {
    if line.is_empty() {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let mut start = 0usize;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    let mut end = chars.len();
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if end <= start {
        return None;
    }
    Some((start as i64, end as i64))
}

fn traceback_infer_column_offsets(line: &str) -> (i64, i64) {
    if line.is_empty() {
        return (0, 0);
    }
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return (0, 0);
    }
    let mut start = 0usize;
    while start < chars.len() && chars[start].is_whitespace() {
        start += 1;
    }
    if start >= chars.len() {
        return (0, 0);
    }
    let mut end = chars.len();
    while end > start && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    let trimmed: String = chars[start..end].iter().collect();
    let mut highlighted_start = start;
    if let Some(rest) = trimmed
        .strip_prefix("return ")
        .or_else(|| trimmed.strip_prefix("raise "))
        .or_else(|| trimmed.strip_prefix("yield "))
        .or_else(|| trimmed.strip_prefix("await "))
        .or_else(|| trimmed.strip_prefix("assert "))
    {
        highlighted_start = end.saturating_sub(rest.chars().count());
        while highlighted_start < end && chars[highlighted_start].is_whitespace() {
            highlighted_start += 1;
        }
    } else {
        let trimmed_chars: Vec<char> = trimmed.chars().collect();
        for idx in 0..trimmed_chars.len() {
            if trimmed_chars[idx] != '=' {
                continue;
            }
            let prev = if idx > 0 {
                Some(trimmed_chars[idx - 1])
            } else {
                None
            };
            let next = if idx + 1 < trimmed_chars.len() {
                Some(trimmed_chars[idx + 1])
            } else {
                None
            };
            if matches!(prev, Some('=' | '!' | '<' | '>' | ':')) || matches!(next, Some('=')) {
                continue;
            }
            let mut rhs_start = start + idx + 1;
            while rhs_start < end && chars[rhs_start].is_whitespace() {
                rhs_start += 1;
            }
            if rhs_start < end {
                highlighted_start = rhs_start;
            }
            break;
        }
    }
    let col = highlighted_start as i64;
    let end_col = end.max(highlighted_start) as i64;
    if end_col <= col {
        (col, col + 1)
    } else {
        (col, end_col)
    }
}

fn traceback_format_caret_line_native(line: &str, mut colno: i64, mut end_colno: i64) -> String {
    if line.is_empty() || colno < 0 {
        return String::new();
    }
    let text_len = line.chars().count() as i64;
    if text_len <= 0 {
        return String::new();
    }
    if end_colno < colno {
        end_colno = colno;
    }
    if colno > text_len {
        colno = text_len;
    }
    if end_colno > text_len {
        end_colno = text_len;
    }
    let Some((trim_start, trim_end)) = traceback_line_trim_bounds(line) else {
        return String::new();
    };
    if colno < trim_start {
        colno = trim_start;
    }
    if end_colno > trim_end {
        end_colno = trim_end;
    }
    if end_colno <= colno {
        return String::new();
    }
    let width = end_colno - colno;
    let col_usize = colno as usize;
    let mut out = String::with_capacity((4 + colno + width + 1) as usize);
    out.push_str("    ");
    for ch in line.chars().take(col_usize) {
        if ch == '\t' {
            out.push('\t');
        } else {
            out.push(' ');
        }
    }
    for _ in 0..width {
        out.push('^');
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod traceback_format_tests {
    use super::{traceback_format_caret_line_native, traceback_infer_column_offsets};

    #[test]
    fn infer_column_offsets_prefers_rhs_for_assignment() {
        let (col, end_col) = traceback_infer_column_offsets("total = left + right   ");
        assert_eq!(col, 8);
        assert!(end_col > col);
    }

    #[test]
    fn infer_column_offsets_skips_return_keyword() {
        let (col, end_col) = traceback_infer_column_offsets("    return value");
        assert_eq!(col, 11);
        assert_eq!(end_col, 16);
    }

    #[test]
    fn caret_line_preserves_tabs_for_alignment() {
        let line = "\titem = source";
        let caret = traceback_format_caret_line_native(line, 1, 5);
        assert!(caret.starts_with("    \t"));
        assert!(caret.contains("^^^^"));
    }

    #[test]
    fn caret_line_omits_invalid_ranges() {
        let line = "value = source";
        assert!(traceback_format_caret_line_native(line, 0, 0).is_empty());
        assert!(traceback_format_caret_line_native(line, 10, 5).is_empty());
    }
}

fn traceback_format_exception_only_line(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
) -> String {
    let value_obj = obj_from_bits(value_bits);
    if let Some(exc_ptr) = value_obj.as_ptr() {
        unsafe {
            if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                let mut kind = "Exception".to_string();
                let class_bits = exception_class_bits(exc_ptr);
                if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    let name_bits = class_name_bits(class_ptr);
                    if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
                        kind = name;
                    }
                }
                let message = format_exception_message(_py, exc_ptr);
                if message.is_empty() {
                    return format!("{kind}\n");
                }
                return format!("{kind}: {message}\n");
            }
        }
    }
    let type_name = if !obj_from_bits(exc_type_bits).is_none() {
        if let Some(tp_ptr) = obj_from_bits(exc_type_bits).as_ptr() {
            unsafe {
                if object_type_id(tp_ptr) == TYPE_ID_TYPE {
                    let name_bits = class_name_bits(tp_ptr);
                    if let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) {
                        name
                    } else {
                        "Exception".to_string()
                    }
                } else {
                    class_name_for_error(type_of_bits(_py, exc_type_bits))
                }
            }
        } else {
            "Exception".to_string()
        }
    } else if !value_obj.is_none() {
        class_name_for_error(type_of_bits(_py, value_bits))
    } else {
        "Exception".to_string()
    };
    if value_obj.is_none() {
        return format!("{type_name}\n");
    }
    let text = format_obj_str(_py, value_obj);
    if text.is_empty() {
        format!("{type_name}\n")
    } else {
        format!("{type_name}: {text}\n")
    }
}

fn traceback_exception_type_bits(_py: &PyToken<'_>, value_bits: u64) -> u64 {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                return exception_class_bits(ptr);
            }
        }
    }
    if obj_from_bits(value_bits).is_none() {
        MoltObject::none().bits()
    } else {
        type_of_bits(_py, value_bits)
    }
}

fn traceback_exception_trace_bits(value_bits: u64) -> u64 {
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                return exception_trace_bits(ptr);
            }
        }
    }
    MoltObject::none().bits()
}

fn traceback_append_exception_single_lines(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit: Option<usize>,
    out: &mut Vec<String>,
) {
    if !obj_from_bits(tb_bits).is_none() {
        out.push("Traceback (most recent call last):\n".to_string());
        let payload = traceback_payload_from_source(_py, tb_bits, limit);
        out.extend(traceback_payload_to_formatted_lines(_py, &payload));
    }
    out.push(traceback_format_exception_only_line(
        _py,
        exc_type_bits,
        value_bits,
    ));
}

#[allow(clippy::too_many_arguments)]
fn traceback_append_exception_chain_lines(
    _py: &PyToken<'_>,
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit: Option<usize>,
    chain: bool,
    seen: &mut HashSet<u64>,
    out: &mut Vec<String>,
) {
    if obj_from_bits(value_bits).is_none() || !chain {
        traceback_append_exception_single_lines(
            _py,
            exc_type_bits,
            value_bits,
            tb_bits,
            limit,
            out,
        );
        return;
    }
    if seen.contains(&value_bits) {
        traceback_append_exception_single_lines(
            _py,
            exc_type_bits,
            value_bits,
            tb_bits,
            limit,
            out,
        );
        return;
    }
    seen.insert(value_bits);
    if let Some(ptr) = obj_from_bits(value_bits).as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_EXCEPTION {
                let cause_bits = exception_cause_bits(ptr);
                if !obj_from_bits(cause_bits).is_none() {
                    let cause_type_bits = traceback_exception_type_bits(_py, cause_bits);
                    let cause_tb_bits = traceback_exception_trace_bits(cause_bits);
                    traceback_append_exception_chain_lines(
                        _py,
                        cause_type_bits,
                        cause_bits,
                        cause_tb_bits,
                        limit,
                        chain,
                        seen,
                        out,
                    );
                    out.push(
                        "The above exception was the direct cause of the following exception:\n\n"
                            .to_string(),
                    );
                    traceback_append_exception_single_lines(
                        _py,
                        exc_type_bits,
                        value_bits,
                        tb_bits,
                        limit,
                        out,
                    );
                    return;
                }
                let context_bits = exception_context_bits(ptr);
                let suppress_context = is_truthy(_py, obj_from_bits(exception_suppress_bits(ptr)));
                if !suppress_context && !obj_from_bits(context_bits).is_none() {
                    let context_type_bits = traceback_exception_type_bits(_py, context_bits);
                    let context_tb_bits = traceback_exception_trace_bits(context_bits);
                    traceback_append_exception_chain_lines(
                        _py,
                        context_type_bits,
                        context_bits,
                        context_tb_bits,
                        limit,
                        chain,
                        seen,
                        out,
                    );
                    out.push(
                        "During handling of the above exception, another exception occurred:\n\n"
                            .to_string(),
                    );
                    traceback_append_exception_single_lines(
                        _py,
                        exc_type_bits,
                        value_bits,
                        tb_bits,
                        limit,
                        out,
                    );
                    return;
                }
            }
        }
    }
    traceback_append_exception_single_lines(_py, exc_type_bits, value_bits, tb_bits, limit, out);
}

fn traceback_lines_to_list(_py: &PyToken<'_>, lines: &[String]) -> u64 {
    let mut bits_vec: Vec<u64> = Vec::with_capacity(lines.len());
    for line in lines {
        let ptr = alloc_string(_py, line.as_bytes());
        if ptr.is_null() {
            for bits in bits_vec {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        bits_vec.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, bits_vec.as_slice());
    for bits in bits_vec {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

#[derive(Clone)]
struct TracebackPayloadFrame {
    filename: String,
    lineno: i64,
    end_lineno: i64,
    colno: i64,
    end_colno: i64,
    name: String,
    line: String,
}

#[derive(Clone)]
struct TracebackExceptionChainNode {
    value_bits: u64,
    frames: Vec<TracebackPayloadFrame>,
    suppress_context: bool,
    cause_index: Option<usize>,
    context_index: Option<usize>,
}

fn traceback_split_molt_symbol(name: &str) -> (String, String) {
    if let Some((module_hint, func)) = name.split_once("__")
        && !module_hint.is_empty()
    {
        let func_name = if func.is_empty() { name } else { func };
        return (format!("<molt:{module_hint}>"), func_name.to_string());
    }
    ("<molt>".to_string(), name.to_string())
}

fn traceback_payload_from_traceback(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    for (filename, lineno, name) in traceback_frames(_py, source_bits, limit) {
        let line = traceback_source_line_native(_py, &filename, lineno);
        let (colno, end_colno) = traceback_infer_column_offsets(&line);
        out.push(TracebackPayloadFrame {
            filename,
            lineno,
            end_lineno: lineno,
            colno,
            end_colno,
            name,
            line,
        });
    }
    out
}

fn traceback_payload_from_frame_chain(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    if obj_from_bits(source_bits).is_none() {
        return Vec::new();
    }
    static F_BACK_NAME: AtomicU64 = AtomicU64::new(0);
    let f_back_name = intern_static_name(_py, &F_BACK_NAME, b"f_back");
    let f_code_name = intern_static_name(_py, &runtime_state(_py).interned.f_code_name, b"f_code");
    let f_lineno_name =
        intern_static_name(_py, &runtime_state(_py).interned.f_lineno_name, b"f_lineno");
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    let mut current_bits = source_bits;
    let mut depth = 0usize;
    while !obj_from_bits(current_bits).is_none() {
        if depth > 1024 {
            break;
        }
        let Some(frame_ptr) = obj_from_bits(current_bits).as_ptr() else {
            break;
        };
        let (code_bits, lineno, back_bits, had_frame_fields) = unsafe {
            let dict_bits = instance_dict_bits(frame_ptr);
            let mut code_bits = MoltObject::none().bits();
            let mut lineno = 0i64;
            let mut back_bits = MoltObject::none().bits();
            let mut had_frame_fields = false;
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_code_name) {
                    code_bits = bits;
                    had_frame_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_lineno_name) {
                    if let Some(value) = to_i64(obj_from_bits(bits)) {
                        lineno = value;
                    }
                    had_frame_fields = true;
                }
                if let Some(bits) = dict_get_in_place(_py, dict_ptr, f_back_name) {
                    back_bits = bits;
                    had_frame_fields = true;
                }
            }
            (code_bits, lineno, back_bits, had_frame_fields)
        };
        if !had_frame_fields {
            break;
        }

        let mut filename = "<unknown>".to_string();
        let mut name = "<module>".to_string();
        if let Some(code_ptr) = obj_from_bits(code_bits).as_ptr() {
            unsafe {
                if object_type_id(code_ptr) == TYPE_ID_CODE {
                    let filename_bits = code_filename_bits(code_ptr);
                    if let Some(value) = string_obj_to_owned(obj_from_bits(filename_bits)) {
                        filename = value;
                    }
                    let name_bits = code_name_bits(code_ptr);
                    if let Some(value) = string_obj_to_owned(obj_from_bits(name_bits))
                        && !value.is_empty()
                    {
                        name = value;
                    }
                }
            }
        }
        let line = traceback_source_line_native(_py, &filename, lineno);
        let (colno, end_colno) = traceback_infer_column_offsets(&line);
        out.push(TracebackPayloadFrame {
            filename,
            lineno,
            end_lineno: lineno,
            colno,
            end_colno,
            name,
            line,
        });
        current_bits = back_bits;
        depth += 1;
    }
    out.reverse();
    if let Some(max) = limit
        && out.len() > max
    {
        return out[out.len() - max..].to_vec();
    }
    out
}

fn traceback_payload_from_entry(
    _py: &PyToken<'_>,
    entry_bits: u64,
) -> Option<TracebackPayloadFrame> {
    if obj_from_bits(entry_bits).is_none() {
        return None;
    }
    let entry_obj = obj_from_bits(entry_bits);
    if let Some(entry_ptr) = entry_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(entry_ptr);
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(entry_ptr);
                if elems.is_empty() {
                    return None;
                }
                if elems.len() == 1 {
                    return traceback_payload_from_entry(_py, elems[0]);
                }
                if elems.len() >= 7 {
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let end_lineno = to_i64(obj_from_bits(elems[2])).unwrap_or(lineno);
                    let mut colno = to_i64(obj_from_bits(elems[3])).unwrap_or(0);
                    let mut end_colno = to_i64(obj_from_bits(elems[4])).unwrap_or(colno.max(0));
                    let name = format_obj_str(_py, obj_from_bits(elems[5]));
                    let line = if obj_from_bits(elems[6]).is_none() {
                        String::new()
                    } else {
                        format_obj_str(_py, obj_from_bits(elems[6]))
                    };
                    if !line.is_empty() && (colno < 0 || end_colno <= colno) {
                        let inferred = traceback_infer_column_offsets(&line);
                        colno = inferred.0;
                        end_colno = inferred.1;
                    }
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno,
                        colno,
                        end_colno,
                        name,
                        line,
                    });
                }
                if elems.len() >= 4 {
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str(_py, obj_from_bits(elems[2]));
                    let line = if obj_from_bits(elems[3]).is_none() {
                        String::new()
                    } else {
                        format_obj_str(_py, obj_from_bits(elems[3]))
                    };
                    let (colno, end_colno) = traceback_infer_column_offsets(&line);
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno: lineno,
                        colno,
                        end_colno,
                        name,
                        line,
                    });
                }
                if elems.len() >= 3 {
                    let filename = format_obj_str(_py, obj_from_bits(elems[0]));
                    let lineno = to_i64(obj_from_bits(elems[1])).unwrap_or(0);
                    let name = format_obj_str(_py, obj_from_bits(elems[2]));
                    let line = traceback_source_line_native(_py, &filename, lineno);
                    let (colno, end_colno) = traceback_infer_column_offsets(&line);
                    return Some(TracebackPayloadFrame {
                        filename,
                        lineno,
                        end_lineno: lineno,
                        colno,
                        end_colno,
                        name,
                        line,
                    });
                }
                if elems.len() == 2 {
                    let first_obj = obj_from_bits(elems[0]);
                    let second_obj = obj_from_bits(elems[1]);
                    if let (Some(filename), Some(lineno)) =
                        (string_obj_to_owned(first_obj), to_i64(second_obj))
                    {
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno,
                            end_lineno: lineno,
                            colno: 0,
                            end_colno: 0,
                            name: "<module>".to_string(),
                            line: String::new(),
                        });
                    }
                    if let (Some(lineno), Some(filename)) =
                        (to_i64(first_obj), string_obj_to_owned(second_obj))
                    {
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno,
                            end_lineno: lineno,
                            colno: 0,
                            end_colno: 0,
                            name: "<module>".to_string(),
                            line: String::new(),
                        });
                    }
                    if let (Some(symbol), Some(_name)) = (
                        string_obj_to_owned(first_obj),
                        string_obj_to_owned(second_obj),
                    ) {
                        let (filename, name) = traceback_split_molt_symbol(&symbol);
                        return Some(TracebackPayloadFrame {
                            filename,
                            lineno: 0,
                            end_lineno: 0,
                            colno: 0,
                            end_colno: 0,
                            name,
                            line: String::new(),
                        });
                    }
                }
                return None;
            }
            if type_id == TYPE_ID_DICT {
                static FILENAME_NAME: AtomicU64 = AtomicU64::new(0);
                static LINENO_NAME: AtomicU64 = AtomicU64::new(0);
                static NAME_NAME: AtomicU64 = AtomicU64::new(0);
                static LINE_NAME: AtomicU64 = AtomicU64::new(0);
                static END_LINENO_NAME: AtomicU64 = AtomicU64::new(0);
                static COLNO_NAME: AtomicU64 = AtomicU64::new(0);
                static END_COLNO_NAME: AtomicU64 = AtomicU64::new(0);
                let filename_key = intern_static_name(_py, &FILENAME_NAME, b"filename");
                let lineno_key = intern_static_name(_py, &LINENO_NAME, b"lineno");
                let name_key = intern_static_name(_py, &NAME_NAME, b"name");
                let line_key = intern_static_name(_py, &LINE_NAME, b"line");
                let end_lineno_key = intern_static_name(_py, &END_LINENO_NAME, b"end_lineno");
                let colno_key = intern_static_name(_py, &COLNO_NAME, b"colno");
                let end_colno_key = intern_static_name(_py, &END_COLNO_NAME, b"end_colno");
                let filename_bits = dict_get_in_place(_py, entry_ptr, filename_key)?;
                let lineno_bits = dict_get_in_place(_py, entry_ptr, lineno_key)?;
                let filename = format_obj_str(_py, obj_from_bits(filename_bits));
                let lineno = to_i64(obj_from_bits(lineno_bits)).unwrap_or(0);
                let name = dict_get_in_place(_py, entry_ptr, name_key)
                    .map(|bits| format_obj_str(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| "<module>".to_string());
                let line = dict_get_in_place(_py, entry_ptr, line_key)
                    .map(|bits| format_obj_str(_py, obj_from_bits(bits)))
                    .unwrap_or_else(|| traceback_source_line_native(_py, &filename, lineno));
                let (mut colno, mut end_colno) = traceback_infer_column_offsets(&line);
                if let Some(value) = dict_get_in_place(_py, entry_ptr, colno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                {
                    colno = value;
                }
                if let Some(value) = dict_get_in_place(_py, entry_ptr, end_colno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                {
                    end_colno = value;
                }
                if !line.is_empty() && (colno < 0 || end_colno <= colno) {
                    let inferred = traceback_infer_column_offsets(&line);
                    colno = inferred.0;
                    end_colno = inferred.1;
                }
                let end_lineno = dict_get_in_place(_py, entry_ptr, end_lineno_key)
                    .and_then(|bits| to_i64(obj_from_bits(bits)))
                    .unwrap_or(lineno);
                return Some(TracebackPayloadFrame {
                    filename,
                    lineno,
                    end_lineno,
                    colno,
                    end_colno,
                    name,
                    line,
                });
            }
        }
    }

    if let Some(value) = string_obj_to_owned(entry_obj) {
        let (filename, name) = traceback_split_molt_symbol(&value);
        return Some(TracebackPayloadFrame {
            filename,
            lineno: 0,
            end_lineno: 0,
            colno: 0,
            end_colno: 0,
            name,
            line: String::new(),
        });
    }

    let mut from_tb = traceback_payload_from_traceback(_py, entry_bits, Some(1));
    if let Some(frame) = from_tb.pop() {
        return Some(frame);
    }
    let mut from_frame = traceback_payload_from_frame_chain(_py, entry_bits, Some(1));
    from_frame.pop()
}

fn traceback_payload_from_entries(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    let Some(source_ptr) = obj_from_bits(source_bits).as_ptr() else {
        return Vec::new();
    };
    let type_id = unsafe { object_type_id(source_ptr) };
    if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
        return Vec::new();
    }
    let elems: Vec<u64> = unsafe { seq_vec_ref(source_ptr).to_vec() };
    let mut out: Vec<TracebackPayloadFrame> = Vec::new();
    for bits in elems {
        if let Some(frame) = traceback_payload_from_entry(_py, bits) {
            out.push(frame);
            if let Some(max) = limit
                && out.len() >= max
            {
                break;
            }
        }
    }
    out
}

fn traceback_payload_from_source(
    _py: &PyToken<'_>,
    source_bits: u64,
    limit: Option<usize>,
) -> Vec<TracebackPayloadFrame> {
    if obj_from_bits(source_bits).is_none() {
        return Vec::new();
    }
    let from_entries = traceback_payload_from_entries(_py, source_bits, limit);
    if !from_entries.is_empty() {
        return from_entries;
    }
    let from_tb = traceback_payload_from_traceback(_py, source_bits, limit);
    if !from_tb.is_empty() {
        return from_tb;
    }
    let from_frame = traceback_payload_from_frame_chain(_py, source_bits, limit);
    if !from_frame.is_empty() {
        return from_frame;
    }
    if let Some(frame) = traceback_payload_from_entry(_py, source_bits) {
        return vec![frame];
    }
    Vec::new()
}

fn traceback_payload_to_list(_py: &PyToken<'_>, payload: &[TracebackPayloadFrame]) -> u64 {
    let mut tuples: Vec<u64> = Vec::new();
    for frame in payload {
        let filename_ptr = alloc_string(_py, frame.filename.as_bytes());
        if filename_ptr.is_null() {
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let name_ptr = alloc_string(_py, frame.name.as_bytes());
        if name_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let line_ptr = alloc_string(_py, frame.line.as_bytes());
        if line_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
            dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
        let lineno_bits = MoltObject::from_int(frame.lineno).bits();
        let end_lineno_bits = MoltObject::from_int(frame.end_lineno).bits();
        let colno_bits = MoltObject::from_int(frame.colno).bits();
        let end_colno_bits = MoltObject::from_int(frame.end_colno).bits();
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let line_bits = MoltObject::from_ptr(line_ptr).bits();
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                filename_bits,
                lineno_bits,
                end_lineno_bits,
                colno_bits,
                end_colno_bits,
                name_bits,
                line_bits,
            ],
        );
        dec_ref_bits(_py, filename_bits);
        dec_ref_bits(_py, end_lineno_bits);
        dec_ref_bits(_py, colno_bits);
        dec_ref_bits(_py, end_colno_bits);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, line_bits);
        if tuple_ptr.is_null() {
            for bits in tuples {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        tuples.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list(_py, tuples.as_slice());
    for bits in tuples {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

fn traceback_payload_frame_source_lines(
    _py: &PyToken<'_>,
    frame: &TracebackPayloadFrame,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut first_line = frame.line.clone();
    let mut first_colno = frame.colno;
    let mut first_end_colno = frame.end_colno;
    if first_line.is_empty() {
        first_line = traceback_source_line_native(_py, &frame.filename, frame.lineno);
        if first_line.is_empty() {
            return lines;
        }
        if first_colno < 0 || first_end_colno <= first_colno {
            let (col, end_col) = traceback_infer_column_offsets(&first_line);
            first_colno = col;
            first_end_colno = end_col;
        }
    }

    let span_end = frame.end_lineno.max(frame.lineno);
    if span_end <= frame.lineno || frame.lineno <= 0 || (span_end - frame.lineno) > 64 {
        lines.push(format!("    {}\n", first_line));
        let caret = traceback_format_caret_line_native(&first_line, first_colno, first_end_colno);
        if !caret.is_empty() {
            lines.push(caret);
        }
        return lines;
    }

    for lineno in frame.lineno..=span_end {
        let text = if lineno == frame.lineno {
            first_line.clone()
        } else {
            traceback_source_line_native(_py, &frame.filename, lineno)
        };
        if text.is_empty() {
            continue;
        }
        lines.push(format!("    {}\n", text));

        let text_len = text.chars().count() as i64;
        if text_len <= 0 {
            continue;
        }
        let (trim_start, trim_end) = traceback_line_trim_bounds(&text).unwrap_or((0, text_len));
        let (start, end) = if lineno == frame.lineno {
            let start = if first_colno >= 0 {
                first_colno
            } else {
                trim_start
            };
            let end = if lineno == span_end {
                if first_end_colno > start {
                    first_end_colno
                } else {
                    trim_end
                }
            } else {
                trim_end
            };
            (start, end)
        } else if lineno == span_end {
            let end = if frame.end_colno > trim_start {
                frame.end_colno
            } else {
                trim_end
            };
            (trim_start, end)
        } else {
            (trim_start, trim_end)
        };
        let caret = traceback_format_caret_line_native(&text, start, end);
        if !caret.is_empty() {
            lines.push(caret);
        }
    }

    lines
}

fn traceback_payload_to_formatted_lines(
    _py: &PyToken<'_>,
    payload: &[TracebackPayloadFrame],
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for frame in payload {
        lines.push(format!(
            "  File \"{}\", line {}, in {}\n",
            frame.filename, frame.lineno, frame.name
        ));
        lines.extend(traceback_payload_frame_source_lines(_py, frame));
    }
    lines
}

fn traceback_exception_components_payload(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
) -> Result<u64, u64> {
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "value must be an exception instance",
        ));
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_EXCEPTION {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "value must be an exception instance",
            ));
        }
    }
    let tb_bits = traceback_exception_trace_bits(value_bits);
    let payload = traceback_payload_from_source(_py, tb_bits, limit);
    let frames_bits = traceback_payload_to_list(_py, &payload);
    if obj_from_bits(frames_bits).is_none() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let (cause_bits, context_bits, suppress_context) = unsafe {
        let cause = exception_cause_bits(value_ptr);
        let context = exception_context_bits(value_ptr);
        let suppress = is_truthy(_py, obj_from_bits(exception_suppress_bits(value_ptr)));
        (cause, context, suppress)
    };
    if !obj_from_bits(cause_bits).is_none() {
        inc_ref_bits(_py, cause_bits);
    }
    if !obj_from_bits(context_bits).is_none() {
        inc_ref_bits(_py, context_bits);
    }
    let suppress_bits = MoltObject::from_bool(suppress_context).bits();
    let tuple_ptr = alloc_tuple(_py, &[frames_bits, cause_bits, context_bits, suppress_bits]);
    dec_ref_bits(_py, frames_bits);
    if !obj_from_bits(cause_bits).is_none() {
        dec_ref_bits(_py, cause_bits);
    }
    if !obj_from_bits(context_bits).is_none() {
        dec_ref_bits(_py, context_bits);
    }
    if tuple_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

fn traceback_exception_chain_collect(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
    nodes: &mut Vec<TracebackExceptionChainNode>,
    seen: &mut HashMap<u64, usize>,
    depth: usize,
) -> Result<usize, u64> {
    if depth > 1024 {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "traceback exception chain recursion too deep",
        ));
    }
    if let Some(index) = seen.get(&value_bits) {
        return Ok(*index);
    }
    let Some(value_ptr) = obj_from_bits(value_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "value must be an exception instance",
        ));
    };
    unsafe {
        if object_type_id(value_ptr) != TYPE_ID_EXCEPTION {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "value must be an exception instance",
            ));
        }
    }
    let tb_bits = traceback_exception_trace_bits(value_bits);
    let frames = traceback_payload_from_source(_py, tb_bits, limit);
    let (cause_bits, context_bits, suppress_context) = unsafe {
        let cause = exception_cause_bits(value_ptr);
        let context = exception_context_bits(value_ptr);
        let suppress = is_truthy(_py, obj_from_bits(exception_suppress_bits(value_ptr)));
        (cause, context, suppress)
    };
    let index = nodes.len();
    seen.insert(value_bits, index);
    nodes.push(TracebackExceptionChainNode {
        value_bits,
        frames,
        suppress_context,
        cause_index: None,
        context_index: None,
    });

    if !obj_from_bits(cause_bits).is_none() {
        let Some(cause_ptr) = obj_from_bits(cause_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "exception __cause__ must be an exception instance or None",
            ));
        };
        unsafe {
            if object_type_id(cause_ptr) != TYPE_ID_EXCEPTION {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "exception __cause__ must be an exception instance or None",
                ));
            }
        }
        let cause_index =
            traceback_exception_chain_collect(_py, cause_bits, limit, nodes, seen, depth + 1)?;
        nodes[index].cause_index = Some(cause_index);
    }

    if !suppress_context && !obj_from_bits(context_bits).is_none() {
        let Some(context_ptr) = obj_from_bits(context_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "exception __context__ must be an exception instance or None",
            ));
        };
        unsafe {
            if object_type_id(context_ptr) != TYPE_ID_EXCEPTION {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "exception __context__ must be an exception instance or None",
                ));
            }
        }
        let context_index =
            traceback_exception_chain_collect(_py, context_bits, limit, nodes, seen, depth + 1)?;
        nodes[index].context_index = Some(context_index);
    }

    Ok(index)
}

fn traceback_exception_chain_payload_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
    limit: Option<usize>,
) -> Result<u64, u64> {
    let mut nodes: Vec<TracebackExceptionChainNode> = Vec::new();
    let mut seen: HashMap<u64, usize> = HashMap::new();
    traceback_exception_chain_collect(_py, value_bits, limit, &mut nodes, &mut seen, 0)?;

    let mut tuple_bits: Vec<u64> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let frames_bits = traceback_payload_to_list(_py, &node.frames);
        if obj_from_bits(frames_bits).is_none() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        inc_ref_bits(_py, node.value_bits);
        let suppress_bits = MoltObject::from_bool(node.suppress_context).bits();
        let cause_bits = match node.cause_index {
            Some(index) => int_bits_from_i64(_py, index as i64),
            None => MoltObject::none().bits(),
        };
        let context_bits = match node.context_index {
            Some(index) => int_bits_from_i64(_py, index as i64),
            None => MoltObject::none().bits(),
        };
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                node.value_bits,
                frames_bits,
                suppress_bits,
                cause_bits,
                context_bits,
            ],
        );
        dec_ref_bits(_py, node.value_bits);
        dec_ref_bits(_py, frames_bits);
        if node.cause_index.is_some() {
            dec_ref_bits(_py, cause_bits);
        }
        if node.context_index.is_some() {
            dec_ref_bits(_py, context_bits);
        }
        if tuple_ptr.is_null() {
            for bits in tuple_bits {
                dec_ref_bits(_py, bits);
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        tuple_bits.push(MoltObject::from_ptr(tuple_ptr).bits());
    }

    let list_ptr = alloc_list(_py, tuple_bits.as_slice());
    for bits in tuple_bits {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(list_ptr).bits())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_payload(source_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let payload = traceback_payload_from_source(_py, source_bits, limit);
        traceback_payload_to_list(_py, &payload)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_exception_components(value_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        match traceback_exception_components_payload(_py, value_bits, limit) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_exception_chain_payload(value_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        match traceback_exception_chain_payload_bits(_py, value_bits, limit) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_source_line(filename_bits: u64, lineno_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(filename) = string_obj_to_owned(obj_from_bits(filename_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "filename must be str");
        };
        let Some(lineno) = to_i64(obj_from_bits(lineno_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "lineno must be int");
        };
        let text = traceback_source_line_native(_py, &filename, lineno);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_infer_col_offsets(line_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(line) = string_obj_to_owned(obj_from_bits(line_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "line must be str");
        };
        let (colno, end_colno) = traceback_infer_column_offsets(&line);
        let colno_bits = MoltObject::from_int(colno).bits();
        let end_colno_bits = MoltObject::from_int(end_colno).bits();
        let tuple_ptr = alloc_tuple(_py, &[colno_bits, end_colno_bits]);
        dec_ref_bits(_py, colno_bits);
        dec_ref_bits(_py, end_colno_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_caret_line(
    line_bits: u64,
    colno_bits: u64,
    end_colno_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(line) = string_obj_to_owned(obj_from_bits(line_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "line must be str");
        };
        let Some(colno) = to_i64(obj_from_bits(colno_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "colno must be int");
        };
        let Some(end_colno) = to_i64(obj_from_bits(end_colno_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end_colno must be int");
        };
        let out = traceback_format_caret_line_native(&line, colno, end_colno);
        let ptr = alloc_string(_py, out.as_bytes());
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_exception_only(exc_type_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let line = traceback_format_exception_only_line(_py, exc_type_bits, value_bits);
        traceback_lines_to_list(_py, &[line])
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_exception(
    exc_type_bits: u64,
    value_bits: u64,
    tb_bits: u64,
    limit_bits: u64,
    chain_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let chain = is_truthy(_py, obj_from_bits(chain_bits));
        let effective_exc_type_bits = if obj_from_bits(exc_type_bits).is_none() {
            traceback_exception_type_bits(_py, value_bits)
        } else {
            exc_type_bits
        };
        let effective_tb_bits = if obj_from_bits(tb_bits).is_none() {
            traceback_exception_trace_bits(value_bits)
        } else {
            tb_bits
        };
        let mut seen: HashSet<u64> = HashSet::new();
        let mut lines: Vec<String> = Vec::new();
        traceback_append_exception_chain_lines(
            _py,
            effective_exc_type_bits,
            value_bits,
            effective_tb_bits,
            limit,
            chain,
            &mut seen,
            &mut lines,
        );
        traceback_lines_to_list(_py, &lines)
    })
}

/// `traceback.format_exc(limit=None)` — format the current exception as a single
/// string.  Equivalent to `"".join(traceback.format_exception(*sys.exc_info()))`.
/// Returns the formatted string, or `"NoneType: None\n"` if no exception is active.
#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_exc(limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let exc_bits_opt = exception_last_bits_noinc(_py);
        let value_bits = match exc_bits_opt {
            Some(bits) => bits,
            None => {
                // No current exception — return "NoneType: None\n"
                let s = "NoneType: None\n";
                let ptr = alloc_string(_py, s.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        };
        let exc_type_bits = traceback_exception_type_bits(_py, value_bits);
        let tb_bits = traceback_exception_trace_bits(value_bits);
        let mut seen: HashSet<u64> = HashSet::new();
        let mut lines: Vec<String> = Vec::new();
        traceback_append_exception_chain_lines(
            _py,
            exc_type_bits,
            value_bits,
            tb_bits,
            limit,
            true, // chain
            &mut seen,
            &mut lines,
        );
        // Join all lines into a single string
        let joined = lines.join("");
        let ptr = alloc_string(_py, joined.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_tb(tb_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let mut lines: Vec<String> = Vec::new();
        for (filename, line, name) in traceback_frames(_py, tb_bits, limit) {
            lines.push(format!("  File \"{filename}\", line {line}, in {name}\n"));
        }
        traceback_lines_to_list(_py, &lines)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_format_stack(source_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let payload = traceback_payload_from_source(_py, source_bits, limit);
        let lines = traceback_payload_to_formatted_lines(_py, &payload);
        traceback_lines_to_list(_py, &lines)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_extract_tb(tb_bits: u64, limit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let limit = match traceback_limit_from_bits(_py, limit_bits) {
            Ok(limit) => limit,
            Err(bits) => return bits,
        };
        let mut tuples: Vec<u64> = Vec::new();
        for (filename, lineno, name) in traceback_frames(_py, tb_bits, limit) {
            let line_text = traceback_source_line_native(_py, &filename, lineno);
            let (colno, end_colno) = traceback_infer_column_offsets(&line_text);
            let end_lineno = lineno;
            let filename_ptr = alloc_string(_py, filename.as_bytes());
            if filename_ptr.is_null() {
                for bits in tuples {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
                for bits in tuples {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let line_ptr = alloc_string(_py, line_text.as_bytes());
            if line_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(filename_ptr).bits());
                dec_ref_bits(_py, MoltObject::from_ptr(name_ptr).bits());
                for bits in tuples {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let filename_bits = MoltObject::from_ptr(filename_ptr).bits();
            let lineno_bits = MoltObject::from_int(lineno).bits();
            let end_lineno_bits = MoltObject::from_int(end_lineno).bits();
            let colno_bits = MoltObject::from_int(colno).bits();
            let end_colno_bits = MoltObject::from_int(end_colno).bits();
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let line_bits = MoltObject::from_ptr(line_ptr).bits();
            let tuple_ptr = alloc_tuple(
                _py,
                &[
                    filename_bits,
                    lineno_bits,
                    end_lineno_bits,
                    colno_bits,
                    end_colno_bits,
                    name_bits,
                    line_bits,
                ],
            );
            dec_ref_bits(_py, filename_bits);
            dec_ref_bits(_py, end_lineno_bits);
            dec_ref_bits(_py, colno_bits);
            dec_ref_bits(_py, end_colno_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, line_bits);
            if tuple_ptr.is_null() {
                for bits in tuples {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            tuples.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list(_py, tuples.as_slice());
        for bits in tuples {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_guard_enter() -> i64 {
    crate::with_gil_entry!(_py, {
        if recursion_guard_enter() {
            1
        } else {
            raise_exception::<i64>(_py, "RecursionError", "maximum recursion depth exceeded")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_guard_exit() {
    crate::with_gil_entry!(_py, {
        recursion_guard_exit();
    })
}

/// Lightweight recursion guard for direct calls to known functions.
/// Uses global atomics only — no TLS access on the hot path.
/// Returns 1 on success, 0 if the recursion limit is exceeded (caller must
/// handle the error).
#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_enter_fast() -> i64 {
    if crate::state::recursion::recursion_guard_enter_fast() { 1 } else { 0 }
}

/// Lightweight recursion guard exit — uses global atomics only.
#[unsafe(no_mangle)]
pub extern "C" fn molt_recursion_exit_fast() {
    crate::state::recursion::recursion_guard_exit_fast();
}

/// Cold-path: raise RecursionError. Only called when molt_recursion_enter_fast
/// returns 0. Acquires the GIL to create the exception object.
#[unsafe(no_mangle)]
#[cold]
pub extern "C" fn molt_raise_recursion_error() -> u64 {
    // Sync the fast global depth back to TLS before the GIL-holding code
    // reads it (traceback formatting, etc.).
    crate::state::recursion::sync_fast_depth_to_tls();
    crate::with_gil_entry!(_py, {
        raise_exception::<u64>(_py, "RecursionError", "maximum recursion depth exceeded")
    })
}

