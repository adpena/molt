// Re-export iter impl functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_iter::{
    enumerate_new_impl, filter_new_impl, map_new_impl, reversed_new_impl, zip_new_impl,
};

// Re-export arith functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_arith::repeat_sequence;

// Re-export compare functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_compare::{CompareOutcome, compare_objects, compare_type_error};

// Re-export format functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_format::{
    FormatSpec, decode_string_list, decode_value_list, format_float_with_spec, format_obj,
    format_obj_str, format_with_spec, parse_format_spec, string_obj_to_owned,
};

// Re-export hash functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_hash::{
    HashSecret, ensure_hashable, fatal_hash_seed, hash_bits, hash_bits_signed, hash_int,
    hash_pointer, hash_slice_bits, hash_string_bytes,
};

// Re-export encoding functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_encoding::{
    DecodeTextError, EncodeError, decode_bytes_text, decode_error_byte, decode_error_range,
    encode_error_reason, encode_string_with_errors, encoding_kind_name, is_surrogate,
    normalize_encoding, unicode_escape,
};

use crate::object::layout::{range_start_bits, range_step_bits, range_stop_bits};
use crate::object::ops_bytes::{
    BytesCtorKind, bytes_ascii_space, bytes_item_to_u8, collect_bytearray_assign_bytes,
};
use crate::state::runtime_state::PythonVersionInfo;
use crate::*;
use memchr::{memchr, memmem};
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::CStr;
#[cfg(not(target_arch = "wasm32"))]
use std::ffi::CString;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::OnceLock;

use super::ops_string::{push_wtf8_codepoint, utf8_char_to_byte_index_cached, wtf8_codepoint_at};

#[cfg(all(target_arch = "wasm32", not(feature = "wasm_freestanding")))]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    fn args_sizes_get(argc: *mut u32, argv_buf_size: *mut u32) -> u16;
    fn args_get(argv: *mut *mut u8, argv_buf: *mut u8) -> u16;
}

#[cfg(all(target_arch = "wasm32", not(feature = "wasm_freestanding")))]
fn collect_wasi_argv_bytes() -> Option<Vec<Vec<u8>>> {
    unsafe {
        let mut argc = 0u32;
        let mut argv_buf_size = 0u32;
        if args_sizes_get(&mut argc, &mut argv_buf_size) != 0 {
            return None;
        }
        let argc_usize = argc as usize;
        let mut argv_ptrs = vec![std::ptr::null_mut(); argc_usize];
        let mut argv_buf = vec![0u8; argv_buf_size as usize];
        let argv_buf_ptr = if argv_buf.is_empty() {
            std::ptr::null_mut()
        } else {
            argv_buf.as_mut_ptr()
        };
        if args_get(argv_ptrs.as_mut_ptr(), argv_buf_ptr) != 0 {
            return None;
        }
        let base = argv_buf.as_ptr() as usize;
        let end = base.saturating_add(argv_buf.len());
        let mut out = Vec::with_capacity(argc_usize);
        for ptr in argv_ptrs {
            if ptr.is_null() {
                return None;
            }
            let addr = ptr as usize;
            if addr < base || addr >= end {
                return None;
            }
            let cstr = CStr::from_ptr(ptr.cast::<i8>());
            out.push(cstr.to_bytes().to_vec());
        }
        Some(out)
    }
}

#[cfg(not(all(target_arch = "wasm32", not(feature = "wasm_freestanding"))))]
fn collect_wasi_argv_bytes() -> Option<Vec<Vec<u8>>> {
    None
}

#[inline]
fn unicode_range_contains(ranges: &[(u32, u32)], code: u32) -> bool {
    let mut lo = 0usize;
    let mut hi = ranges.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (start, end) = ranges[mid];
        if code < start {
            hi = mid;
        } else if code > end {
            lo = mid + 1;
        } else {
            return true;
        }
    }
    false
}

pub(crate) mod unicode_digit_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_digit_ranges.rs"));

    pub(crate) fn is_digit(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_DIGIT_RANGES, code)
    }
}

pub(crate) mod unicode_decimal_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_decimal_ranges.rs"));

    pub(crate) fn is_decimal(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_DECIMAL_RANGES, code)
    }
}

pub(crate) mod unicode_numeric_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_numeric_ranges.rs"));

    pub(crate) fn is_numeric(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_NUMERIC_RANGES, code)
    }
}

pub(crate) mod unicode_space_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_space_ranges.rs"));

    pub(crate) fn is_space(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_SPACE_RANGES, code)
    }
}

pub(crate) mod unicode_printable_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_printable_ranges.rs"));

    pub(crate) fn is_printable(code: u32) -> bool {
        super::unicode_range_contains(UNICODE_PRINTABLE_RANGES, code)
    }
}

pub(crate) mod unicode_titlecase_table {
    include!(concat!(env!("OUT_DIR"), "/unicode_titlecase_map.rs"));

    pub(crate) fn titlecase(code: u32) -> Option<&'static str> {
        let idx = UNICODE_TITLECASE_MAP
            .binary_search_by_key(&code, |entry| entry.0)
            .ok()?;
        Some(UNICODE_TITLECASE_MAP[idx].1)
    }
}

pub(crate) fn slice_bounds_from_args(
    _py: &PyToken<'_>,
    start_bits: u64,
    end_bits: u64,
    has_start: bool,
    has_end: bool,
    len: i64,
) -> (i64, i64, i64) {
    let msg = "slice indices must be integers or None or have an __index__ method";
    let start_obj = if has_start {
        Some(obj_from_bits(start_bits))
    } else {
        None
    };
    let end_obj = if has_end {
        Some(obj_from_bits(end_bits))
    } else {
        None
    };
    let mut start = if let Some(obj) = start_obj {
        if obj.is_none() {
            0
        } else {
            index_i64_from_obj(_py, start_bits, msg)
        }
    } else {
        0
    };
    let mut end = if let Some(obj) = end_obj {
        if obj.is_none() {
            len
        } else {
            index_i64_from_obj(_py, end_bits, msg)
        }
    } else {
        len
    };
    if start < 0 {
        start += len;
    }
    if end < 0 {
        end += len;
    }
    let start_raw = start;
    if start < 0 {
        start = 0;
    }
    if end < 0 {
        end = 0;
    }
    if start > len {
        start = len;
    }
    if end > len {
        end = len;
    }
    (start, end, start_raw)
}

pub(crate) fn slice_match(
    slice: &[u8],
    needle: &[u8],
    start_raw: i64,
    total: i64,
    suffix: bool,
) -> bool {
    if needle.is_empty() {
        return start_raw <= total;
    }
    if suffix {
        slice.ends_with(needle)
    } else {
        slice.starts_with(needle)
    }
}

pub(super) fn range_components_bigint(ptr: *mut u8) -> Option<(BigInt, BigInt, BigInt)> {
    unsafe {
        let start_obj = obj_from_bits(range_start_bits(ptr));
        let stop_obj = obj_from_bits(range_stop_bits(ptr));
        let step_obj = obj_from_bits(range_step_bits(ptr));
        let start = to_bigint(start_obj)?;
        let stop = to_bigint(stop_obj)?;
        let step = to_bigint(step_obj)?;
        Some((start, stop, step))
    }
}

pub(super) fn range_components_i64(ptr: *mut u8) -> Option<(i64, i64, i64)> {
    unsafe {
        let start = to_i64(obj_from_bits(range_start_bits(ptr)))?;
        let stop = to_i64(obj_from_bits(range_stop_bits(ptr)))?;
        let step = to_i64(obj_from_bits(range_step_bits(ptr)))?;
        if step == 0 {
            return None;
        }
        Some((start, stop, step))
    }
}

pub(super) fn range_len_i128(start: i64, stop: i64, step: i64) -> i128 {
    if step == 0 {
        return 0;
    }
    let start_i = start as i128;
    let stop_i = stop as i128;
    let step_i = step as i128;
    if step_i > 0 {
        if start_i >= stop_i {
            return 0;
        }
        let span = stop_i - start_i - 1;
        return 1 + span / step_i;
    }
    if start_i <= stop_i {
        return 0;
    }
    let step_abs = -step_i;
    let span = start_i - stop_i - 1;
    1 + span / step_abs
}

pub(super) fn range_value_at_index_i64(start: i64, stop: i64, step: i64, idx: i128) -> Option<i64> {
    if idx < 0 {
        return None;
    }
    let step_i = step as i128;
    let val = (start as i128).checked_add(step_i.checked_mul(idx)?)?;
    if step_i > 0 {
        if val >= stop as i128 {
            return None;
        }
    } else if step_i < 0 {
        if val <= stop as i128 {
            return None;
        }
    } else {
        return None;
    }
    i64::try_from(val).ok()
}

pub(super) fn range_index_for_candidate(
    start: &BigInt,
    stop: &BigInt,
    step: &BigInt,
    val: &BigInt,
) -> Option<BigInt> {
    if step.is_zero() {
        return None;
    }
    let in_range = if step.is_positive() {
        val >= start && val < stop
    } else {
        val <= start && val > stop
    };
    if !in_range {
        return None;
    }
    let offset = val - start;
    let step_abs = if step.is_negative() {
        -step
    } else {
        step.clone()
    };
    if !offset.mod_floor(&step_abs).is_zero() {
        return None;
    }
    Some(offset / step)
}

pub(super) fn range_lookup_candidate(_py: &PyToken<'_>, val_bits: u64) -> Option<BigInt> {
    let val = obj_from_bits(val_bits);
    if let Some(f) = val.as_float() {
        if !f.is_finite() || f.fract() != 0.0 {
            return None;
        }
        return Some(bigint_from_f64_trunc(f));
    }
    let type_err = format!(
        "'{}' object cannot be interpreted as an integer",
        type_name(_py, val)
    );
    let candidate = index_bigint_from_obj(_py, val_bits, &type_err);
    if candidate.is_none() && exception_pending(_py) {
        molt_exception_clear();
    }
    candidate
}

#[inline]
fn debug_index_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_INDEX").as_deref() == Ok("1"))
}

#[inline]
fn debug_index_list_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_INDEX_LIST").as_deref() == Ok("1"))
}

#[inline]
fn debug_store_index_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_STORE_INDEX").as_deref() == Ok("1"))
}

pub(super) fn range_len_bigint(start: &BigInt, stop: &BigInt, step: &BigInt) -> BigInt {
    if step.is_zero() {
        return BigInt::from(0);
    }
    if step.is_positive() {
        if start >= stop {
            return BigInt::from(0);
        }
        let span = stop - start - 1;
        return BigInt::from(1) + span / step;
    }
    if start <= stop {
        return BigInt::from(0);
    }
    let step_abs = -step;
    let span = start - stop - 1;
    BigInt::from(1) + span / step_abs
}

pub(super) fn alloc_range_from_bigints(
    _py: &PyToken<'_>,
    start: BigInt,
    stop: BigInt,
    step: BigInt,
) -> u64 {
    let start_bits = int_bits_from_bigint(_py, start);
    let stop_bits = int_bits_from_bigint(_py, stop);
    let step_bits = int_bits_from_bigint(_py, step);
    let ptr = alloc_range(_py, start_bits, stop_bits, step_bits);
    let range_bits = if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    };
    dec_ref_bits(_py, start_bits);
    dec_ref_bits(_py, stop_bits);
    dec_ref_bits(_py, step_bits);
    range_bits
}

// --- NaN-boxed ops ---

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

// ---------------------------------------------------------------------------
// SIMD-accelerated float sum: SSE2 (2×f64), AVX2 (4×f64), NEON (2×f64)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// SIMD-accelerated sequence element identity comparison
// Batch-compare NaN-boxed u64 arrays to quickly find first mismatch index.
// ---------------------------------------------------------------------------

/// Compare two u64 slices for element-wise bitwise equality using SIMD.
/// Returns the index of the first mismatch, or `len` if all elements match.
/// This is an identity check (bits ==), not semantic equality (obj_eq).
pub(super) fn simd_find_first_mismatch(lhs: &[u64], rhs: &[u64]) -> usize {
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

#[cfg(target_arch = "wasm32")]
pub(crate) unsafe fn sum_ints_simd_wasm32(elems: &[u64], acc: i64) -> Option<i64> {
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

#[cfg(target_arch = "wasm32")]
pub(crate) unsafe fn min_ints_simd_wasm32(elems: &[u64], acc: i64) -> Option<i64> {
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

#[cfg(target_arch = "wasm32")]
pub(crate) unsafe fn max_ints_simd_wasm32(elems: &[u64], acc: i64) -> Option<i64> {
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

#[cfg(target_arch = "wasm32")]
pub(crate) unsafe fn sum_ints_trusted_simd_wasm32(elems: &[u64], acc: i64) -> i64 {
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

// Re-export slice indexing helpers from ops_sys (authoritative copy).
pub(super) use super::ops_sys::slice_error;
use super::ops_sys::{collect_iterable_values, collect_slice_indices, normalize_slice_indices};

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

/// Fast len for known-list values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_list(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_LIST || tid == TYPE_ID_LIST_INT {
                    return MoltObject::from_int(list_len(ptr) as i64).bits();
                }
            }
        }
        molt_len(bits)
    })
}

/// Fast len for known-str values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_str(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let count = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                    return MoltObject::from_int(count).bits();
                }
            }
        }
        molt_len(bits)
    })
}

/// Fast len for known-dict values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_dict(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    return MoltObject::from_int(dict_len(dict_ptr) as i64).bits();
                }
            }
        }
        molt_len(bits)
    })
}

/// Fast len for known-tuple values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_tuple(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TUPLE {
                    return MoltObject::from_int(tuple_len(ptr) as i64).bits();
                }
            }
        }
        molt_len(bits)
    })
}

/// Fast len for known-set values. Single type check, no 18-type dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_len_set(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_SET || tid == TYPE_ID_FROZENSET {
                    return MoltObject::from_int(set_len(ptr) as i64).bits();
                }
            }
        }
        molt_len(bits)
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_chr(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Fast path: inline small integer in valid codepoint range.
        // Avoids BigInt allocation, error-message formatting, and (for ASCII)
        // goes straight to the interned single-char cache.
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            if i < 0 || i > 0x10FFFF {
                return raise_exception::<_>(_py, "ValueError", "chr() arg not in range(0x110000)");
            }
            let code = i as u32;
            let mut out_bytes = Vec::with_capacity(4);
            push_wtf8_codepoint(&mut out_bytes, code);
            let out = alloc_string(_py, &out_bytes);
            if out.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(out).bits();
        }

        // Slow path: BigInt / __index__ protocol.
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

// Re-export GC state helpers from ops_sys (authoritative copy).
use super::ops_sys::{gc_int_arg, gc_state};

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
        let mut args_guard = runtime_state(_py).argv.lock().unwrap();
        if args_guard.is_empty() {
            if let Some(wasi_args) = collect_wasi_argv_bytes() {
                if !wasi_args.is_empty() {
                    *args_guard = wasi_args;
                }
            }
        }
        // On WASM, molt_set_argv may not have been called (no C main stub).
        // Fall back to std::env::args() so WASI args are still visible.
        let env_args_storage;
        let args: &Vec<Vec<u8>> = if args_guard.is_empty() {
            env_args_storage = std::env::args().map(|s| s.into_bytes()).collect::<Vec<_>>();
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

// Re-export sys/time helpers from ops_sys (authoritative copy).
use super::ops_sys::{
    DEFAULT_SYS_FLAGS_INT_MAX_STR_DIGITS, alloc_sys_version_info_tuple, current_sys_version_info,
    default_sys_version_info, dict_set_bytes_key, env_flag_bool, env_flag_level,
    env_non_negative_i64, env_sys_version_info, format_sys_version, sys_abiflags,
    sys_api_version, sys_cache_tag, sys_flags_hash_randomization, sys_hexversion_from_info,
    sys_implementation_name, trace_sys_version,
};

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

// Re-export process_time_duration from ops_sys (authoritative copy).
use super::ops_sys::process_time_duration;

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

// Re-export time helpers from ops_sys (authoritative copy).
use super::ops_sys::{
    days_from_civil, parse_time_seconds, parse_time_tuple,
    time_parts_to_tuple,
};
#[cfg(not(target_arch = "wasm32"))]
use super::ops_sys::{
    time_parts_from_tm, tm_from_time_parts,
};
#[cfg(target_arch = "wasm32")]
use super::ops_sys::{
    civil_from_days, day_of_year, is_leap_year, local_offset_west_wasm,
    time_parts_from_epoch_utc, timezone_west_wasm, tzname_wasm,
};

// parse_time_tuple imported from ops_sys above.

use super::ops_sys::asctime_from_parts;

// Re-export timezone/mktime helpers from ops_sys (authoritative copy).
#[cfg(not(target_arch = "wasm32"))]
use super::ops_sys::{
    altzone_native, daylight_native, mktime_native,
    timezone_native, tzname_native,
};
#[cfg(target_arch = "wasm32")]
use super::ops_sys::{
    altzone_wasm, daylight_wasm, mktime_wasm, sample_offset_west_wasm,
};

// Re-export traceback helpers from ops_sys (authoritative copy).
use super::ops_sys::{
    traceback_append_exception_chain_lines,
    traceback_exception_chain_payload_bits, traceback_exception_components_payload,
    traceback_exception_trace_bits, traceback_exception_type_bits,
    traceback_format_caret_line_native, traceback_format_exception_only_line, traceback_frames,
    traceback_infer_column_offsets, traceback_limit_from_bits,
    traceback_lines_to_list, traceback_payload_from_source, traceback_payload_to_formatted_lines,
    traceback_payload_to_list, traceback_source_line_native,
};

use super::ops_sys::parse_mktime_tuple;

use super::ops_sys::parse_timegm_tuple;

#[cfg(not(target_arch = "wasm32"))]
use super::ops_sys::localtime_tm;

#[cfg(not(target_arch = "wasm32"))]
use super::ops_sys::gmtime_tm;

#[cfg(target_arch = "wasm32")]
use super::ops_sys::strftime_wasm;

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









#[allow(clippy::too_many_arguments)]


























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
    if crate::state::recursion::recursion_guard_enter_fast() {
        1
    } else {
        0
    }
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

#[unsafe(no_mangle)]

pub(crate) fn parse_codec_arg(
    _py: &PyToken<'_>,
    bits: u64,
    func_name: &str,
    arg_name: &str,
    default: &str,
) -> Option<String> {
    if bits == missing_bits(_py) {
        return Some(default.to_string());
    }
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        let msg = format!("{func_name}() argument '{arg_name}' must be str, not None");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let Some(text) = string_obj_to_owned(obj) else {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = format!("{func_name}() argument '{arg_name}' must be str, not '{type_name}'");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    Some(text)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_to_bytes(
    int_bits: u64,
    length_bits: u64,
    byteorder_bits: u64,
    signed_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let length_type = class_name_for_error(type_of_bits(_py, length_bits));
        let length_msg = format!(
            "'{}' object cannot be interpreted as an integer",
            length_type
        );
        let length = index_i64_from_obj(_py, length_bits, &length_msg);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if length < 0 {
            return raise_exception::<_>(_py, "ValueError", "length argument must be non-negative");
        }
        let len = match usize::try_from(length) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(_py, "OverflowError", "length too large");
            }
        };
        let byteorder_obj = obj_from_bits(byteorder_bits);
        let Some(byteorder) = string_obj_to_owned(byteorder_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, byteorder_bits));
            let msg = format!(
                "to_bytes() argument 'byteorder' must be str, not {}",
                type_name
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let byteorder_norm = byteorder.to_ascii_lowercase();
        let is_little = match byteorder_norm.as_str() {
            "little" => true,
            "big" => false,
            _ => {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "byteorder must be either 'little' or 'big'",
                );
            }
        };
        let signed = is_truthy(_py, obj_from_bits(signed_bits));
        let value_obj = obj_from_bits(int_bits);
        let Some(value) = to_bigint(value_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, int_bits));
            let msg = format!(
                "descriptor 'to_bytes' requires a 'int' object but received '{}'",
                type_name
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        if !signed && value.sign() == Sign::Minus {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "can't convert negative int to unsigned",
            );
        }
        let mut bytes = if signed {
            value.to_signed_bytes_be()
        } else {
            value.to_bytes_be().1
        };
        if bytes.len() > len {
            return raise_exception::<_>(_py, "OverflowError", "int too big to convert");
        }
        if bytes.len() < len {
            let pad = if signed && value.sign() == Sign::Minus {
                0xFF
            } else {
                0x00
            };
            let mut out = vec![pad; len - bytes.len()];
            out.extend_from_slice(&bytes);
            bytes = out;
        }
        if is_little {
            bytes.reverse();
        }
        let ptr = alloc_bytes(_py, &bytes);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_from_bytes(
    class_bits: u64,
    bytes_bits: u64,
    byteorder_bits: u64,
    signed_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let byteorder_obj = obj_from_bits(byteorder_bits);
        let Some(byteorder) = string_obj_to_owned(byteorder_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, byteorder_bits));
            let msg = format!(
                "from_bytes() argument 'byteorder' must be str, not {}",
                type_name
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let byteorder_norm = byteorder.to_ascii_lowercase();
        let is_little = match byteorder_norm.as_str() {
            "little" => true,
            "big" => false,
            _ => {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "byteorder must be either 'little' or 'big'",
                );
            }
        };
        let signed = is_truthy(_py, obj_from_bits(signed_bits));
        let bytes_obj = obj_from_bits(bytes_bits);
        let Some(bytes_ptr) = bytes_obj.as_ptr() else {
            let type_name = class_name_for_error(type_of_bits(_py, bytes_bits));
            let msg = format!("cannot convert '{}' object to bytes", type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let Some(slice) = (unsafe { bytes_like_slice(bytes_ptr) }) else {
            let type_name = class_name_for_error(type_of_bits(_py, bytes_bits));
            let msg = format!("cannot convert '{}' object to bytes", type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let mut bytes = slice.to_vec();
        if is_little {
            bytes.reverse();
        }
        let value = if signed {
            BigInt::from_signed_bytes_be(&bytes)
        } else {
            BigInt::from_bytes_be(Sign::Plus, &bytes)
        };
        let int_bits = int_bits_from_bigint(_py, value);
        let builtins = builtin_classes(_py);
        if class_bits == builtins.int {
            return int_bits;
        }
        unsafe { call_callable1(_py, class_bits, int_bits) }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_index(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Fast path: dict[key] — skips exception_pending and type dispatch chain.
        if let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() {
            unsafe {
                if object_type_id(obj_ptr) == TYPE_ID_DICT {
                    if let Some(val) = dict_get_in_place(_py, obj_ptr, key_bits) {
                        if obj_from_bits(val).as_ptr().is_some() {
                            inc_ref_bits(_py, val);
                        }
                        return val;
                    }
                    return raise_key_error_with_key(_py, key_bits);
                }
                // list_int: flat i64 storage — delegate to specialized getitem
                if object_type_id(obj_ptr) == TYPE_ID_LIST_INT {
                    return molt_list_int_getitem(obj_bits, key_bits);
                }
            }
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_MEMORYVIEW {
                    let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                        Some(fmt) => fmt,
                        None => return MoltObject::none().bits(),
                    };
                    let owner_bits = memoryview_owner_bits(ptr);
                    let owner = obj_from_bits(owner_bits);
                    let owner_ptr = match owner.as_ptr() {
                        Some(ptr) => ptr,
                        None => return MoltObject::none().bits(),
                    };
                    let base = match bytes_like_slice_raw(owner_ptr) {
                        Some(slice) => slice,
                        None => return MoltObject::none().bits(),
                    };
                    let shape = memoryview_shape(ptr).unwrap_or(&[]);
                    let strides = memoryview_strides(ptr).unwrap_or(&[]);
                    let ndim = shape.len();
                    if ndim == 0 {
                        if let Some(tup_ptr) = key.as_ptr()
                            && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                        {
                            let elems = seq_vec_ref(tup_ptr);
                            if elems.is_empty() {
                                let val =
                                    memoryview_read_scalar(_py, base, memoryview_offset(ptr), fmt);
                                return val.unwrap_or_else(|| MoltObject::none().bits());
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "invalid indexing of 0-dim memory",
                        );
                    }
                    if let Some(tup_ptr) = key.as_ptr()
                        && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                    {
                        let elems = seq_vec_ref(tup_ptr);
                        let mut has_slice = false;
                        let mut all_slice = true;
                        for &elem_bits in elems.iter() {
                            let elem_obj = obj_from_bits(elem_bits);
                            if let Some(elem_ptr) = elem_obj.as_ptr() {
                                if object_type_id(elem_ptr) == TYPE_ID_SLICE {
                                    has_slice = true;
                                } else {
                                    all_slice = false;
                                }
                            } else {
                                all_slice = false;
                            }
                        }
                        if has_slice {
                            if all_slice {
                                return raise_exception::<_>(
                                    _py,
                                    "NotImplementedError",
                                    "multi-dimensional slicing is not implemented",
                                );
                            }
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "memoryview: invalid slice key",
                            );
                        }
                        if elems.len() < ndim {
                            return raise_exception::<_>(
                                _py,
                                "NotImplementedError",
                                "multi-dimensional sub-views are not implemented",
                            );
                        }
                        if elems.len() > ndim {
                            let msg = format!(
                                "cannot index {}-dimension view with {}-element tuple",
                                ndim,
                                elems.len()
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        if shape.len() != strides.len() {
                            return MoltObject::none().bits();
                        }
                        let mut pos = memoryview_offset(ptr);
                        for (dim, &elem_bits) in elems.iter().enumerate() {
                            let Some(idx) = index_i64_with_overflow(
                                _py,
                                elem_bits,
                                "memoryview: invalid slice key",
                                None,
                            ) else {
                                return MoltObject::none().bits();
                            };
                            let mut i = idx;
                            let dim_len = shape[dim];
                            let dim_len_i64 = dim_len as i64;
                            if i < 0 {
                                i += dim_len_i64;
                            }
                            if i < 0 || i >= dim_len_i64 {
                                let msg = format!("index out of bounds on dimension {}", dim + 1);
                                return raise_exception::<_>(_py, "IndexError", &msg);
                            }
                            pos = pos.saturating_add((i as isize).saturating_mul(strides[dim]));
                        }
                        if pos < 0 || pos + fmt.itemsize as isize > base.len() as isize {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "index out of bounds on dimension 1",
                            );
                        }
                        let val = memoryview_read_scalar(_py, base, pos, fmt);
                        return val.unwrap_or_else(|| MoltObject::none().bits());
                    }
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = shape[0];
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let base_offset = memoryview_offset(ptr);
                        let base_stride = strides[0];
                        let itemsize = memoryview_itemsize(ptr);
                        let new_offset = base_offset + start * base_stride;
                        let new_stride = base_stride * step;
                        let new_len = range_len_i64(start as i64, stop as i64, step as i64);
                        let new_len = new_len.max(0) as usize;
                        let mut new_shape = shape.to_vec();
                        let mut new_strides = strides.to_vec();
                        if !new_shape.is_empty() {
                            new_shape[0] = new_len as isize;
                            new_strides[0] = new_stride;
                        }
                        let out_ptr = alloc_memoryview_shaped(
                            _py,
                            memoryview_owner_bits(ptr),
                            new_offset,
                            itemsize,
                            memoryview_readonly(ptr),
                            memoryview_format_bits(ptr),
                            new_shape,
                            new_strides,
                        );
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    if ndim > 1 {
                        return raise_exception::<_>(
                            _py,
                            "NotImplementedError",
                            "multi-dimensional sub-views are not implemented",
                        );
                    }
                    let Some(idx) = index_i64_with_overflow(
                        _py,
                        key_bits,
                        "memoryview: invalid slice key",
                        None,
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let len = shape[0] as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let pos = memoryview_offset(ptr) + (i as isize) * strides[0];
                    if pos < 0 || pos + fmt.itemsize as isize > base.len() as isize {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let val = memoryview_read_scalar(_py, base, pos, fmt);
                    return val.unwrap_or_else(|| MoltObject::none().bits());
                }
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let bytes = if type_id == TYPE_ID_STRING {
                            std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr))
                        } else {
                            std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr))
                        };
                        let len = if type_id == TYPE_ID_STRING {
                            utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize)) as isize
                        } else {
                            bytes.len() as isize
                        };
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                if type_id == TYPE_ID_STRING {
                                    alloc_string(_py, &[])
                                } else if type_id == TYPE_ID_BYTES {
                                    alloc_bytes(_py, &[])
                                } else {
                                    alloc_bytearray(_py, &[])
                                }
                            } else if type_id == TYPE_ID_STRING {
                                let start_byte = utf8_char_to_byte_index_cached(
                                    _py,
                                    bytes,
                                    s as i64,
                                    Some(ptr as usize),
                                );
                                let end_byte = utf8_char_to_byte_index_cached(
                                    _py,
                                    bytes,
                                    e as i64,
                                    Some(ptr as usize),
                                );
                                alloc_string(_py, &bytes[start_byte..end_byte])
                            } else if type_id == TYPE_ID_BYTES {
                                alloc_bytes(_py, &bytes[s..e])
                            } else {
                                alloc_bytearray(_py, &bytes[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            if type_id == TYPE_ID_STRING {
                                for idx in indices {
                                    if let Some(code) = wtf8_codepoint_at(bytes, idx) {
                                        push_wtf8_codepoint(&mut out, code.to_u32());
                                    }
                                }
                            } else {
                                for idx in indices {
                                    out.push(bytes[idx]);
                                }
                            }
                            if type_id == TYPE_ID_STRING {
                                alloc_string(_py, &out)
                            } else if type_id == TYPE_ID_BYTES {
                                alloc_bytes(_py, &out)
                            } else {
                                alloc_bytearray(_py, &out)
                            }
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let type_err = if type_id == TYPE_ID_STRING {
                        format!(
                            "string indices must be integers, not '{}'",
                            type_name(_py, key)
                        )
                    } else if type_id == TYPE_ID_BYTES {
                        format!(
                            "byte indices must be integers or slices, not {}",
                            type_name(_py, key)
                        )
                    } else {
                        format!(
                            "bytearray indices must be integers or slices, not {}",
                            type_name(_py, key)
                        )
                    };
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
                        return MoltObject::none().bits();
                    };
                    if type_id == TYPE_ID_STRING {
                        let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                        let mut i = idx;
                        let len = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                        if i < 0 {
                            i += len;
                        }
                        if i < 0 || i >= len {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "string index out of range",
                            );
                        }
                        let Some(code) = wtf8_codepoint_at(bytes, i as usize) else {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "string index out of range",
                            );
                        };
                        let mut out = Vec::with_capacity(4);
                        push_wtf8_codepoint(&mut out, code.to_u32());
                        let out_ptr = alloc_string(_py, &out);
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
                    let len = bytes.len() as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        if type_id == TYPE_ID_BYTEARRAY {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "bytearray index out of range",
                            );
                        }
                        return raise_exception::<_>(_py, "IndexError", "index out of range");
                    }
                    return MoltObject::from_int(bytes[i as usize] as i64).bits();
                }
                if type_id == TYPE_ID_LIST {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let elems = seq_vec_ref(ptr);
                        let len = elems.len() as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                alloc_list(_py, &[])
                            } else {
                                alloc_list(_py, &elems[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            for idx in indices {
                                out.push(elems[idx]);
                            }
                            alloc_list(_py, out.as_slice())
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let idx = if let Some(i) = to_i64(key) {
                        i
                    } else {
                        let key_type = type_name(_py, key);
                        if debug_index_enabled() {
                            eprintln!(
                                "molt index type-error op=get container=list key_type={} key_bits=0x{:x} key_float={:?}",
                                key_type,
                                key_bits,
                                key.as_float()
                            );
                        }
                        let type_err =
                            format!("list indices must be integers or slices, not {}", key_type);
                        let Some(i) = index_i64_with_overflow(_py, key_bits, &type_err, None)
                        else {
                            return MoltObject::none().bits();
                        };
                        i
                    };
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        if debug_index_enabled() {
                            let task = crate::current_task_key()
                                .map(|slot| slot.0 as usize)
                                .unwrap_or(0);
                            eprintln!(
                                "molt index oob task=0x{:x} type=list len={} idx={}",
                                task, len, i
                            );
                        }
                        return raise_exception::<_>(_py, "IndexError", "list index out of range");
                    }
                    let elems = seq_vec_ref(ptr);
                    let val = elems[i as usize];
                    if debug_index_list_enabled() {
                        let val_obj = obj_from_bits(val);
                        eprintln!(
                            "molt_index list obj=0x{:x} idx={} val_type={} val_bits=0x{:x}",
                            obj_bits,
                            i,
                            type_name(_py, val_obj),
                            val
                        );
                    }
                    inc_ref_bits(_py, val);
                    return val;
                }
                if type_id == TYPE_ID_TUPLE {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let elems = seq_vec_ref(ptr);
                        let len = elems.len() as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                alloc_tuple(_py, &[])
                            } else {
                                alloc_tuple(_py, &elems[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            for idx in indices {
                                out.push(elems[idx]);
                            }
                            alloc_tuple(_py, out.as_slice())
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let idx = if let Some(i) = to_i64(key) {
                        i
                    } else {
                        let key_type = type_name(_py, key);
                        if debug_index_enabled() {
                            eprintln!(
                                "molt index type-error op=get container=tuple key_type={} key_bits=0x{:x} key_float={:?}",
                                key_type,
                                key_bits,
                                key.as_float()
                            );
                        }
                        let type_err =
                            format!("tuple indices must be integers or slices, not {}", key_type);
                        let Some(i) = index_i64_with_overflow(_py, key_bits, &type_err, None)
                        else {
                            return MoltObject::none().bits();
                        };
                        i
                    };
                    let len = tuple_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        if debug_index_enabled() {
                            let task = crate::current_task_key()
                                .map(|slot| slot.0 as usize)
                                .unwrap_or(0);
                            eprintln!(
                                "molt index oob task=0x{:x} type=tuple len={} idx={}",
                                task, len, i
                            );
                        }
                        return raise_exception::<_>(_py, "IndexError", "tuple index out of range");
                    }
                    let elems = seq_vec_ref(ptr);
                    let val = elems[i as usize];
                    inc_ref_bits(_py, val);
                    return val;
                }
                if type_id == TYPE_ID_RANGE {
                    if let Some((start_i64, stop_i64, step_i64)) = range_components_i64(ptr)
                        && let Some(mut idx_i64) = to_i64(key)
                    {
                        if idx_i64 < 0 {
                            let len = range_len_i128(start_i64, stop_i64, step_i64);
                            let adj = (idx_i64 as i128) + len;
                            if adj < 0 {
                                return raise_exception::<_>(
                                    _py,
                                    "IndexError",
                                    "range object index out of range",
                                );
                            }
                            idx_i64 = match i64::try_from(adj) {
                                Ok(v) => v,
                                Err(_) => {
                                    return raise_exception::<_>(
                                        _py,
                                        "IndexError",
                                        "range object index out of range",
                                    );
                                }
                            };
                        }
                        if let Some(value) =
                            range_value_at_index_i64(start_i64, stop_i64, step_i64, idx_i64 as i128)
                        {
                            return MoltObject::from_int(value).bits();
                        }
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "range object index out of range",
                        );
                    }
                    let type_err = format!(
                        "range indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(mut idx) = index_bigint_from_obj(_py, key_bits, &type_err) else {
                        return MoltObject::none().bits();
                    };
                    let Some((start, stop, step)) = range_components_bigint(ptr) else {
                        return MoltObject::none().bits();
                    };
                    let len = range_len_bigint(&start, &stop, &step);
                    if idx.is_negative() {
                        idx += &len;
                    }
                    if idx.is_negative() || idx >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "range object index out of range",
                        );
                    }
                    let val = start + step * idx;
                    return int_bits_from_bigint(_py, val);
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if let Some(val) = dict_get_in_place(_py, dict_ptr, key_bits) {
                        // Skip inc_ref for inline values (ints, bools, None).
                        if obj_from_bits(val).as_ptr().is_some() {
                            inc_ref_bits(_py, val);
                        }
                        return val;
                    }
                    if object_type_id(ptr) != TYPE_ID_DICT
                        && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__missing__")
                    {
                        if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits)
                        {
                            dec_ref_bits(_py, name_bits);
                            let res = call_callable1(_py, call_bits, key_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            return res;
                        }
                        dec_ref_bits(_py, name_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    return raise_key_error_with_key(_py, key_bits);
                }
                if type_id == TYPE_ID_DICT_KEYS_VIEW
                    || type_id == TYPE_ID_DICT_VALUES_VIEW
                    || type_id == TYPE_ID_DICT_ITEMS_VIEW
                {
                    let view_name = type_name(_py, obj);
                    let msg = format!("'{}' object is not subscriptable", view_name);
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if type_id == TYPE_ID_TYPE {
                    // Try explicit __class_getitem__ first (handles custom
                    // implementations in user-defined classes).
                    if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__class_getitem__") {
                        if let Some(call_bits) =
                            class_attr_lookup(_py, ptr, ptr, Some(ptr), name_bits)
                        {
                            dec_ref_bits(_py, name_bits);
                            exception_stack_push();
                            let res = call_callable1(_py, call_bits, key_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                exception_stack_pop(_py);
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            return res;
                        }
                        dec_ref_bits(_py, name_bits);
                    }
                    // Default __class_getitem__: create a GenericAlias
                    // directly.  This matches CPython >= 3.12 where every
                    // type supports subscript via a default that returns
                    // types.GenericAlias(cls, params).
                    return crate::builtins::types::molt_generic_alias_new(obj_bits, key_bits);
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") {
                    if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        exception_stack_push();
                        let res = call_callable1(_py, call_bits, key_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            exception_stack_pop(_py);
                            return MoltObject::none().bits();
                        }
                        exception_stack_pop(_py);
                        return res;
                    }
                    dec_ref_bits(_py, name_bits);
                }
            }
            let msg = if unsafe { object_type_id(ptr) } == TYPE_ID_TYPE {
                let class_name =
                    unsafe { string_obj_to_owned(obj_from_bits(class_name_bits(ptr))) }
                        .unwrap_or_else(|| "object".to_string());
                eprintln!(
                    "[MOLT-DEBUG] subscript fail (TYPE_ID_TYPE, no __class_getitem__): class_name={}, obj_bits=0x{:016x}, key_bits=0x{:016x}",
                    class_name, obj_bits, key_bits
                );
                format!("type '{}' is not subscriptable", class_name)
            } else {
                let tn = type_name(_py, obj);
                let tid = unsafe { object_type_id(ptr) };
                eprintln!(
                    "[MOLT-DEBUG] subscript fail (ptr path): type_name={}, type_id={}, obj_bits=0x{:016x}, key_bits=0x{:016x}",
                    tn, tid, obj_bits, key_bits
                );
                format!("'{}' object is not subscriptable", tn)
            };
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let obj_dbg = obj_from_bits(obj_bits);
        eprintln!(
            "[MOLT-DEBUG] subscript fail (no-ptr path): type_name={}, obj_bits=0x{:016x}, key_bits=0x{:016x}, is_int={}, is_float={}, is_bool={}, is_none={}, is_pending={}",
            type_name(_py, obj_dbg),
            obj_bits,
            key_bits,
            obj_dbg.is_int(),
            obj_dbg.is_float(),
            obj_dbg.is_bool(),
            obj_dbg.is_none(),
            obj_dbg.is_pending()
        );
        let msg = format!("'{}' object is not subscriptable", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_store_index(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        // Fast path: dict[key] = val — skips type dispatch chain.
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_DICT {
                    dict_set_in_place(_py, ptr, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return obj_bits;
                }
                // list_int: flat i64 storage — delegate to specialized setitem
                if object_type_id(ptr) == TYPE_ID_LIST_INT {
                    return molt_list_int_setitem(obj_bits, key_bits, val_bits);
                }
            }
        }
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = list_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let new_items = match collect_iterable_values(
                            _py,
                            val_bits,
                            "must assign iterable to extended slice",
                        ) {
                            Some(items) => items,
                            None => return MoltObject::none().bits(),
                        };
                        let elems = seq_vec(ptr);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            for &item in new_items.iter() {
                                inc_ref_bits(_py, item);
                            }
                            let removed: Vec<u64> =
                                elems.splice(s..e, new_items.iter().copied()).collect();
                            for old_bits in removed {
                                dec_ref_bits(_py, old_bits);
                            }
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if indices.len() != new_items.len() {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                &format!(
                                    "attempt to assign sequence of size {} to extended slice of size {}",
                                    new_items.len(),
                                    indices.len()
                                ),
                            );
                        }
                        for &item in new_items.iter() {
                            inc_ref_bits(_py, item);
                        }
                        for (idx, &item) in indices.iter().zip(new_items.iter()) {
                            let old_bits = elems[*idx];
                            if old_bits != item {
                                dec_ref_bits(_py, old_bits);
                                elems[*idx] = item;
                            }
                        }
                        return obj_bits;
                    }
                    let idx = if let Some(i) = to_i64(key) {
                        i
                    } else {
                        let key_type = type_name(_py, key);
                        if debug_index_enabled() {
                            eprintln!(
                                "molt index type-error op=set container=list key_type={} key_bits=0x{:x} key_float={:?}",
                                key_type,
                                key_bits,
                                key.as_float()
                            );
                        }
                        let type_err =
                            format!("list indices must be integers or slices, not {}", key_type);
                        let Some(i) = index_i64_with_overflow(_py, key_bits, &type_err, None)
                        else {
                            return MoltObject::none().bits();
                        };
                        i
                    };
                    if debug_store_index_enabled() {
                        let val_obj = obj_from_bits(val_bits);
                        eprintln!(
                            "molt_store_index list obj=0x{:x} idx={} val_type={} val_bits=0x{:x}",
                            obj_bits,
                            idx,
                            type_name(_py, val_obj),
                            val_bits
                        );
                    }
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "list assignment index out of range",
                        );
                    }
                    let elems = seq_vec(ptr);
                    let old_bits = elems[i as usize];
                    if old_bits != val_bits {
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        elems[i as usize] = val_bits;
                    }
                    return obj_bits;
                }
                if type_id == TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = bytes_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let src_bytes = match collect_bytearray_assign_bytes(_py, val_bits) {
                            Some(bytes) => bytes,
                            None => return MoltObject::none().bits(),
                        };
                        let elems = bytearray_vec(ptr);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            elems.splice(s..e, src_bytes.iter().copied());
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if indices.len() != src_bytes.len() {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                &format!(
                                    "attempt to assign bytes of size {} to extended slice of size {}",
                                    src_bytes.len(),
                                    indices.len()
                                ),
                            );
                        }
                        for (idx, byte) in indices.iter().zip(src_bytes.iter()) {
                            elems[*idx] = *byte;
                        }
                        return obj_bits;
                    }
                    let type_err = format!(
                        "bytearray indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
                        return MoltObject::none().bits();
                    };
                    let len = bytes_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "bytearray index out of range",
                        );
                    }
                    let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray)
                    else {
                        return MoltObject::none().bits();
                    };
                    let elems = bytearray_vec(ptr);
                    elems[i as usize] = byte;
                    return obj_bits;
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    if memoryview_readonly(ptr) {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot modify read-only memory",
                        );
                    }
                    let owner_bits = memoryview_owner_bits(ptr);
                    let owner = obj_from_bits(owner_bits);
                    let owner_ptr = match owner.as_ptr() {
                        Some(ptr) => ptr,
                        None => return MoltObject::none().bits(),
                    };
                    if object_type_id(owner_ptr) != TYPE_ID_BYTEARRAY {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "memoryview is not writable",
                        );
                    }
                    let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                        Some(fmt) => fmt,
                        None => return MoltObject::none().bits(),
                    };
                    let shape = memoryview_shape(ptr).unwrap_or(&[]);
                    let strides = memoryview_strides(ptr).unwrap_or(&[]);
                    let ndim = shape.len();
                    let data = bytearray_vec(owner_ptr);
                    if ndim == 0 {
                        if let Some(tup_ptr) = key.as_ptr()
                            && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                        {
                            let elems = seq_vec_ref(tup_ptr);
                            if elems.is_empty() {
                                let ok = memoryview_write_scalar(
                                    _py,
                                    data.as_mut_slice(),
                                    memoryview_offset(ptr),
                                    fmt,
                                    val_bits,
                                );
                                if ok.is_none() {
                                    return MoltObject::none().bits();
                                }
                                return obj_bits;
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "invalid indexing of 0-dim memory",
                        );
                    }
                    if let Some(tup_ptr) = key.as_ptr()
                        && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                    {
                        let elems = seq_vec_ref(tup_ptr);
                        let mut has_slice = false;
                        let mut all_slice = true;
                        for &elem_bits in elems.iter() {
                            let elem_obj = obj_from_bits(elem_bits);
                            if let Some(elem_ptr) = elem_obj.as_ptr() {
                                if object_type_id(elem_ptr) == TYPE_ID_SLICE {
                                    has_slice = true;
                                } else {
                                    all_slice = false;
                                }
                            } else {
                                all_slice = false;
                            }
                        }
                        if has_slice {
                            if all_slice {
                                return raise_exception::<_>(
                                    _py,
                                    "NotImplementedError",
                                    "memoryview slice assignments are currently restricted to ndim = 1",
                                );
                            }
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "memoryview: invalid slice key",
                            );
                        }
                        if elems.len() < ndim {
                            return raise_exception::<_>(
                                _py,
                                "NotImplementedError",
                                "sub-views are not implemented",
                            );
                        }
                        if elems.len() > ndim {
                            let msg = format!(
                                "cannot index {}-dimension view with {}-element tuple",
                                ndim,
                                elems.len()
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        if shape.len() != strides.len() {
                            return MoltObject::none().bits();
                        }
                        let mut pos = memoryview_offset(ptr);
                        for (dim, &elem_bits) in elems.iter().enumerate() {
                            let Some(idx) = index_i64_with_overflow(
                                _py,
                                elem_bits,
                                "memoryview: invalid slice key",
                                None,
                            ) else {
                                return MoltObject::none().bits();
                            };
                            let mut i = idx;
                            let dim_len = shape[dim];
                            let dim_len_i64 = dim_len as i64;
                            if i < 0 {
                                i += dim_len_i64;
                            }
                            if i < 0 || i >= dim_len_i64 {
                                let msg = format!("index out of bounds on dimension {}", dim + 1);
                                return raise_exception::<_>(_py, "IndexError", &msg);
                            }
                            pos = pos.saturating_add((i as isize).saturating_mul(strides[dim]));
                        }
                        if pos < 0 || pos + fmt.itemsize as isize > data.len() as isize {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "index out of bounds on dimension 1",
                            );
                        }
                        let ok =
                            memoryview_write_scalar(_py, data.as_mut_slice(), pos, fmt, val_bits);
                        if ok.is_none() {
                            return MoltObject::none().bits();
                        }
                        return obj_bits;
                    }
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        if ndim != 1 {
                            return raise_exception::<_>(
                                _py,
                                "NotImplementedError",
                                "memoryview slice assignments are currently restricted to ndim = 1",
                            );
                        }
                        let len = shape[0];
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let indices = collect_slice_indices(start, stop, step);
                        let elem_count = indices.len();
                        let val_obj = obj_from_bits(val_bits);
                        let src_bytes = if let Some(src_ptr) = val_obj.as_ptr() {
                            let src_type = object_type_id(src_ptr);
                            if src_type == TYPE_ID_BYTES || src_type == TYPE_ID_BYTEARRAY {
                                if fmt.code != b'B' {
                                    return raise_exception::<_>(
                                        _py,
                                        "ValueError",
                                        "memoryview assignment: lvalue and rvalue have different structures",
                                    );
                                }
                                bytes_like_slice_raw(src_ptr).unwrap_or(&[]).to_vec()
                            } else if src_type == TYPE_ID_MEMORYVIEW {
                                let src_fmt = match memoryview_format_from_bits(
                                    memoryview_format_bits(src_ptr),
                                ) {
                                    Some(fmt) => fmt,
                                    None => return MoltObject::none().bits(),
                                };
                                let src_shape = memoryview_shape(src_ptr).unwrap_or(&[]);
                                if src_fmt.code != fmt.code
                                    || src_shape.len() != 1
                                    || src_shape[0] as usize != elem_count
                                {
                                    return raise_exception::<_>(
                                        _py,
                                        "ValueError",
                                        "memoryview assignment: lvalue and rvalue have different structures",
                                    );
                                }
                                match memoryview_collect_bytes(src_ptr) {
                                    Some(buf) => buf,
                                    None => return MoltObject::none().bits(),
                                }
                            } else {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(_py, val_obj)
                                    ),
                                );
                            }
                        } else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, val_obj)
                                ),
                            );
                        };
                        let expected = elem_count * fmt.itemsize;
                        if src_bytes.len() != expected {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "memoryview assignment: lvalue and rvalue have different structures",
                            );
                        }
                        let base_offset = memoryview_offset(ptr);
                        let base_stride = strides[0];
                        let mut pos = base_offset + start * base_stride;
                        let step_stride = base_stride * step;
                        let mut idx = 0usize;
                        while idx < src_bytes.len() {
                            if pos < 0 || pos + fmt.itemsize as isize > data.len() as isize {
                                return MoltObject::none().bits();
                            }
                            let start = pos as usize;
                            let end = start + fmt.itemsize;
                            data[start..end].copy_from_slice(&src_bytes[idx..idx + fmt.itemsize]);
                            idx += fmt.itemsize;
                            pos += step_stride;
                        }
                        return obj_bits;
                    }
                    if ndim != 1 {
                        return raise_exception::<_>(
                            _py,
                            "NotImplementedError",
                            "sub-views are not implemented",
                        );
                    }
                    let Some(idx) = index_i64_with_overflow(
                        _py,
                        key_bits,
                        "memoryview: invalid slice key",
                        None,
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let len = shape[0] as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let pos = memoryview_offset(ptr) + (i as isize) * strides[0];
                    if pos < 0 || pos + fmt.itemsize as isize > data.len() as isize {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let ok = memoryview_write_scalar(_py, data.as_mut_slice(), pos, fmt, val_bits);
                    if ok.is_none() {
                        return MoltObject::none().bits();
                    }
                    return obj_bits;
                }
                if type_id == TYPE_ID_OBJECT {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        let mappingproxy_bits =
                            crate::builtins::types::mappingproxy_class_bits(_py);
                        if class_bits == mappingproxy_bits {
                            return raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "'mappingproxy' object does not support item assignment",
                            );
                        }
                        let builtins = builtin_classes(_py);
                        if issubclass_bits(class_bits, builtins.dict)
                            && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__setitem__")
                        {
                            if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                                dec_ref_bits(_py, name_bits);
                                exception_stack_push();
                                let _ = call_callable2(_py, call_bits, key_bits, val_bits);
                                dec_ref_bits(_py, call_bits);
                                if exception_pending(_py) {
                                    exception_stack_pop(_py);
                                    return MoltObject::none().bits();
                                }
                                exception_stack_pop(_py);
                                return obj_bits;
                            }
                            dec_ref_bits(_py, name_bits);
                        }
                    }
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return obj_bits;
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__setitem__") {
                    if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        exception_stack_push();
                        let _ = call_callable2(_py, call_bits, key_bits, val_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            exception_stack_pop(_py);
                            return MoltObject::none().bits();
                        }
                        exception_stack_pop(_py);
                        return obj_bits;
                    }
                    dec_ref_bits(_py, name_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_del_index(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = list_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let elems = seq_vec(ptr);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            let removed: Vec<u64> = elems.drain(s..e).collect();
                            for old_bits in removed {
                                dec_ref_bits(_py, old_bits);
                            }
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if step > 0 {
                            for &idx in indices.iter().rev() {
                                let old_bits = elems.remove(idx);
                                dec_ref_bits(_py, old_bits);
                            }
                        } else {
                            for &idx in indices.iter() {
                                let old_bits = elems.remove(idx);
                                dec_ref_bits(_py, old_bits);
                            }
                        }
                        return obj_bits;
                    }
                    let key_type = type_name(_py, key);
                    if debug_index_enabled() {
                        eprintln!(
                            "molt index type-error op=del container=list key_type={} key_bits=0x{:x} key_float={:?}",
                            key_type,
                            key_bits,
                            key.as_float()
                        );
                    }
                    let type_err =
                        format!("list indices must be integers or slices, not {}", key_type);
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
                        return MoltObject::none().bits();
                    };
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "list assignment index out of range",
                        );
                    }
                    let elems = seq_vec(ptr);
                    let old_bits = elems.remove(i as usize);
                    dec_ref_bits(_py, old_bits);
                    return obj_bits;
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = bytes_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let elems = bytearray_vec(ptr);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            elems.drain(s..e);
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if step > 0 {
                            for &idx in indices.iter().rev() {
                                elems.remove(idx);
                            }
                        } else {
                            for &idx in indices.iter() {
                                elems.remove(idx);
                            }
                        }
                        return obj_bits;
                    }
                    let type_err = format!(
                        "bytearray indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
                        return MoltObject::none().bits();
                    };
                    let len = bytes_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "bytearray index out of range",
                        );
                    }
                    let elems = bytearray_vec(ptr);
                    elems.remove(i as usize);
                    return obj_bits;
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    if memoryview_readonly(ptr) {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot modify read-only memory",
                        );
                    }
                    return raise_exception::<_>(_py, "TypeError", "cannot delete memory");
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    let removed = dict_del_in_place(_py, dict_ptr, key_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if removed {
                        return obj_bits;
                    }
                    return raise_key_error_with_key(_py, key_bits);
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__delitem__") {
                    if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        exception_stack_push();
                        let _ = call_callable1(_py, call_bits, key_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            exception_stack_pop(_py);
                            return MoltObject::none().bits();
                        }
                        exception_stack_pop(_py);
                        return obj_bits;
                    }
                    dec_ref_bits(_py, name_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getitem_method(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_index(obj_bits, key_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_setitem_method(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        let _ = molt_store_index(obj_bits, key_bits, val_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_delitem_method(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = molt_del_index(obj_bits, key_bits);
        MoltObject::none().bits()
    })
}

pub(super) unsafe fn eq_bool_from_bits(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
) -> Option<bool> {
    let pending_before = exception_pending(_py);
    let prev_exc_bits = if pending_before {
        exception_last_bits_noinc(_py).unwrap_or(0)
    } else {
        0
    };
    let res_bits = molt_eq(lhs_bits, rhs_bits);
    if exception_pending(_py) {
        if !pending_before {
            return None;
        }
        let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
        if after_exc_bits != prev_exc_bits {
            return None;
        }
    }
    let res_obj = obj_from_bits(res_bits);
    if pending_before && res_obj.is_none() {
        return Some(obj_eq(
            _py,
            obj_from_bits(lhs_bits),
            obj_from_bits(rhs_bits),
        ));
    }
    Some(is_truthy(_py, res_obj))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contains(container_bits: u64, item_bits: u64) -> u64 {
    // Tolerate None container from undefined SSA paths on exception handler branches.
    if obj_from_bits(container_bits).is_none() {
        return MoltObject::from_bool(false).bits();
    }
    crate::with_gil_entry!(_py, {
        let container = obj_from_bits(container_bits);
        let item = obj_from_bits(item_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if !ensure_hashable(_py, item_bits) {
                        return MoltObject::none().bits();
                    }
                    let order = dict_order(dict_ptr);
                    let table = dict_table(dict_ptr);
                    let found = dict_find_entry(_py, order, table, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_bool(found.is_some()).bits();
                }
                match type_id {
                    TYPE_ID_LIST => {
                        // Fast path: for NaN-boxed integers/bools/None, identity
                        // (bit-equality) implies value equality. Scan the raw u64
                        // slice first to avoid per-element inc_ref/eq/dec_ref.
                        // This is the hot path for `x in [1, 2, 3]` style range checks.
                        if item.as_int().is_some() || item.is_bool() || item.is_none() {
                            let elems = seq_vec_ref(ptr);
                            if simd_contains_u64(elems, item_bits) {
                                return MoltObject::from_bool(true).bits();
                            }
                            return MoltObject::from_bool(false).bits();
                        }
                        let mut idx = 0usize;
                        while let Some(val) = list_elem_at(ptr, idx) {
                            let elem_bits = val;
                            // Identity check: bit-equality for non-float tagged
                            // values.  NaN-boxed floats collapse all NaN values
                            // to CANONICAL_NAN_BITS, so bit-equality gives false
                            // positives for different NaN objects.  Pointer-tagged
                            // objects (strings, lists, etc.) have unique addresses
                            // so bit-equality is correct for identity.
                            if elem_bits == item_bits && !obj_from_bits(elem_bits).is_float() {
                                return MoltObject::from_bool(true).bits();
                            }
                            inc_ref_bits(_py, elem_bits);
                            let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                                Some(val) => val,
                                None => {
                                    dec_ref_bits(_py, elem_bits);
                                    return MoltObject::none().bits();
                                }
                            };
                            dec_ref_bits(_py, elem_bits);
                            if eq {
                                return MoltObject::from_bool(true).bits();
                            }
                            idx += 1;
                        }
                        return MoltObject::from_bool(false).bits();
                    }
                    TYPE_ID_TUPLE => {
                        let elems = seq_vec_ref(ptr);
                        // Same identity fast path for tuples with inline-int/bool/None needle.
                        if item.as_int().is_some() || item.is_bool() || item.is_none() {
                            if simd_contains_u64(elems, item_bits) {
                                return MoltObject::from_bool(true).bits();
                            }
                            return MoltObject::from_bool(false).bits();
                        }
                        for &elem_bits in elems.iter() {
                            if elem_bits == item_bits && !obj_from_bits(elem_bits).is_float() {
                                return MoltObject::from_bool(true).bits();
                            }
                            let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                                Some(val) => val,
                                None => return MoltObject::none().bits(),
                            };
                            if eq {
                                return MoltObject::from_bool(true).bits();
                            }
                        }
                        return MoltObject::from_bool(false).bits();
                    }
                    TYPE_ID_SET | TYPE_ID_FROZENSET => {
                        if !ensure_hashable(_py, item_bits) {
                            return MoltObject::none().bits();
                        }
                        let order = set_order(ptr);
                        let table = set_table(ptr);
                        let found = set_find_entry(_py, order, table, item_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_bool(found.is_some()).bits();
                    }
                    TYPE_ID_STRING => {
                        let Some(item_ptr) = item.as_ptr() else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "'in <string>' requires string as left operand, not {}",
                                    type_name(_py, item)
                                ),
                            );
                        };
                        if object_type_id(item_ptr) != TYPE_ID_STRING {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "'in <string>' requires string as left operand, not {}",
                                    type_name(_py, item)
                                ),
                            );
                        }
                        let hay_len = string_len(ptr);
                        let needle_len = string_len(item_ptr);
                        let hay_bytes = std::slice::from_raw_parts(string_bytes(ptr), hay_len);
                        let needle_bytes =
                            std::slice::from_raw_parts(string_bytes(item_ptr), needle_len);
                        if needle_bytes.is_empty() {
                            return MoltObject::from_bool(true).bits();
                        }
                        let idx = bytes_find_impl(hay_bytes, needle_bytes);
                        return MoltObject::from_bool(idx >= 0).bits();
                    }
                    TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                        let hay_len = bytes_len(ptr);
                        let hay_bytes = std::slice::from_raw_parts(bytes_data(ptr), hay_len);
                        if let Some(byte) = item.as_int() {
                            if !(0..=255).contains(&byte) {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "byte must be in range(0, 256)",
                                );
                            }
                            let found = memchr(byte as u8, hay_bytes).is_some();
                            return MoltObject::from_bool(found).bits();
                        }
                        if let Some(item_ptr) = item.as_ptr() {
                            let item_type = object_type_id(item_ptr);
                            if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                                let needle_len = bytes_len(item_ptr);
                                let needle_bytes =
                                    std::slice::from_raw_parts(bytes_data(item_ptr), needle_len);
                                if needle_bytes.is_empty() {
                                    return MoltObject::from_bool(true).bits();
                                }
                                let idx = bytes_find_impl(hay_bytes, needle_bytes);
                                return MoltObject::from_bool(idx >= 0).bits();
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            &format!(
                                "a bytes-like object is required, not '{}'",
                                type_name(_py, item)
                            ),
                        );
                    }
                    TYPE_ID_RANGE => {
                        let candidate = if let Some(f) = item.as_float() {
                            if !f.is_finite() || f.fract() != 0.0 {
                                return MoltObject::from_bool(false).bits();
                            }
                            bigint_from_f64_trunc(f)
                        } else {
                            let type_err = format!(
                                "'{}' object cannot be interpreted as an integer",
                                type_name(_py, item)
                            );
                            let Some(val) = index_bigint_from_obj(_py, item_bits, &type_err) else {
                                if exception_pending(_py) {
                                    molt_exception_clear();
                                }
                                return MoltObject::from_bool(false).bits();
                            };
                            val
                        };
                        let Some((start, stop, step)) = range_components_bigint(ptr) else {
                            return MoltObject::none().bits();
                        };
                        if step.is_zero() {
                            return MoltObject::from_bool(false).bits();
                        }
                        let in_range = if step.is_positive() {
                            candidate >= start && candidate < stop
                        } else {
                            candidate <= start && candidate > stop
                        };
                        if !in_range {
                            return MoltObject::from_bool(false).bits();
                        }
                        let offset = candidate - start;
                        let step_abs = if step.is_negative() { -step } else { step };
                        let aligned = offset.mod_floor(&step_abs).is_zero();
                        return MoltObject::from_bool(aligned).bits();
                    }
                    TYPE_ID_MEMORYVIEW => {
                        let owner_bits = memoryview_owner_bits(ptr);
                        let owner = obj_from_bits(owner_bits);
                        let owner_ptr = match owner.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(_py, item)
                                    ),
                                );
                            }
                        };
                        let base = match bytes_like_slice_raw(owner_ptr) {
                            Some(slice) => slice,
                            None => {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(_py, item)
                                    ),
                                );
                            }
                        };
                        let offset = memoryview_offset(ptr);
                        let len = memoryview_len(ptr);
                        let itemsize = memoryview_itemsize(ptr);
                        let stride = memoryview_stride(ptr);
                        if offset < 0 {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, item)
                                ),
                            );
                        }
                        if itemsize != 1 {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "memoryview itemsize not supported",
                            );
                        }
                        if stride == 1 {
                            let start = offset as usize;
                            let end = start.saturating_add(len);
                            let hay = &base[start.min(base.len())..end.min(base.len())];
                            if let Some(byte) = item.as_int() {
                                if !(0..=255).contains(&byte) {
                                    return raise_exception::<_>(
                                        _py,
                                        "ValueError",
                                        "byte must be in range(0, 256)",
                                    );
                                }
                                let found = memchr(byte as u8, hay).is_some();
                                return MoltObject::from_bool(found).bits();
                            }
                            if let Some(item_ptr) = item.as_ptr() {
                                let item_type = object_type_id(item_ptr);
                                if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                                    let needle_len = bytes_len(item_ptr);
                                    let needle_bytes = std::slice::from_raw_parts(
                                        bytes_data(item_ptr),
                                        needle_len,
                                    );
                                    if needle_bytes.is_empty() {
                                        return MoltObject::from_bool(true).bits();
                                    }
                                    let idx = bytes_find_impl(hay, needle_bytes);
                                    return MoltObject::from_bool(idx >= 0).bits();
                                }
                            }
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, item)
                                ),
                            );
                        }
                        let mut out = Vec::with_capacity(len);
                        for idx in 0..len {
                            let start = offset + (idx as isize) * stride;
                            if start < 0 {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(_py, item)
                                    ),
                                );
                            }
                            let start = start as usize;
                            if start >= base.len() {
                                break;
                            }
                            out.push(base[start]);
                        }
                        let hay = out.as_slice();
                        if let Some(byte) = item.as_int() {
                            if !(0..=255).contains(&byte) {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "byte must be in range(0, 256)",
                                );
                            }
                            let found = memchr(byte as u8, hay).is_some();
                            return MoltObject::from_bool(found).bits();
                        }
                        if let Some(item_ptr) = item.as_ptr() {
                            let item_type = object_type_id(item_ptr);
                            if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                                let needle_len = bytes_len(item_ptr);
                                let needle_bytes =
                                    std::slice::from_raw_parts(bytes_data(item_ptr), needle_len);
                                if needle_bytes.is_empty() {
                                    return MoltObject::from_bool(true).bits();
                                }
                                let idx = bytes_find_impl(hay, needle_bytes);
                                return MoltObject::from_bool(idx >= 0).bits();
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            &format!(
                                "a bytes-like object is required, not '{}'",
                                type_name(_py, item)
                            ),
                        );
                    }
                    _ => {}
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__contains__") {
                    if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        let res_bits = call_callable1(_py, call_bits, item_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        if !is_not_implemented_bits(_py, res_bits) {
                            let truthy = is_truthy(_py, obj_from_bits(res_bits));
                            dec_ref_bits(_py, res_bits);
                            return MoltObject::from_bool(truthy).bits();
                        }
                        dec_ref_bits(_py, res_bits);
                    } else {
                        dec_ref_bits(_py, name_bits);
                    }
                }
                let iter_bits = molt_iter(container_bits);
                if !obj_from_bits(iter_bits).is_none() {
                    loop {
                        let pair_bits = molt_iter_next(iter_bits);
                        let pair_obj = obj_from_bits(pair_bits);
                        let Some(pair_ptr) = pair_obj.as_ptr() else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        };
                        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        }
                        let elems = seq_vec_ref(pair_ptr);
                        if elems.len() < 2 {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        }
                        let val_bits = elems[0];
                        let done_bits = elems[1];
                        if is_truthy(_py, obj_from_bits(done_bits)) {
                            return MoltObject::from_bool(false).bits();
                        }
                        if obj_eq(_py, obj_from_bits(val_bits), item) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") {
                    if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        let mut idx = 0i64;
                        loop {
                            let idx_bits = MoltObject::from_int(idx).bits();
                            exception_stack_push();
                            let val_bits = call_callable1(_py, call_bits, idx_bits);
                            if exception_pending(_py) {
                                let exc_bits = molt_exception_last();
                                let exc_obj = obj_from_bits(exc_bits);
                                let mut is_index_error = false;
                                if let Some(exc_ptr) = exc_obj.as_ptr()
                                    && object_type_id(exc_ptr) == TYPE_ID_EXCEPTION
                                {
                                    let kind_bits = exception_kind_bits(exc_ptr);
                                    let kind_obj = obj_from_bits(kind_bits);
                                    if let Some(kind_ptr) = kind_obj.as_ptr()
                                        && object_type_id(kind_ptr) == TYPE_ID_STRING
                                    {
                                        let bytes = std::slice::from_raw_parts(
                                            string_bytes(kind_ptr),
                                            string_len(kind_ptr),
                                        );
                                        if bytes == b"IndexError" {
                                            is_index_error = true;
                                        }
                                    }
                                }
                                dec_ref_bits(_py, exc_bits);
                                exception_stack_pop(_py);
                                if is_index_error {
                                    clear_exception(_py);
                                    return MoltObject::from_bool(false).bits();
                                }
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            if obj_eq(_py, obj_from_bits(val_bits), item) {
                                dec_ref_bits(_py, val_bits);
                                return MoltObject::from_bool(true).bits();
                            }
                            dec_ref_bits(_py, val_bits);
                            idx += 1;
                        }
                    } else {
                        dec_ref_bits(_py, name_bits);
                    }
                }
            }
        }
        raise_exception::<_>(
            _py,
            "TypeError",
            &format!(
                "argument of type '{}' is not iterable",
                type_name(_py, container)
            ),
        )
    })
}

/// Specialized `in` for list containers (linear scan, no type dispatch).
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_contains(container_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let container = obj_from_bits(container_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                let mut idx = 0usize;
                while let Some(val) = list_elem_at(ptr, idx) {
                    let elem_bits = val;
                    if elem_bits == item_bits && !obj_from_bits(elem_bits).is_float() {
                        return MoltObject::from_bool(true).bits();
                    }
                    inc_ref_bits(_py, elem_bits);
                    let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                        Some(val) => val,
                        None => {
                            dec_ref_bits(_py, elem_bits);
                            return MoltObject::none().bits();
                        }
                    };
                    dec_ref_bits(_py, elem_bits);
                    if eq {
                        return MoltObject::from_bool(true).bits();
                    }
                    idx += 1;
                }
                return MoltObject::from_bool(false).bits();
            }
        }
        molt_contains(container_bits, item_bits)
    })
}

/// Specialized `in` for str containers (substring search, no type dispatch).
#[unsafe(no_mangle)]
pub extern "C" fn molt_str_contains(container_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let container = obj_from_bits(container_bits);
        let item = obj_from_bits(item_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                let Some(item_ptr) = item.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!(
                            "'in <string>' requires string as left operand, not {}",
                            type_name(_py, item)
                        ),
                    );
                };
                if object_type_id(item_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!(
                            "'in <string>' requires string as left operand, not {}",
                            type_name(_py, item)
                        ),
                    );
                }
                let hay_len = string_len(ptr);
                let needle_len = string_len(item_ptr);
                let hay_bytes = std::slice::from_raw_parts(string_bytes(ptr), hay_len);
                let needle_bytes = std::slice::from_raw_parts(string_bytes(item_ptr), needle_len);
                if needle_bytes.is_empty() {
                    return MoltObject::from_bool(true).bits();
                }
                let idx = bytes_find_impl(hay_bytes, needle_bytes);
                return MoltObject::from_bool(idx >= 0).bits();
            }
        }
        molt_contains(container_bits, item_bits)
    })
}

pub(crate) extern "C" fn dict_keys_method(self_bits: u64) -> i64 {
    molt_dict_keys(self_bits) as i64
}

pub(crate) extern "C" fn dict_values_method(self_bits: u64) -> i64 {
    molt_dict_values(self_bits) as i64
}

pub(crate) extern "C" fn dict_items_method(self_bits: u64) -> i64 {
    molt_dict_items(self_bits) as i64
}

pub(crate) extern "C" fn dict_get_method(self_bits: u64, key_bits: u64, default_bits: u64) -> i64 {
    molt_dict_get(self_bits, key_bits, default_bits) as i64
}

pub(crate) extern "C" fn dict_pop_method(
    self_bits: u64,
    key_bits: u64,
    default_bits: u64,
    has_default_bits: u64,
) -> i64 {
    molt_dict_pop(self_bits, key_bits, default_bits, has_default_bits) as i64
}

pub(crate) extern "C" fn dict_clear_method(self_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
            }
            dict_clear_in_place(_py, ptr);
        }
        MoltObject::none().bits() as i64
    })
}

pub(crate) extern "C" fn dict_copy_method(self_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
            }
            let pairs = dict_order(ptr).clone();
            let out_ptr = alloc_dict_with_pairs(_py, pairs.as_slice());
            if out_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            MoltObject::from_ptr(out_ptr).bits() as i64
        }
    })
}

pub(crate) extern "C" fn dict_popitem_method(self_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.popitem expects dict");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.popitem expects dict");
            }
            let order = dict_order(ptr);
            if order.len() < 2 {
                return raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty");
            }
            let key_bits = order[order.len() - 2];
            let val_bits = order[order.len() - 1];
            let item_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
            if item_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, val_bits);
            order.truncate(order.len() - 2);
            let entries = order.len() / 2;
            let table = dict_table(ptr);
            let capacity = dict_table_capacity(entries.max(1));
            dict_rebuild(_py, order, table, capacity);
            MoltObject::from_ptr(item_ptr).bits() as i64
        }
    })
}

pub(crate) extern "C" fn dict_setdefault_method(
    self_bits: u64,
    key_bits: u64,
    default_bits: u64,
) -> i64 {
    molt_dict_setdefault(self_bits, key_bits, default_bits) as i64
}

pub(crate) extern "C" fn dict_fromkeys_method(
    self_bits: u64,
    iterable_bits: u64,
    default_bits: u64,
) -> i64 {
    crate::with_gil_entry!(_py, {
        let class_bits = if let Some(ptr) = maybe_ptr_from_bits(self_bits) {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TYPE {
                    self_bits
                } else {
                    type_of_bits(_py, self_bits)
                }
            }
        } else {
            type_of_bits(_py, self_bits)
        };
        let builtins = builtin_classes(_py);
        if !issubclass_bits(class_bits, builtins.dict) {
            return raise_exception::<_>(_py, "TypeError", "dict.fromkeys expects dict type");
        }
        let capacity_hint = {
            let obj = obj_from_bits(iterable_bits);
            let mut hint = if let Some(ptr) = obj.as_ptr() {
                unsafe {
                    match object_type_id(ptr) {
                        TYPE_ID_LIST => list_len(ptr),
                        TYPE_ID_TUPLE => tuple_len(ptr),
                        TYPE_ID_DICT => dict_len(ptr),
                        TYPE_ID_SET | TYPE_ID_FROZENSET => set_len(ptr),
                        TYPE_ID_DICT_KEYS_VIEW
                        | TYPE_ID_DICT_VALUES_VIEW
                        | TYPE_ID_DICT_ITEMS_VIEW => dict_view_len(ptr),
                        TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => bytes_len(ptr),
                        TYPE_ID_STRING => string_len(ptr),
                        TYPE_ID_INTARRAY => intarray_len(ptr),
                        TYPE_ID_RANGE => {
                            if let Some((start, stop, step)) = range_components_bigint(ptr) {
                                let len = range_len_bigint(&start, &stop, &step);
                                len.to_usize().unwrap_or(usize::MAX)
                            } else {
                                0
                            }
                        }
                        _ => 0,
                    }
                }
            } else {
                0
            };
            let max_entries = (isize::MAX as usize) / 2;
            if hint > max_entries {
                hint = max_entries;
            }
            hint
        };
        let dict_bits = if class_bits == builtins.dict {
            molt_dict_new(capacity_hint as u64)
        } else {
            let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
                return MoltObject::none().bits() as i64;
            };
            unsafe { call_class_init_with_args(_py, class_ptr, &[]) }
        };
        if exception_pending(_py) {
            return MoltObject::none().bits() as i64;
        }
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits() as i64;
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits() as i64;
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let key_bits = elems[0];
                let _ = molt_store_index(dict_bits, key_bits, default_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        dict_bits as i64
    })
}

pub(crate) extern "C" fn dict_update_method(self_bits: u64, other_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        if other_bits == missing_bits(_py) {
            return MoltObject::none().bits() as i64;
        }
        molt_dict_update(self_bits, other_bits) as i64
    })
}

pub(crate) unsafe fn dict_update_set_via_store(
    _py: &PyToken<'_>,
    target_bits: u64,
    key_bits: u64,
    val_bits: u64,
) {
    crate::gil_assert();
    let _ = molt_store_index(target_bits, key_bits, val_bits);
}

pub(crate) unsafe fn dict_inc_in_place(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
    delta_bits: u64,
) -> bool {
    unsafe {
        if !ensure_hashable(_py, key_bits) {
            return false;
        }
        let current_bits =
            dict_get_in_place(_py, dict_ptr, key_bits).unwrap_or(MoltObject::from_int(0).bits());
        if exception_pending(_py) {
            return false;
        }

        if let (Some(current), Some(delta)) = (
            obj_from_bits(current_bits).as_int(),
            obj_from_bits(delta_bits).as_int(),
        ) && let Some(sum) = current.checked_add(delta)
        {
            let sum_bits = MoltObject::from_int(sum).bits();
            dict_set_in_place(_py, dict_ptr, key_bits, sum_bits);
            return !exception_pending(_py);
        }

        let sum_bits = molt_add(current_bits, delta_bits);
        if obj_from_bits(sum_bits).is_none() {
            return false;
        }
        dict_set_in_place(_py, dict_ptr, key_bits, sum_bits);
        dec_ref_bits(_py, sum_bits);
        !exception_pending(_py)
    }
}

fn bits_as_int(bits: u64) -> Option<i64> {
    obj_from_bits(bits).as_int()
}

pub(crate) unsafe fn dict_inc_prehashed_string_key_in_place(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
    delta_bits: u64,
) -> Option<bool> {
    unsafe {
        let key_obj = obj_from_bits(key_bits);
        let key_ptr = key_obj.as_ptr()?;
        if object_type_id(key_ptr) != TYPE_ID_STRING {
            return None;
        }
        let delta = bits_as_int(delta_bits)?;
        let key_bytes = std::slice::from_raw_parts(string_bytes(key_ptr), string_len(key_ptr));
        let hash = hash_string_bytes(_py, key_bytes) as u64;

        let order = dict_order(dict_ptr);
        let table = dict_table(dict_ptr);
        if !table.is_empty() {
            let mask = table.len() - 1;
            let mut slot = (hash as usize) & mask;
            loop {
                let entry = table[slot];
                if entry == 0 {
                    break;
                }
                let entry_idx = entry - 1;
                if entry_idx * 2 >= order.len() {
                    slot = (slot + 1) & mask;
                    continue;
                }
                let entry_key_bits = order[entry_idx * 2];
                let mut keys_match = entry_key_bits == key_bits;
                if !keys_match {
                    let Some(entry_key_ptr) = obj_from_bits(entry_key_bits).as_ptr() else {
                        // continue probing
                        slot = (slot + 1) & mask;
                        continue;
                    };
                    if object_type_id(entry_key_ptr) == TYPE_ID_STRING {
                        let entry_len = string_len(entry_key_ptr);
                        if entry_len == key_bytes.len() {
                            let entry_bytes =
                                std::slice::from_raw_parts(string_bytes(entry_key_ptr), entry_len);
                            keys_match = entry_bytes == key_bytes;
                        }
                    }
                }
                if keys_match {
                    profile_hit_unchecked(&DICT_STR_INT_PREHASH_HIT_COUNT);
                    let val_idx = entry_idx * 2 + 1;
                    let current_bits = order[val_idx];
                    let sum_bits: u64;
                    let mut sum_owned = false;
                    if let Some(current) = obj_from_bits(current_bits).as_int() {
                        if let Some(sum) = current.checked_add(delta) {
                            sum_bits = MoltObject::from_int(sum).bits();
                        } else {
                            sum_bits = molt_add(current_bits, delta_bits);
                            if obj_from_bits(sum_bits).is_none() {
                                return Some(false);
                            }
                            sum_owned = true;
                        }
                    } else {
                        sum_bits = molt_add(current_bits, delta_bits);
                        if obj_from_bits(sum_bits).is_none() {
                            return Some(false);
                        }
                        sum_owned = true;
                    }
                    if current_bits != sum_bits {
                        dec_ref_bits(_py, current_bits);
                        inc_ref_bits(_py, sum_bits);
                        order[val_idx] = sum_bits;
                    }
                    if sum_owned {
                        dec_ref_bits(_py, sum_bits);
                    }
                    return Some(!exception_pending(_py));
                }
                slot = (slot + 1) & mask;
            }
        }

        let sum_bits = MoltObject::from_int(delta).bits();
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                return Some(false);
            }
        }
        order.push(key_bits);
        order.push(sum_bits);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, sum_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        profile_hit_unchecked(&DICT_STR_INT_PREHASH_MISS_COUNT);
        Some(!exception_pending(_py))
    }
}

unsafe fn dict_inc_with_string_token_fallback(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    token: &[u8],
    delta_bits: u64,
    last_bits: &mut u64,
    had_any: &mut bool,
) -> bool {
    unsafe {
        let key_ptr = alloc_string(_py, token);
        if key_ptr.is_null() {
            return false;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        if let Some(done) =
            dict_inc_prehashed_string_key_in_place(_py, dict_ptr, key_bits, delta_bits)
        {
            if !done {
                dec_ref_bits(_py, key_bits);
                return false;
            }
        } else if !dict_inc_in_place(_py, dict_ptr, key_bits, delta_bits) {
            dec_ref_bits(_py, key_bits);
            return false;
        }
        if *had_any && !obj_from_bits(*last_bits).is_none() {
            dec_ref_bits(_py, *last_bits);
        }
        inc_ref_bits(_py, key_bits);
        *last_bits = key_bits;
        *had_any = true;
        dec_ref_bits(_py, key_bits);
        true
    }
}

unsafe fn dict_inc_with_string_token(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    token: &[u8],
    delta_bits: u64,
    last_bits: &mut u64,
    had_any: &mut bool,
) -> bool {
    unsafe {
        let hash = hash_string_bytes(_py, token) as u64;
        {
            let order = dict_order(dict_ptr);
            let table = dict_table(dict_ptr);
            if !table.is_empty() {
                let mask = table.len() - 1;
                let mut slot = (hash as usize) & mask;
                loop {
                    let entry = table[slot];
                    if entry == 0 {
                        break;
                    }
                    let entry_idx = entry - 1;
                    if entry_idx * 2 >= order.len() {
                        slot = (slot + 1) & mask;
                        continue;
                    }
                    let entry_key_bits = order[entry_idx * 2];
                    let Some(entry_key_ptr) = obj_from_bits(entry_key_bits).as_ptr() else {
                        return dict_inc_with_string_token_fallback(
                            _py, dict_ptr, token, delta_bits, last_bits, had_any,
                        );
                    };
                    if object_type_id(entry_key_ptr) != TYPE_ID_STRING {
                        return dict_inc_with_string_token_fallback(
                            _py, dict_ptr, token, delta_bits, last_bits, had_any,
                        );
                    }
                    let entry_len = string_len(entry_key_ptr);
                    if entry_len == token.len() {
                        let entry_bytes =
                            std::slice::from_raw_parts(string_bytes(entry_key_ptr), entry_len);
                        if entry_bytes == token {
                            let val_idx = entry_idx * 2 + 1;
                            let current_bits = order[val_idx];
                            let sum_bits: u64;
                            let mut sum_owned = false;
                            if let (Some(current), Some(delta)) = (
                                obj_from_bits(current_bits).as_int(),
                                obj_from_bits(delta_bits).as_int(),
                            ) {
                                if let Some(sum) = current.checked_add(delta) {
                                    sum_bits = MoltObject::from_int(sum).bits();
                                } else {
                                    sum_bits = molt_add(current_bits, delta_bits);
                                    if obj_from_bits(sum_bits).is_none() {
                                        return false;
                                    }
                                    sum_owned = true;
                                }
                            } else {
                                sum_bits = molt_add(current_bits, delta_bits);
                                if obj_from_bits(sum_bits).is_none() {
                                    return false;
                                }
                                sum_owned = true;
                            }
                            if current_bits != sum_bits {
                                dec_ref_bits(_py, current_bits);
                                inc_ref_bits(_py, sum_bits);
                                order[val_idx] = sum_bits;
                            }
                            if sum_owned {
                                dec_ref_bits(_py, sum_bits);
                            }
                            if *had_any && !obj_from_bits(*last_bits).is_none() {
                                dec_ref_bits(_py, *last_bits);
                            }
                            inc_ref_bits(_py, entry_key_bits);
                            *last_bits = entry_key_bits;
                            *had_any = true;
                            return true;
                        }
                    }
                    slot = (slot + 1) & mask;
                }
            }
        }

        let key_ptr = alloc_string(_py, token);
        if key_ptr.is_null() {
            return false;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let zero_bits = MoltObject::from_int(0).bits();
        let sum_bits: u64;
        let mut sum_owned = false;
        if let Some(delta) = obj_from_bits(delta_bits).as_int() {
            sum_bits = MoltObject::from_int(delta).bits();
        } else {
            sum_bits = molt_add(zero_bits, delta_bits);
            if obj_from_bits(sum_bits).is_none() {
                dec_ref_bits(_py, key_bits);
                return false;
            }
            sum_owned = true;
        }
        let order = dict_order(dict_ptr);
        let table = dict_table(dict_ptr);
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                if sum_owned {
                    dec_ref_bits(_py, sum_bits);
                }
                dec_ref_bits(_py, key_bits);
                return false;
            }
        }
        order.push(key_bits);
        order.push(sum_bits);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, sum_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if sum_owned {
            dec_ref_bits(_py, sum_bits);
        }
        if *had_any && !obj_from_bits(*last_bits).is_none() {
            dec_ref_bits(_py, *last_bits);
        }
        inc_ref_bits(_py, key_bits);
        *last_bits = key_bits;
        *had_any = true;
        dec_ref_bits(_py, key_bits);
        true
    }
}

unsafe fn dict_setdefault_empty_list_with_string_token(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    token: &[u8],
) -> Option<u64> {
    unsafe {
        let hash = hash_string_bytes(_py, token) as u64;
        {
            let order = dict_order(dict_ptr);
            let table = dict_table(dict_ptr);
            if !table.is_empty() {
                let mask = table.len() - 1;
                let mut slot = (hash as usize) & mask;
                loop {
                    let entry = table[slot];
                    if entry == 0 {
                        break;
                    }
                    let entry_idx = entry - 1;
                    if entry_idx * 2 >= order.len() {
                        slot = (slot + 1) & mask;
                        continue;
                    }
                    let entry_key_bits = order[entry_idx * 2];
                    let Some(entry_key_ptr) = obj_from_bits(entry_key_bits).as_ptr() else {
                        slot = (slot + 1) & mask;
                        continue;
                    };
                    if object_type_id(entry_key_ptr) == TYPE_ID_STRING {
                        let entry_len = string_len(entry_key_ptr);
                        if entry_len == token.len() {
                            let entry_bytes =
                                std::slice::from_raw_parts(string_bytes(entry_key_ptr), entry_len);
                            if entry_bytes == token {
                                let val_bits = order[entry_idx * 2 + 1];
                                inc_ref_bits(_py, val_bits);
                                return Some(val_bits);
                            }
                        }
                    }
                    slot = (slot + 1) & mask;
                }
            }
        }

        let key_ptr = alloc_string(_py, token);
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let default_ptr = alloc_list(_py, &[]);
        if default_ptr.is_null() {
            dec_ref_bits(_py, key_bits);
            return None;
        }
        let default_bits = MoltObject::from_ptr(default_ptr).bits();
        let order = dict_order(dict_ptr);
        let table = dict_table(dict_ptr);
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                dec_ref_bits(_py, default_bits);
                dec_ref_bits(_py, key_bits);
                return None;
            }
        }
        order.push(key_bits);
        order.push(default_bits);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, default_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if exception_pending(_py) {
            dec_ref_bits(_py, default_bits);
            dec_ref_bits(_py, key_bits);
            return None;
        }
        inc_ref_bits(_py, default_bits);
        dec_ref_bits(_py, default_bits);
        dec_ref_bits(_py, key_bits);
        Some(default_bits)
    }
}

unsafe fn split_dict_inc_result_tuple(_py: &PyToken<'_>, last_bits: u64, had_any: bool) -> u64 {
    let had_any_bits = MoltObject::from_bool(had_any).bits();
    let pair_ptr = alloc_tuple(_py, &[last_bits, had_any_bits]);
    if pair_ptr.is_null() {
        if had_any && !obj_from_bits(last_bits).is_none() {
            dec_ref_bits(_py, last_bits);
        }
        return MoltObject::none().bits();
    }
    if had_any && !obj_from_bits(last_bits).is_none() {
        dec_ref_bits(_py, last_bits);
    }
    MoltObject::from_ptr(pair_ptr).bits()
}

fn parse_ascii_i64_field(_py: &PyToken<'_>, field: &[u8]) -> Option<i64> {
    let mut start = 0usize;
    let mut end = field.len();
    while start < end && field[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && field[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let trimmed = &field[start..end];
    if trimmed.is_empty() {
        profile_hit_unchecked(&ASCII_I64_PARSE_FAIL_COUNT);
        raise_exception::<()>(
            _py,
            "ValueError",
            "invalid literal for int() with base 10: ''",
        );
        return None;
    }
    let mut idx = 0usize;
    let mut neg = false;
    if trimmed[0] == b'+' {
        idx = 1;
    } else if trimmed[0] == b'-' {
        neg = true;
        idx = 1;
    }
    if idx >= trimmed.len() {
        profile_hit_unchecked(&ASCII_I64_PARSE_FAIL_COUNT);
        let shown = String::from_utf8_lossy(trimmed);
        let msg = format!("invalid literal for int() with base 10: '{shown}'");
        raise_exception::<()>(_py, "ValueError", &msg);
        return None;
    }
    let mut value: i128 = 0;
    while idx < trimmed.len() {
        let b = trimmed[idx];
        if !b.is_ascii_digit() {
            profile_hit_unchecked(&ASCII_I64_PARSE_FAIL_COUNT);
            let shown = String::from_utf8_lossy(trimmed);
            let msg = format!("invalid literal for int() with base 10: '{shown}'");
            raise_exception::<()>(_py, "ValueError", &msg);
            return None;
        }
        value = value * 10 + i128::from((b - b'0') as i64);
        idx += 1;
    }
    if neg {
        value = -value;
    }
    if value < i128::from(i64::MIN) || value > i128::from(i64::MAX) {
        profile_hit_unchecked(&ASCII_I64_PARSE_FAIL_COUNT);
        let shown = String::from_utf8_lossy(trimmed);
        let msg = format!("invalid literal for int() with base 10: '{shown}'");
        raise_exception::<()>(_py, "ValueError", &msg);
        None
    } else {
        Some(value as i64)
    }
}

#[inline]
fn is_ascii_split_whitespace_byte(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\r' | b'\t' | 0x0b | 0x0c)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn find_ascii_split_whitespace_sse2(bytes: &[u8], start: usize) -> usize {
    use std::arch::x86_64::*;
    let mut i = start;
    let len = bytes.len();
    let sp = _mm_set1_epi8(b' ' as i8);
    let nl = _mm_set1_epi8(b'\n' as i8);
    let cr = _mm_set1_epi8(b'\r' as i8);
    let tab = _mm_set1_epi8(b'\t' as i8);
    let vt = _mm_set1_epi8(0x0b_i8);
    let ff = _mm_set1_epi8(0x0c_i8);
    while i + 16 <= len {
        let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
        let mut mask_vec = _mm_or_si128(_mm_cmpeq_epi8(chunk, sp), _mm_cmpeq_epi8(chunk, nl));
        mask_vec = _mm_or_si128(mask_vec, _mm_cmpeq_epi8(chunk, cr));
        mask_vec = _mm_or_si128(mask_vec, _mm_cmpeq_epi8(chunk, tab));
        mask_vec = _mm_or_si128(mask_vec, _mm_cmpeq_epi8(chunk, vt));
        mask_vec = _mm_or_si128(mask_vec, _mm_cmpeq_epi8(chunk, ff));
        let mask = _mm_movemask_epi8(mask_vec) as u32;
        if mask != 0 {
            return i + mask.trailing_zeros() as usize;
        }
        i += 16;
    }
    while i < len {
        if is_ascii_split_whitespace_byte(bytes[i]) {
            return i;
        }
        i += 1;
    }
    len
}

/// NEON variant: find first ASCII whitespace byte in slice starting at `start`.
#[cfg(target_arch = "aarch64")]
unsafe fn find_ascii_split_whitespace_neon(bytes: &[u8], start: usize) -> usize {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = start;
        let len = bytes.len();
        let sp = vdupq_n_u8(b' ');
        let nl = vdupq_n_u8(b'\n');
        let cr = vdupq_n_u8(b'\r');
        let tab = vdupq_n_u8(b'\t');
        let vt = vdupq_n_u8(0x0b);
        let ff = vdupq_n_u8(0x0c);
        while i + 16 <= len {
            let chunk = vld1q_u8(bytes.as_ptr().add(i));
            let is_ws = vorrq_u8(
                vorrq_u8(
                    vorrq_u8(vceqq_u8(chunk, sp), vceqq_u8(chunk, nl)),
                    vceqq_u8(chunk, cr),
                ),
                vorrq_u8(
                    vceqq_u8(chunk, tab),
                    vorrq_u8(vceqq_u8(chunk, vt), vceqq_u8(chunk, ff)),
                ),
            );
            if vmaxvq_u8(is_ws) != 0 {
                // Found whitespace in this chunk — scan for exact position
                let mut buf = [0u8; 16];
                vst1q_u8(buf.as_mut_ptr(), is_ws);
                for (j, &byte) in buf.iter().enumerate() {
                    if byte != 0 {
                        return i + j;
                    }
                }
            }
            i += 16;
        }
        while i < len {
            if is_ascii_split_whitespace_byte(bytes[i]) {
                return i;
            }
            i += 1;
        }
        len
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn find_ascii_split_whitespace_wasm32(bytes: &[u8], start: usize) -> usize {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = start;
        let len = bytes.len();
        let sp = u8x16_splat(b' ');
        let nl = u8x16_splat(b'\n');
        let cr = u8x16_splat(b'\r');
        let tab = u8x16_splat(b'\t');
        let vt = u8x16_splat(0x0b);
        let ff = u8x16_splat(0x0c);
        while i + 16 <= len {
            let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
            let is_ws = v128_or(
                v128_or(
                    v128_or(u8x16_eq(chunk, sp), u8x16_eq(chunk, nl)),
                    u8x16_eq(chunk, cr),
                ),
                v128_or(
                    u8x16_eq(chunk, tab),
                    v128_or(u8x16_eq(chunk, vt), u8x16_eq(chunk, ff)),
                ),
            );
            let mask = u8x16_bitmask(is_ws);
            if mask != 0 {
                return i + mask.trailing_zeros() as usize;
            }
            i += 16;
        }
        while i < len {
            if is_ascii_split_whitespace_byte(bytes[i]) {
                return i;
            }
            i += 1;
        }
        len
    }
}

fn find_ascii_split_whitespace(bytes: &[u8], start: usize) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { find_ascii_split_whitespace_sse2(bytes, start) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { find_ascii_split_whitespace_neon(bytes, start) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { find_ascii_split_whitespace_wasm32(bytes, start) };
    }
    #[allow(unreachable_code)]
    {
        let mut i = start;
        while i < bytes.len() {
            if is_ascii_split_whitespace_byte(bytes[i]) {
                return i;
            }
            i += 1;
        }
        bytes.len()
    }
}

unsafe fn split_ascii_whitespace_dict_inc_tokens(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    line_bytes: &[u8],
    delta_bits: u64,
    last_bits: &mut u64,
    had_any: &mut bool,
) -> bool {
    unsafe {
        let mut idx = 0usize;
        let len = line_bytes.len();
        while idx < len {
            while idx < len && is_ascii_split_whitespace_byte(line_bytes[idx]) {
                idx += 1;
            }
            if idx >= len {
                break;
            }
            let token_start = idx;
            let token_end = find_ascii_split_whitespace(line_bytes, token_start);
            if !dict_inc_with_string_token(
                _py,
                dict_ptr,
                &line_bytes[token_start..token_end],
                delta_bits,
                last_bits,
                had_any,
            ) {
                return false;
            }
            idx = token_end;
        }
        true
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_ws_dict_inc(
    line_bits: u64,
    dict_bits: u64,
    delta_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let line_obj = obj_from_bits(line_bits);
        let dict_obj = obj_from_bits(dict_bits);
        let Some(line_ptr) = line_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "split expects str");
        };
        let Some(dict_ptr_raw) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
        };
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "split expects str");
            }
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, dict_ptr_raw) else {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            }
            let line_bytes =
                std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            let mut last_bits = MoltObject::none().bits();
            let mut had_any = false;
            if line_bytes.is_ascii() {
                profile_hit_unchecked(&SPLIT_WS_ASCII_FAST_PATH_COUNT);
                if !split_ascii_whitespace_dict_inc_tokens(
                    _py,
                    dict_ptr,
                    line_bytes,
                    delta_bits,
                    &mut last_bits,
                    &mut had_any,
                ) {
                    return MoltObject::none().bits();
                }
            } else {
                profile_hit_unchecked(&SPLIT_WS_UNICODE_PATH_COUNT);
                let Ok(line_str) = std::str::from_utf8(line_bytes) else {
                    return MoltObject::none().bits();
                };
                for part in line_str.split_whitespace() {
                    if !dict_inc_with_string_token(
                        _py,
                        dict_ptr,
                        part.as_bytes(),
                        delta_bits,
                        &mut last_bits,
                        &mut had_any,
                    ) {
                        return MoltObject::none().bits();
                    }
                }
            }
            split_dict_inc_result_tuple(_py, last_bits, had_any)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_sep_dict_inc(
    line_bits: u64,
    sep_bits: u64,
    dict_bits: u64,
    delta_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let line_obj = obj_from_bits(line_bits);
        let sep_obj = obj_from_bits(sep_bits);
        let dict_obj = obj_from_bits(dict_bits);
        let Some(line_ptr) = line_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "split expects str");
        };
        let Some(sep_ptr) = sep_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "must be str or None");
        };
        let Some(dict_ptr_raw) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
        };
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "split expects str");
            }
            if object_type_id(sep_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "must be str or None");
            }
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, dict_ptr_raw) else {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            }

            let line_bytes =
                std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let mut last_bits = MoltObject::none().bits();
            let mut had_any = false;
            let mut start = 0usize;
            if sep_bytes.len() == 1 {
                for idx in memchr::memchr_iter(sep_bytes[0], line_bytes) {
                    if !dict_inc_with_string_token(
                        _py,
                        dict_ptr,
                        &line_bytes[start..idx],
                        delta_bits,
                        &mut last_bits,
                        &mut had_any,
                    ) {
                        return MoltObject::none().bits();
                    }
                    start = idx + 1;
                }
            } else {
                let finder = memmem::Finder::new(sep_bytes);
                for idx in finder.find_iter(line_bytes) {
                    if !dict_inc_with_string_token(
                        _py,
                        dict_ptr,
                        &line_bytes[start..idx],
                        delta_bits,
                        &mut last_bits,
                        &mut had_any,
                    ) {
                        return MoltObject::none().bits();
                    }
                    start = idx + sep_bytes.len();
                }
            }
            if !dict_inc_with_string_token(
                _py,
                dict_ptr,
                &line_bytes[start..],
                delta_bits,
                &mut last_bits,
                &mut had_any,
            ) {
                return MoltObject::none().bits();
            }
            split_dict_inc_result_tuple(_py, last_bits, had_any)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_taq_ingest_line(
    dict_bits: u64,
    line_bits: u64,
    bucket_size_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        profile_hit_unchecked(&TAQ_INGEST_CALL_COUNT);
        let dict_obj = obj_from_bits(dict_bits);
        let line_obj = obj_from_bits(line_bits);
        let Some(dict_ptr_raw) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects dict");
        };
        let Some(line_ptr) = line_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects str");
        };
        let Some(bucket_size) = obj_from_bits(bucket_size_bits).as_int() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "TAQ ingest expects integer bucket size",
            );
        };
        if bucket_size == 0 {
            return raise_exception::<_>(
                _py,
                "ZeroDivisionError",
                "integer division or modulo by zero",
            );
        }
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects str");
            }
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, dict_ptr_raw) else {
                return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects dict");
            }

            let line_bytes =
                std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            let mut field_idx = 0usize;
            let mut field_start = 0usize;
            let mut ts_field: Option<&[u8]> = None;
            let mut sym_field: Option<&[u8]> = None;
            let mut vol_field: Option<&[u8]> = None;
            for idx in 0..=line_bytes.len() {
                if idx == line_bytes.len() || line_bytes[idx] == b'|' {
                    let field = &line_bytes[field_start..idx];
                    match field_idx {
                        0 => ts_field = Some(field),
                        2 => sym_field = Some(field),
                        4 => {
                            vol_field = Some(field);
                            break;
                        }
                        _ => {}
                    }
                    field_idx += 1;
                    field_start = idx + 1;
                }
            }
            let Some(ts_field) = ts_field else {
                return raise_exception::<_>(_py, "IndexError", "list index out of range");
            };
            let Some(sym_field) = sym_field else {
                return raise_exception::<_>(_py, "IndexError", "list index out of range");
            };
            let Some(vol_field) = vol_field else {
                return raise_exception::<_>(_py, "IndexError", "list index out of range");
            };
            if ts_field == b"END" || vol_field == b"ENDP" {
                profile_hit_unchecked(&TAQ_INGEST_SKIP_MARKER_COUNT);
                return MoltObject::from_bool(false).bits();
            }
            let Some(timestamp) = parse_ascii_i64_field(_py, ts_field) else {
                return MoltObject::none().bits();
            };
            let Some(volume) = parse_ascii_i64_field(_py, vol_field) else {
                return MoltObject::none().bits();
            };
            let Some(series_bits) =
                dict_setdefault_empty_list_with_string_token(_py, dict_ptr, sym_field)
            else {
                return MoltObject::none().bits();
            };
            let bucket_bits = MoltObject::from_int(timestamp.div_euclid(bucket_size)).bits();
            let volume_bits = MoltObject::from_int(volume).bits();
            let pair_ptr = alloc_tuple(_py, &[bucket_bits, volume_bits]);
            if pair_ptr.is_null() {
                dec_ref_bits(_py, series_bits);
                return MoltObject::none().bits();
            }
            let pair_bits = MoltObject::from_ptr(pair_ptr).bits();
            let appended = if let Some(series_ptr) = obj_from_bits(series_bits).as_ptr() {
                if object_type_id(series_ptr) == TYPE_ID_LIST {
                    let _ = molt_list_append(series_bits, pair_bits);
                    !exception_pending(_py)
                } else {
                    let Some(append_name_bits) = attr_name_bits_from_bytes(_py, b"append") else {
                        dec_ref_bits(_py, pair_bits);
                        dec_ref_bits(_py, series_bits);
                        return MoltObject::none().bits();
                    };
                    let method_bits = attr_lookup_ptr(_py, series_ptr, append_name_bits);
                    dec_ref_bits(_py, append_name_bits);
                    let Some(method_bits) = method_bits else {
                        dec_ref_bits(_py, pair_bits);
                        dec_ref_bits(_py, series_bits);
                        return MoltObject::none().bits();
                    };
                    let out_bits = call_callable1(_py, method_bits, pair_bits);
                    if maybe_ptr_from_bits(out_bits).is_some() {
                        dec_ref_bits(_py, out_bits);
                    }
                    !exception_pending(_py)
                }
            } else {
                false
            };
            dec_ref_bits(_py, pair_bits);
            dec_ref_bits(_py, series_bits);
            if !appended {
                return MoltObject::none().bits();
            }
            MoltObject::from_bool(true).bits()
        }
    })
}

pub(crate) unsafe fn list_from_iter_bits(_py: &PyToken<'_>, other_bits: u64) -> Option<u64> {
    let list_ptr = alloc_list(_py, &[]);
    if list_ptr.is_null() {
        return None;
    }
    let list_bits = MoltObject::from_ptr(list_ptr).bits();
    let _ = molt_list_extend(list_bits, other_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, list_bits);
        return None;
    }
    Some(list_bits)
}

pub(crate) unsafe fn tuple_from_iter_bits(_py: &PyToken<'_>, other_bits: u64) -> Option<u64> {
    unsafe {
        let obj = obj_from_bits(other_bits);
        if let Some(ptr) = obj.as_ptr() {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TUPLE {
                inc_ref_bits(_py, other_bits);
                return Some(other_bits);
            }
            if type_id == TYPE_ID_LIST {
                let tuple_bits = molt_tuple_from_list(other_bits);
                if obj_from_bits(tuple_bits).is_none() {
                    return None;
                }
                return Some(tuple_bits);
            }
        }
        let list_bits = list_from_iter_bits(_py, other_bits)?;
        let tuple_bits = molt_tuple_from_list(list_bits);
        dec_ref_bits(_py, list_bits);
        if obj_from_bits(tuple_bits).is_none() {
            return None;
        }
        Some(tuple_bits)
    }
}

pub(crate) unsafe fn frozenset_from_iter_bits(_py: &PyToken<'_>, other_bits: u64) -> Option<u64> {
    unsafe {
        let obj = obj_from_bits(other_bits);
        if let Some(ptr) = obj.as_ptr()
            && object_type_id(ptr) == TYPE_ID_FROZENSET
        {
            inc_ref_bits(_py, other_bits);
            return Some(other_bits);
        }
        let iter_bits = molt_iter(other_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, other_bits);
        }
        let set_bits = molt_frozenset_new(0);
        let set_ptr = obj_from_bits(set_bits).as_ptr()?;
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let pair_ptr = pair_obj.as_ptr()?;
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return None;
            }
            let pair_elems = seq_vec_ref(pair_ptr);
            if pair_elems.len() < 2 {
                return None;
            }
            let done_bits = pair_elems[1];
            if is_truthy(_py, obj_from_bits(done_bits)) {
                break;
            }
            let val_bits = pair_elems[0];
            set_add_in_place(_py, set_ptr, val_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, set_bits);
                return None;
            }
        }
        Some(set_bits)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref_obj(bits: u64) {
    // Fast path: skip GIL for non-pointer values (ints, floats, bools, none).
    if !obj_from_bits(bits).is_ptr() {
        return;
    }
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            unsafe { molt_inc_ref(ptr) };
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref_obj(bits: u64) {
    // Fast path: skip GIL for non-pointer values (ints, floats, bools, none).
    let obj = obj_from_bits(bits);
    if !obj.is_ptr() {
        return;
    }
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                // Validate type_id before dec_ref to prevent use-after-free
                // from codegen double-free bugs. A freed object's header is
                // overwritten by the allocator's freelist metadata, producing
                // invalid type_ids (>300 or 0). Skip dec_ref for these.
                let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *const MoltHeader;
                let type_id = (*header_ptr).type_id;
                if type_id == 0 || type_id > 300 {
                    return;
                }
                molt_dec_ref(ptr);
            };
        }
    })
}

/// Batched `inc_ref`: increment the refcount by `count` in a single atomic
/// operation. Returns the input bits unchanged (convenience for chaining).
#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref_n(bits: u64, count: u32) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            unsafe { crate::object::inc_ref_n_ptr(_py, ptr, count) };
        }
    });
    bits
}

/// Batched `dec_ref`: decrement the refcount by calling `dec_ref` `count`
/// times. (Cannot use a single atomic subtract because each decrement may
/// trigger deallocation at zero.)
#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref_n(bits: u64, count: u32) {
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            for _ in 0..count {
                unsafe { molt_dec_ref(ptr) };
            }
        }
    })
}

/// Outlined sequence unpacking helper. Validates that the sequence length
/// matches `expected_count`, extracts each element (with incref), and writes
/// element bits to `output_ptr[0..expected_count]`.
///
/// Returns 0 on success.  On length mismatch a `ValueError` is raised through
/// the normal exception-pending mechanism and `MoltObject::none().bits()` is
/// returned so the caller can short-circuit.
#[unsafe(no_mangle)]
pub extern "C" fn molt_unpack_sequence(
    seq_bits: u64,
    expected_count: u64,
    output_ptr: *mut u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(seq_bits);
        let expected = expected_count as usize;
        let Some(ptr) = obj.as_ptr() else {
            raise_exception::<u64>(_py, "TypeError", "cannot unpack non-sequence");
            return MoltObject::none().bits();
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems: &[u64] = seq_vec_ref(ptr);
                let actual = elems.len();
                if actual < expected {
                    let msg = format!(
                        "not enough values to unpack (expected {}, got {})",
                        expected, actual
                    );
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    return MoltObject::none().bits();
                }
                if actual > expected {
                    let msg = format!("too many values to unpack (expected {}, got {})", expected, actual);
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    return MoltObject::none().bits();
                }
                let out_slice = std::slice::from_raw_parts_mut(output_ptr, expected);
                for (i, &bits) in elems.iter().enumerate().take(expected) {
                    inc_ref_bits(_py, bits);
                    out_slice[i] = bits;
                }
            } else {
                // Generic iterable: materialize via iter/next.
                let iter_bits = molt_iter(seq_bits);
                if obj_from_bits(iter_bits).is_none() {
                    raise_exception::<u64>(_py, "TypeError", "cannot unpack non-sequence");
                    return MoltObject::none().bits();
                }
                let out_slice = std::slice::from_raw_parts_mut(output_ptr, expected);
                let mut count = 0usize;
                loop {
                    let pair_bits = molt_iter_next(iter_bits);
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        break;
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        break;
                    }
                    let pair_elems = seq_vec_ref(pair_ptr);
                    if pair_elems.len() < 2 {
                        break;
                    }
                    let done = is_truthy(_py, obj_from_bits(pair_elems[1]));
                    if done {
                        break;
                    }
                    let val_bits = pair_elems[0];
                    if count < expected {
                        inc_ref_bits(_py, val_bits);
                        out_slice[count] = val_bits;
                    }
                    count += 1;
                    if count > expected {
                        // Try to exhaust iterator for the "got N" count.
                        // Cap at 1024 extra iterations to avoid hanging on
                        // infinite iterators.
                        let mut extra = 0usize;
                        let exhausted = loop {
                            if extra >= 1024 { break false; }
                            let extra_bits = molt_iter_next(iter_bits);
                            let extra_obj = obj_from_bits(extra_bits);
                            let Some(extra_ptr) = extra_obj.as_ptr() else { break true; };
                            if object_type_id(extra_ptr) != TYPE_ID_TUPLE { break true; }
                            let extra_elems = seq_vec_ref(extra_ptr);
                            if extra_elems.len() < 2 { break true; }
                            let done = is_truthy(_py, obj_from_bits(extra_elems[1]));
                            if done { break true; }
                            extra += 1;
                        };
                        count += extra;
                        dec_ref_bits(_py, iter_bits);
                        let msg = if exhausted {
                            format!("too many values to unpack (expected {}, got {})", expected, count)
                        } else {
                            // Iterator didn't terminate — report without count
                            format!("too many values to unpack (expected {})", expected)
                        };
                        raise_exception::<u64>(_py, "ValueError", &msg);
                        return MoltObject::none().bits();
                    }
                }
                dec_ref_bits(_py, iter_bits);
                if count < expected {
                    // If an exception is already pending (e.g. the iterator
                    // raised RuntimeError, not StopIteration), propagate that
                    // exception instead of replacing it with ValueError.
                    if exception_pending(_py) {
                        for i in 0..count {
                            dec_ref_bits(_py, out_slice[i]);
                        }
                        return MoltObject::none().bits();
                    }
                    let msg = format!(
                        "not enough values to unpack (expected {}, got {})",
                        expected, count
                    );
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    // Dec-ref any already-extracted values.
                    for i in 0..count {
                        dec_ref_bits(_py, out_slice[i]);
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        0u64
    })
}

unsafe fn dict_subclass_storage_bits(_py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    unsafe {
        let debug = std::env::var("MOLT_DEBUG_DICT_SUBCLASS").as_deref() == Ok("1");
        let class_bits = object_class_bits(ptr);
        if class_bits == 0 {
            if debug {
                eprintln!(
                    "dict_subclass_storage_bits: no class bits for ptr=0x{:x}",
                    ptr as usize
                );
            }
            return None;
        }
        let builtins = builtin_classes(_py);
        if !issubclass_bits(class_bits, builtins.dict) {
            if debug {
                let class_name = class_name_for_error(class_bits);
                if class_name == "defaultdict" || class_name == "dict" {
                    eprintln!(
                        "dict_subclass_storage_bits: class not dict-subclass ptr=0x{:x} class={}",
                        ptr as usize, class_name
                    );
                }
            }
            return None;
        }
        let payload = object_payload_size(ptr);
        if debug {
            eprintln!(
                "dict_subclass_storage_bits: ptr=0x{:x} payload={}",
                ptr as usize, payload
            );
        }
        if payload < 2 * std::mem::size_of::<u64>() {
            if debug {
                eprintln!(
                    "dict_subclass_storage_bits: using sidecar storage for ptr=0x{:x}",
                    ptr as usize
                );
            }
            let slot = PtrSlot(ptr);
            let mut storage = runtime_state(_py).dict_subclass_storage.lock().unwrap();
            if let Some(bits) = storage.get(&slot).copied() {
                return Some(bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return None;
            }
            let storage_bits = MoltObject::from_ptr(dict_ptr).bits();
            storage.insert(slot, storage_bits);
            return Some(storage_bits);
        }
        let storage_ptr = ptr.add(payload - 2 * std::mem::size_of::<u64>()) as *mut u64;
        let mut storage_bits = *storage_ptr;
        let mut needs_init = storage_bits == 0;
        let mut dict_ptr_opt = if storage_bits == 0 {
            None
        } else {
            obj_from_bits(storage_bits).as_ptr()
        };
        if let Some(dict_ptr) = dict_ptr_opt {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                if debug {
                    eprintln!(
                        "dict_subclass_storage_bits: storage not dict ptr=0x{:x} bits=0x{:x} type_id={}",
                        ptr as usize,
                        storage_bits,
                        object_type_id(dict_ptr)
                    );
                }
                needs_init = true;
            }
        } else if storage_bits != 0 {
            needs_init = true;
        }
        if needs_init {
            let dict_ptr = alloc_dict_with_pairs(_py, &[]);
            if dict_ptr.is_null() {
                return None;
            }
            storage_bits = MoltObject::from_ptr(dict_ptr).bits();
            *storage_ptr = storage_bits;
            dict_ptr_opt = Some(dict_ptr);
            if debug {
                eprintln!(
                    "dict_subclass_storage_bits: initialized storage ptr=0x{:x} bits=0x{:x}",
                    ptr as usize, storage_bits
                );
            }
        }
        if let Some(dict_ptr) = dict_ptr_opt {
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return None;
            }
        } else {
            return None;
        }
        Some(storage_bits)
    }
}

pub(crate) unsafe fn dict_like_bits_from_ptr(_py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    unsafe {
        if object_type_id(ptr) == TYPE_ID_DICT {
            return Some(MoltObject::from_ptr(ptr).bits());
        }
        if object_type_id(ptr) == TYPE_ID_OBJECT {
            return dict_subclass_storage_bits(_py, ptr);
        }
        None
    }
}

pub(crate) fn class_break_cycles(_py: &PyToken<'_>, bits: u64) {
    crate::gil_assert();
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_TYPE {
            return;
        }
        let none_bits = MoltObject::none().bits();
        let bases_bits = class_bases_bits(ptr);
        let mro_bits = class_mro_bits(ptr);
        if !obj_from_bits(bases_bits).is_none() {
            dec_ref_bits(_py, bases_bits);
        }
        if !obj_from_bits(mro_bits).is_none() {
            dec_ref_bits(_py, mro_bits);
        }
        class_set_bases_bits(ptr, none_bits);
        class_set_mro_bits(ptr, none_bits);
        class_set_annotations_bits(_py, ptr, 0u64);
        class_set_annotate_bits(_py, ptr, 0u64);
        let dict_bits = class_dict_bits(ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
        {
            dict_clear_in_place(_py, dict_ptr);
        }
    }
}

pub(crate) fn tuple_from_isize_slice(_py: &PyToken<'_>, values: &[isize]) -> u64 {
    let mut elems = Vec::with_capacity(values.len());
    for &val in values {
        elems.push(MoltObject::from_int(val as i64).bits());
    }
    let ptr = alloc_tuple(_py, &elems);
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

pub(crate) fn is_truthy(_py: &PyToken<'_>, obj: MoltObject) -> bool {
    if obj.is_none() {
        return false;
    }
    if let Some(b) = obj.as_bool() {
        return b;
    }
    if let Some(i) = to_i64(obj) {
        return i != 0;
    }
    if let Some(f) = obj.as_float() {
        return f != 0.0;
    }
    if let Some(big) = to_bigint(obj) {
        return !big.is_zero();
    }
    if let Some(ptr) = obj.as_ptr() {
        if ptr.is_null() {
            return false;
        }
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                return string_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BYTES {
                return bytes_len(ptr) > 0;
            }
            if type_id == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return value.re != 0.0 || value.im != 0.0;
            }
            if type_id == TYPE_ID_BYTEARRAY {
                return bytes_len(ptr) > 0;
            }
            if type_id == TYPE_ID_LIST {
                return list_len(ptr) > 0;
            }
            if type_id == TYPE_ID_TUPLE {
                return tuple_len(ptr) > 0;
            }
            if type_id == TYPE_ID_INTARRAY {
                return intarray_len(ptr) > 0;
            }
            if type_id == TYPE_ID_DICT {
                return dict_len(ptr) > 0;
            }
            if type_id == TYPE_ID_SET {
                return set_len(ptr) > 0;
            }
            if type_id == TYPE_ID_FROZENSET {
                return set_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BUFFER2D {
                let buf_ptr = buffer2d_ptr(ptr);
                if buf_ptr.is_null() {
                    return false;
                }
                let buf = &*buf_ptr;
                return buf.rows.saturating_mul(buf.cols) > 0;
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                return dict_view_len(ptr) > 0;
            }
            if type_id == TYPE_ID_RANGE {
                let Some((start, stop, step)) = range_components_bigint(ptr) else {
                    return false;
                };
                let len = range_len_bigint(&start, &stop, &step);
                return !len.is_zero();
            }
            if type_id == TYPE_ID_ITER {
                return true;
            }
            if type_id == TYPE_ID_GENERATOR {
                return true;
            }
            if type_id == TYPE_ID_ASYNC_GENERATOR {
                return true;
            }
            if type_id == TYPE_ID_ENUMERATE {
                return true;
            }
            if type_id == TYPE_ID_CALL_ITER
                || type_id == TYPE_ID_REVERSED
                || type_id == TYPE_ID_ZIP
                || type_id == TYPE_ID_MAP
                || type_id == TYPE_ID_FILTER
            {
                return true;
            }
            if type_id == TYPE_ID_SLICE {
                return true;
            }
            if type_id == TYPE_ID_CONTEXT_MANAGER {
                return true;
            }
            if type_id == TYPE_ID_FILE_HANDLE {
                return true;
            }
            if type_id == TYPE_ID_OBJECT || type_id == TYPE_ID_DATACLASS {
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__bool__") {
                    let call_bits = attr_lookup_ptr_allow_missing(_py, ptr, name_bits);
                    dec_ref_bits(_py, name_bits);
                    if let Some(call_bits) = call_bits {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, res_bits);
                            return false;
                        }
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(b) = res_obj.as_bool() {
                            dec_ref_bits(_py, res_bits);
                            return b;
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        dec_ref_bits(_py, res_bits);
                        let msg = format!("__bool__ should return bool, returned {res_type}");
                        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                        return false;
                    }
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__len__") {
                    let call_bits = attr_lookup_ptr_allow_missing(_py, ptr, name_bits);
                    dec_ref_bits(_py, name_bits);
                    if let Some(call_bits) = call_bits {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, res_bits);
                            return false;
                        }
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(i) = to_i64(res_obj) {
                            dec_ref_bits(_py, res_bits);
                            if i < 0 {
                                let _ = raise_exception::<u64>(
                                    _py,
                                    "ValueError",
                                    "__len__() should return >= 0",
                                );
                                return false;
                            }
                            return i != 0;
                        }
                        if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                            let big = bigint_ref(big_ptr);
                            if big.is_negative() {
                                let _ = raise_exception::<u64>(
                                    _py,
                                    "ValueError",
                                    "__len__() should return >= 0",
                                );
                                dec_ref_bits(_py, res_bits);
                                return false;
                            }
                            let Some(len) = big.to_usize() else {
                                let _ = raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "cannot fit 'int' into an index-sized integer",
                                );
                                dec_ref_bits(_py, res_bits);
                                return false;
                            };
                            if len > i64::MAX as usize {
                                let _ = raise_exception::<u64>(
                                    _py,
                                    "OverflowError",
                                    "cannot fit 'int' into an index-sized integer",
                                );
                                dec_ref_bits(_py, res_bits);
                                return false;
                            }
                            dec_ref_bits(_py, res_bits);
                            return len != 0;
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        dec_ref_bits(_py, res_bits);
                        let msg =
                            format!("'{}' object cannot be interpreted as an integer", res_type);
                        let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                        return false;
                    }
                }
                return true;
            }
            return true;
        }
    }
    false
}

fn union_type_display_name() -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        let minor = std::env::var("MOLT_SYS_VERSION_INFO")
            .ok()
            .and_then(|raw| {
                let mut parts = raw.split(',');
                let _major = parts.next()?.trim().parse::<i64>().ok()?;
                let minor = parts.next()?.trim().parse::<i64>().ok()?;
                Some(minor)
            })
            .unwrap_or(14);
        if minor >= 14 {
            "types.Union"
        } else {
            "types.UnionType"
        }
    })
}

pub(crate) fn type_name(_py: &PyToken<'_>, obj: MoltObject) -> Cow<'static, str> {
    if obj.is_int() {
        return Cow::Borrowed("int");
    }
    if obj.is_float() {
        return Cow::Borrowed("float");
    }
    if obj.is_bool() {
        return Cow::Borrowed("bool");
    }
    if obj.is_none() {
        return Cow::Borrowed("NoneType");
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            return match object_type_id(ptr) {
                TYPE_ID_STRING => Cow::Borrowed("str"),
                TYPE_ID_BYTES => Cow::Borrowed("bytes"),
                TYPE_ID_BYTEARRAY => Cow::Borrowed("bytearray"),
                TYPE_ID_LIST => Cow::Borrowed("list"),
                TYPE_ID_TUPLE => Cow::Borrowed("tuple"),
                TYPE_ID_DICT => Cow::Borrowed("dict"),
                TYPE_ID_DICT_KEYS_VIEW => Cow::Borrowed("dict_keys"),
                TYPE_ID_DICT_VALUES_VIEW => Cow::Borrowed("dict_values"),
                TYPE_ID_DICT_ITEMS_VIEW => Cow::Borrowed("dict_items"),
                TYPE_ID_SET => Cow::Borrowed("set"),
                TYPE_ID_FROZENSET => Cow::Borrowed("frozenset"),
                TYPE_ID_BIGINT => Cow::Borrowed("int"),
                TYPE_ID_COMPLEX => Cow::Borrowed("complex"),
                TYPE_ID_RANGE => Cow::Borrowed("range"),
                TYPE_ID_SLICE => Cow::Borrowed("slice"),
                TYPE_ID_MEMORYVIEW => Cow::Borrowed("memoryview"),
                TYPE_ID_INTARRAY => Cow::Borrowed("intarray"),
                TYPE_ID_NOT_IMPLEMENTED => Cow::Borrowed("NotImplementedType"),
                TYPE_ID_ELLIPSIS => Cow::Borrowed("ellipsis"),
                TYPE_ID_EXCEPTION => Cow::Borrowed("Exception"),
                TYPE_ID_DATACLASS => {
                    Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits())))
                }
                TYPE_ID_BUFFER2D => Cow::Borrowed("buffer2d"),
                TYPE_ID_CONTEXT_MANAGER => Cow::Borrowed("context_manager"),
                TYPE_ID_FILE_HANDLE => {
                    Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits())))
                }
                TYPE_ID_FUNCTION => Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits()))),
                TYPE_ID_BOUND_METHOD => Cow::Borrowed("method"),
                TYPE_ID_CODE => Cow::Borrowed("code"),
                TYPE_ID_MODULE => Cow::Borrowed("module"),
                TYPE_ID_TYPE => Cow::Borrowed("type"),
                TYPE_ID_GENERIC_ALIAS => Cow::Borrowed("types.GenericAlias"),
                TYPE_ID_UNION => Cow::Borrowed(union_type_display_name()),
                TYPE_ID_GENERATOR => Cow::Borrowed("generator"),
                TYPE_ID_ASYNC_GENERATOR => Cow::Borrowed("async_generator"),
                TYPE_ID_ENUMERATE => Cow::Borrowed("enumerate"),
                TYPE_ID_ITER => Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits()))),
                TYPE_ID_CALL_ITER => Cow::Borrowed("callable_iterator"),
                TYPE_ID_REVERSED => Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits()))),
                TYPE_ID_ZIP => Cow::Borrowed("zip"),
                TYPE_ID_MAP => Cow::Borrowed("map"),
                TYPE_ID_FILTER => Cow::Borrowed("filter"),
                TYPE_ID_CLASSMETHOD => Cow::Borrowed("classmethod"),
                TYPE_ID_STATICMETHOD => Cow::Borrowed("staticmethod"),
                TYPE_ID_PROPERTY => Cow::Borrowed("property"),
                TYPE_ID_SUPER => Cow::Borrowed("super"),
                TYPE_ID_OBJECT => Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits()))),
                _ => Cow::Borrowed("object"),
            };
        }
    }
    Cow::Borrowed("object")
}

pub(super) enum BinaryDunderOutcome {
    Value(u64),
    NotImplemented,
    Missing,
    Error,
}

pub(super) unsafe fn call_dunder_raw(
    _py: &PyToken<'_>,
    raw_bits: u64,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
    arg_bits: u64,
) -> BinaryDunderOutcome {
    unsafe {
        let Some(inst_ptr) = instance_ptr else {
            return BinaryDunderOutcome::Missing;
        };
        let Some(bound_bits) = descriptor_bind(_py, raw_bits, owner_ptr, Some(inst_ptr)) else {
            if exception_pending(_py) {
                return BinaryDunderOutcome::Error;
            }
            return BinaryDunderOutcome::Missing;
        };
        let res_bits = call_callable1(_py, bound_bits, arg_bits);
        dec_ref_bits(_py, bound_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, res_bits);
            return BinaryDunderOutcome::Error;
        }
        if is_not_implemented_bits(_py, res_bits) {
            dec_ref_bits(_py, res_bits);
            return BinaryDunderOutcome::NotImplemented;
        }
        BinaryDunderOutcome::Value(res_bits)
    }
}

pub(super) unsafe fn call_binary_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
    rop_name_bits: u64,
) -> Option<u64> {
    unsafe {
        let lhs_obj = obj_from_bits(lhs_bits);
        let rhs_obj = obj_from_bits(rhs_bits);
        let lhs_ptr = lhs_obj.as_ptr();
        let rhs_ptr = rhs_obj.as_ptr();

        let lhs_type_bits = type_of_bits(_py, lhs_bits);
        let rhs_type_bits = type_of_bits(_py, rhs_bits);
        let lhs_type_ptr = obj_from_bits(lhs_type_bits).as_ptr();
        let rhs_type_ptr = obj_from_bits(rhs_type_bits).as_ptr();

        let lhs_op_raw =
            lhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, op_name_bits));
        let rhs_rop_raw =
            rhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, rop_name_bits));

        let rhs_is_subclass =
            rhs_type_bits != lhs_type_bits && issubclass_bits(rhs_type_bits, lhs_type_bits);
        let prefer_rhs = rhs_is_subclass
            && rhs_rop_raw.is_some()
            && lhs_op_raw.is_none_or(|lhs_raw| lhs_raw != rhs_rop_raw.unwrap());

        let mut tried_rhs = false;
        if prefer_rhs
            && let (Some(rhs_ptr), Some(rhs_type_ptr), Some(rhs_raw)) =
                (rhs_ptr, rhs_type_ptr, rhs_rop_raw)
        {
            tried_rhs = true;
            match call_dunder_raw(_py, rhs_raw, rhs_type_ptr, Some(rhs_ptr), lhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }

        if let (Some(lhs_ptr), Some(lhs_type_ptr), Some(lhs_raw)) =
            (lhs_ptr, lhs_type_ptr, lhs_op_raw)
        {
            match call_dunder_raw(_py, lhs_raw, lhs_type_ptr, Some(lhs_ptr), rhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }

        if !tried_rhs
            && let (Some(rhs_ptr), Some(rhs_type_ptr), Some(rhs_raw)) =
                (rhs_ptr, rhs_type_ptr, rhs_rop_raw)
        {
            match call_dunder_raw(_py, rhs_raw, rhs_type_ptr, Some(rhs_ptr), lhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }
        None
    }
}

pub(super) unsafe fn call_inplace_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
) -> Option<u64> {
    unsafe {
        if let Some(lhs_ptr) = obj_from_bits(lhs_bits).as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr(_py, lhs_ptr, op_name_bits) {
                let res_bits = call_callable1(_py, call_bits, rhs_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return Some(MoltObject::none().bits());
                }
                if !is_not_implemented_bits(_py, res_bits) {
                    return Some(res_bits);
                }
                dec_ref_bits(_py, res_bits);
            }
            if exception_pending(_py) {
                return Some(MoltObject::none().bits());
            }
        }
        None
    }
}

pub(crate) fn obj_eq(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> bool {
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        return li == ri;
    }
    if lhs.is_none() && rhs.is_none() {
        return true;
    }
    if (lhs.is_float() || rhs.is_float())
        && let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs))
    {
        return lf == rf;
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        return l_big == r_big;
    }
    if complex_ptr_from_bits(lhs.bits()).is_some() || complex_ptr_from_bits(rhs.bits()).is_some() {
        let l_complex = complex_from_obj_lossy(lhs);
        let r_complex = complex_from_obj_lossy(rhs);
        if let (Some(lc), Some(rc)) = (l_complex, r_complex) {
            return lc.re == rc.re && lc.im == rc.im;
        }
        return false;
    }
    if let (Some(lp), Some(rp)) = (
        maybe_ptr_from_bits(lhs.bits()),
        maybe_ptr_from_bits(rhs.bits()),
    ) {
        unsafe {
            let ltype = object_type_id(lp);
            let rtype = object_type_id(rp);
            if ltype != rtype {
                if (ltype == TYPE_ID_BYTES && rtype == TYPE_ID_BYTEARRAY)
                    || (ltype == TYPE_ID_BYTEARRAY && rtype == TYPE_ID_BYTES)
                {
                    let l_len = bytes_len(lp);
                    let r_len = bytes_len(rp);
                    if l_len != r_len {
                        return false;
                    }
                    return simd_bytes_eq(bytes_data(lp), bytes_data(rp), l_len);
                }
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    let l_elems = set_order(lp);
                    let r_elems = set_order(rp);
                    if l_elems.len() != r_elems.len() {
                        return false;
                    }
                    let r_table = set_table(rp);
                    for key_bits in l_elems.iter().copied() {
                        if set_find_entry_fast(_py, r_elems, r_table, key_bits).is_none() {
                            return false;
                        }
                    }
                    return true;
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return false;
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return false;
                        };
                        (ptr, Some(bits))
                    };
                    let (rhs_ptr, rhs_bits) = if is_set_like_type(rtype) {
                        (rp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, rp, rtype) else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            return false;
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return false;
                        };
                        (ptr, Some(bits))
                    };
                    let l_elems = set_order(lhs_ptr);
                    let r_elems = set_order(rhs_ptr);
                    let mut equal = true;
                    if l_elems.len() != r_elems.len() {
                        equal = false;
                    } else {
                        let r_table = set_table(rhs_ptr);
                        for key_bits in l_elems.iter().copied() {
                            if set_find_entry_fast(_py, r_elems, r_table, key_bits).is_none() {
                                equal = false;
                                break;
                            }
                        }
                    }
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return equal;
                }
                return false;
            }
            if ltype == TYPE_ID_STRING {
                let l_len = string_len(lp);
                let r_len = string_len(rp);
                if l_len != r_len {
                    return false;
                }
                return simd_bytes_eq(string_bytes(lp), string_bytes(rp), l_len);
            }
            if ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                if l_len != r_len {
                    return false;
                }
                return simd_bytes_eq(bytes_data(lp), bytes_data(rp), l_len);
            }
            if ltype == TYPE_ID_TUPLE {
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                // SIMD fast path: skip past identity-equal prefix
                let first_diff = simd_find_first_mismatch(l_elems, r_elems);
                for idx in first_diff..l_elems.len() {
                    if !obj_eq(
                        _py,
                        obj_from_bits(l_elems[idx]),
                        obj_from_bits(r_elems[idx]),
                    ) {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_SLICE {
                let l_start = slice_start_bits(lp);
                let l_stop = slice_stop_bits(lp);
                let l_step = slice_step_bits(lp);
                let r_start = slice_start_bits(rp);
                let r_stop = slice_stop_bits(rp);
                let r_step = slice_step_bits(rp);
                if !obj_eq(_py, obj_from_bits(l_start), obj_from_bits(r_start)) {
                    return false;
                }
                if !obj_eq(_py, obj_from_bits(l_stop), obj_from_bits(r_stop)) {
                    return false;
                }
                if !obj_eq(_py, obj_from_bits(l_step), obj_from_bits(r_step)) {
                    return false;
                }
                return true;
            }
            if ltype == TYPE_ID_GENERIC_ALIAS {
                let l_origin = generic_alias_origin_bits(lp);
                let l_args = generic_alias_args_bits(lp);
                let r_origin = generic_alias_origin_bits(rp);
                let r_args = generic_alias_args_bits(rp);
                return obj_eq(_py, obj_from_bits(l_origin), obj_from_bits(r_origin))
                    && obj_eq(_py, obj_from_bits(l_args), obj_from_bits(r_args));
            }
            if ltype == TYPE_ID_UNION {
                let l_args = union_type_args_bits(lp);
                let r_args = union_type_args_bits(rp);
                return obj_eq(_py, obj_from_bits(l_args), obj_from_bits(r_args));
            }
            // Identity check: if pointers are equal, the objects are equal
            // (handles self-referential containers without infinite recursion).
            if lp == rp {
                return true;
            }
            if ltype == TYPE_ID_LIST {
                // Recursion guard for nested/self-referential containers
                if !crate::state::recursion::recursion_guard_enter_fast() {
                    raise_exception::<u64>(
                        _py,
                        "RecursionError",
                        "maximum recursion depth exceeded in comparison",
                    );
                    return false;
                }
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    crate::state::recursion::recursion_guard_exit_fast();
                    return false;
                }
                // SIMD fast path: skip past identity-equal prefix
                let first_diff = simd_find_first_mismatch(l_elems, r_elems);
                for idx in first_diff..l_elems.len() {
                    if !obj_eq(
                        _py,
                        obj_from_bits(l_elems[idx]),
                        obj_from_bits(r_elems[idx]),
                    ) {
                        crate::state::recursion::recursion_guard_exit_fast();
                        return false;
                    }
                }
                crate::state::recursion::recursion_guard_exit_fast();
                return true;
            }
            if ltype == TYPE_ID_DICT {
                if !crate::state::recursion::recursion_guard_enter_fast() {
                    raise_exception::<u64>(
                        _py,
                        "RecursionError",
                        "maximum recursion depth exceeded in comparison",
                    );
                    return false;
                }
                let l_pairs = dict_order(lp);
                let r_pairs = dict_order(rp);
                if l_pairs.len() != r_pairs.len() {
                    crate::state::recursion::recursion_guard_exit_fast();
                    return false;
                }
                let r_table = dict_table(rp);
                let entries = l_pairs.len() / 2;
                for entry_idx in 0..entries {
                    let key_bits = l_pairs[entry_idx * 2];
                    let val_bits = l_pairs[entry_idx * 2 + 1];
                    let Some(r_entry_idx) = dict_find_entry_fast(_py, r_pairs, r_table, key_bits)
                    else {
                        crate::state::recursion::recursion_guard_exit_fast();
                        return false;
                    };
                    let r_val_bits = r_pairs[r_entry_idx * 2 + 1];
                    if !obj_eq(_py, obj_from_bits(val_bits), obj_from_bits(r_val_bits)) {
                        crate::state::recursion::recursion_guard_exit_fast();
                        return false;
                    }
                }
                crate::state::recursion::recursion_guard_exit_fast();
                return true;
            }
            if ltype == TYPE_ID_SET || ltype == TYPE_ID_FROZENSET {
                let l_elems = set_order(lp);
                let r_elems = set_order(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                let r_table = set_table(rp);
                for key_bits in l_elems.iter().copied() {
                    if set_find_entry_fast(_py, r_elems, r_table, key_bits).is_none() {
                        return false;
                    }
                }
                return true;
            }
            if ltype == TYPE_ID_DATACLASS {
                let l_desc = dataclass_desc_ptr(lp);
                let r_desc = dataclass_desc_ptr(rp);
                if l_desc.is_null() || r_desc.is_null() {
                    return false;
                }
                let l_desc = &*l_desc;
                let r_desc = &*r_desc;
                if !l_desc.eq || !r_desc.eq {
                    return lp == rp;
                }
                if l_desc.name != r_desc.name || l_desc.field_names != r_desc.field_names {
                    return false;
                }
                let l_vals = dataclass_fields_ref(lp);
                let r_vals = dataclass_fields_ref(rp);
                if l_vals.len() != r_vals.len() {
                    return false;
                }
                for (idx, (l_val, r_val)) in l_vals.iter().zip(r_vals.iter()).enumerate() {
                    let flag = l_desc.field_flags.get(idx).copied().unwrap_or(0x7);
                    if (flag & 0x2) == 0 {
                        continue;
                    }
                    if is_missing_bits(_py, *l_val) || is_missing_bits(_py, *r_val) {
                        return false;
                    }
                    if !obj_eq(_py, obj_from_bits(*l_val), obj_from_bits(*r_val)) {
                        return false;
                    }
                }
                return true;
            }
            // Function equality: two functions with the same code object
            // are equal (CPython parity: len == len is True).
            if ltype == TYPE_ID_FUNCTION {
                let l_code = function_code_bits(lp);
                let r_code = function_code_bits(rp);
                if l_code != 0 && l_code == r_code {
                    return true;
                }
            }
            // Range equality: range(a,b,c) == range(a,b,c) if start,stop,step match.
            if ltype == TYPE_ID_RANGE {
                return obj_eq(_py, obj_from_bits(range_start_bits(lp)), obj_from_bits(range_start_bits(rp)))
                    && obj_eq(_py, obj_from_bits(range_stop_bits(lp)), obj_from_bits(range_stop_bits(rp)))
                    && obj_eq(_py, obj_from_bits(range_step_bits(lp)), obj_from_bits(range_step_bits(rp)));
            }
        }
        return lp == rp;
    }
    false
}

pub(crate) fn dict_table_capacity(entries: usize) -> usize {
    let mut cap = entries.saturating_mul(2).next_power_of_two();
    if cap < 8 {
        cap = 8;
    }
    cap
}

const TABLE_TOMBSTONE: usize = usize::MAX;

fn dict_insert_entry(_py: &PyToken<'_>, order: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let key_bits = order[entry_idx * 2];
    // Fast path: inline int keys use hash_int directly, avoiding the
    // full hash_bits dispatch through exception-checking code paths.
    let key_obj = obj_from_bits(key_bits);
    let hash = if let Some(i) = key_obj.as_int() {
        hash_int(i) as u64
    } else {
        hash_bits(_py, key_bits)
    };
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn dict_insert_entry_with_hash(
    _py: &PyToken<'_>,
    _order: &[u64],
    table: &mut [usize],
    entry_idx: usize,
    hash: u64,
) {
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}
pub(crate) fn dict_rebuild(
    _py: &PyToken<'_>,
    order: &[u64],
    table: &mut Vec<usize>,
    capacity: usize,
) {
    table.clear();
    table.resize(capacity, 0);
    let entry_count = order.len() / 2;
    for entry_idx in 0..entry_count {
        dict_insert_entry(_py, order, table, entry_idx);
    }
}

pub(crate) fn dict_find_entry_fast(
    _py: &PyToken<'_>,
    order: &[u64],
    table: &[usize],
    key_bits: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash_bits(_py, key_bits) as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        if entry_idx * 2 >= order.len() {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx * 2];
        // Fast path: identical bit patterns are always equal.
        if entry_key == key_bits || obj_eq(_py, obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn dict_find_entry(
    _py: &PyToken<'_>,
    order: &[u64],
    table: &[usize],
    key_bits: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let pending_before = exception_pending(_py);
    let mask = table.len() - 1;
    let mut slot = (hash_bits(_py, key_bits) as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        // Safety: corrupted hash tables can have huge entry values from
        // use-after-free. Bounds-check before indexing to turn a crash
        // into a graceful "not found".
        if entry_idx * 2 >= order.len() {
            // Corrupted entry — skip it like a tombstone.
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx * 2];
        // Fast path: identical bit patterns are always equal.
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        if let Some(eq) = unsafe { string_bits_eq(entry_key, key_bits) } {
            if eq {
                return Some(entry_idx);
            }
            slot = (slot + 1) & mask;
            continue;
        }
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {
                if pending_before && unsafe { string_bits_eq(entry_key, key_bits) } == Some(true) {
                    return Some(entry_idx);
                }
            }
            None => {
                if pending_before && unsafe { string_bits_eq(entry_key, key_bits) } == Some(true) {
                    return Some(entry_idx);
                }
                return None;
            }
        }
        slot = (slot + 1) & mask;
    }
}

// ---------------------------------------------------------------------------
// SIMD-accelerated byte-level equality for string/bytes comparisons.
// For short strings (< 32 bytes), the compiler-generated memcmp is fast enough.
// For longer strings, explicit SIMD provides measurable wins especially on
// Apple Silicon where NEON is always available with no runtime detection cost.
// ---------------------------------------------------------------------------

/// SIMD byte equality: returns true if `a[..len] == b[..len]`.
/// Precondition: both pointers are valid for `len` bytes.
#[inline(always)]
pub(super) unsafe fn simd_bytes_eq(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        // Tiny strings (<=8 bytes): direct comparison, no SIMD overhead.
        if len <= 8 {
            if len == 0 {
                return true;
            }
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }

        // Short strings (9-15 bytes): compare overlapping 8-byte windows.
        // This covers the full range without underflowing the tail pointer.
        if len < 16 {
            return simd_bytes_eq_short_u64(a, b, len);
        }

        // Short strings (16-31 bytes): use NEON/SSE2 16-byte loads instead of
        // scalar memcmp. Two overlapping 16-byte loads cover any length in
        // 16..31 without a loop, which is measurably faster for dict-key
        // comparisons where keys are typically short identifiers (< 32 bytes).
        #[cfg(target_arch = "aarch64")]
        if len < 32 {
            return simd_bytes_eq_short_neon(a, b, len);
        }
        #[cfg(target_arch = "x86_64")]
        if len < 32 {
            if std::arch::is_x86_feature_detected!("sse2") {
                return simd_bytes_eq_short_sse2(a, b, len);
            }
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        if len < 32 {
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }

        // Long strings (>= 32 bytes): full SIMD loops.
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return simd_bytes_eq_avx2(a, b, len);
            }
            return simd_bytes_eq_sse2(a, b, len);
        }
        #[cfg(target_arch = "aarch64")]
        {
            return simd_bytes_eq_neon(a, b, len);
        }
        #[cfg(target_arch = "wasm32")]
        {
            return simd_bytes_eq_wasm32(a, b, len);
        }
        #[allow(unreachable_code)]
        {
            std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len)
        }
    }
}

/// Short-string equality for 9-15 bytes: overlapping unaligned 8-byte loads.
#[inline(always)]
unsafe fn simd_bytes_eq_short_u64(a: *const u8, b: *const u8, len: usize) -> bool {
    debug_assert!(len >= 9 && len < 16);
    unsafe {
        let head_a = std::ptr::read_unaligned(a as *const u64);
        let head_b = std::ptr::read_unaligned(b as *const u64);
        if head_a != head_b {
            return false;
        }
        let tail_a = std::ptr::read_unaligned(a.add(len - 8) as *const u64);
        let tail_b = std::ptr::read_unaligned(b.add(len - 8) as *const u64);
        tail_a == tail_b
    }
}

/// NEON short-string equality for 16-31 bytes: two overlapping 16-byte loads.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn simd_bytes_eq_short_neon(a: *const u8, b: *const u8, len: usize) -> bool {
    use std::arch::aarch64::*;
    debug_assert!(len >= 16 && len < 32);
    unsafe {
        // Load from the start
        let va0 = vld1q_u8(a);
        let vb0 = vld1q_u8(b);
        let cmp0 = vceqq_u8(va0, vb0);
        // Load from (end - 16), overlapping with the first load for short strings
        let va1 = vld1q_u8(a.add(len - 16));
        let vb1 = vld1q_u8(b.add(len - 16));
        let cmp1 = vceqq_u8(va1, vb1);
        // Both loads must match: AND the comparison results and check all-0xFF
        let combined = vandq_u8(cmp0, cmp1);
        vminvq_u8(combined) == 0xFF
    }
}

/// SSE2 short-string equality for 16-31 bytes: two overlapping 16-byte loads.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn simd_bytes_eq_short_sse2(a: *const u8, b: *const u8, len: usize) -> bool {
    use std::arch::x86_64::*;
    debug_assert!(len >= 16 && len < 32);
    // Load from the start
    let va0 = _mm_loadu_si128(a as *const __m128i);
    let vb0 = _mm_loadu_si128(b as *const __m128i);
    let cmp0 = _mm_cmpeq_epi8(va0, vb0);
    // Load from (end - 16), overlapping with the first load
    let va1 = _mm_loadu_si128(a.add(len - 16) as *const __m128i);
    let vb1 = _mm_loadu_si128(b.add(len - 16) as *const __m128i);
    let cmp1 = _mm_cmpeq_epi8(va1, vb1);
    // Both must be all-equal: AND the masks
    let mask0 = _mm_movemask_epi8(cmp0);
    let mask1 = _mm_movemask_epi8(cmp1);
    (mask0 & mask1) == 0xFFFF
}

// ---------------------------------------------------------------------------
// SIMD-accelerated u64 linear scan for list/tuple `in` operator.
// For NaN-boxed integers, bools, and None, bit-equality implies value equality,
// so we can scan the raw u64 element slice without calling eq_bool_from_bits.
// On aarch64 (NEON) and x86_64 (SSE2/AVX2), we broadcast the needle into a
// SIMD register and compare 2-4 elements per cycle.
// ---------------------------------------------------------------------------

/// Returns true if `needle` appears in `haystack` by raw u64 identity.
/// Only valid when the needle is a NaN-boxed int, bool, or None (where
/// bit-equality implies Python value equality).
#[inline(always)]
fn simd_contains_u64(haystack: &[u64], needle: u64) -> bool {
    let len = haystack.len();
    if len == 0 {
        return false;
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { simd_contains_u64_neon(haystack, needle) };
    }

    #[cfg(target_arch = "x86_64")]
    {
        if len >= 4 && std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { simd_contains_u64_avx2(haystack, needle) };
        }
        if len >= 2 && std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { simd_contains_u64_sse2(haystack, needle) };
        }
    }

    // Scalar fallback (also covers wasm32 and other targets).
    #[allow(unreachable_code)]
    {
        for &elem in haystack {
            if elem == needle {
                return true;
            }
        }
        false
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn simd_contains_u64_neon(haystack: &[u64], needle: u64) -> bool {
    unsafe {
        use std::arch::aarch64::*;
        let ptr = haystack.as_ptr();
        let len = haystack.len();
        let needle_vec = vdupq_n_u64(needle);
        let mut i = 0usize;
        // Process 2 u64s at a time (128-bit NEON register = 2 x u64).
        while i + 2 <= len {
            let chunk = vld1q_u64(ptr.add(i));
            let cmp = vceqq_u64(chunk, needle_vec);
            // vmaxvq_u64 is not available; use vgetq_lane to check both lanes.
            if vgetq_lane_u64(cmp, 0) != 0 || vgetq_lane_u64(cmp, 1) != 0 {
                return true;
            }
            i += 2;
        }
        // Tail element
        if i < len && *ptr.add(i) == needle {
            return true;
        }
        false
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn simd_contains_u64_sse2(haystack: &[u64], needle: u64) -> bool {
    use std::arch::x86_64::*;
    let ptr = haystack.as_ptr() as *const __m128i;
    let len = haystack.len();
    let needle_vec = _mm_set1_epi64x(needle as i64);
    let mut i = 0usize;
    // Process 2 u64s at a time (128-bit register = 2 x u64).
    while i + 2 <= len {
        let chunk = _mm_loadu_si128(ptr.add(i / 2));
        let cmp = _mm_cmpeq_epi32(chunk, needle_vec);
        // For 64-bit equality, both 32-bit halves must match.
        // Shuffle to align adjacent 32-bit results and AND them.
        let shuffled = _mm_shuffle_epi32(cmp, 0b10_11_00_01);
        let both = _mm_and_si128(cmp, shuffled);
        if _mm_movemask_epi8(both) != 0 {
            return true;
        }
        i += 2;
    }
    if i < len && haystack[i] == needle {
        return true;
    }
    false
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn simd_contains_u64_avx2(haystack: &[u64], needle: u64) -> bool {
    use std::arch::x86_64::*;
    let ptr = haystack.as_ptr() as *const __m256i;
    let len = haystack.len();
    let needle_vec = _mm256_set1_epi64x(needle as i64);
    let mut i = 0usize;
    // Process 4 u64s at a time (256-bit register = 4 x u64).
    while i + 4 <= len {
        let chunk = _mm256_loadu_si256(ptr.add(i / 4));
        let cmp = _mm256_cmpeq_epi64(chunk, needle_vec);
        if _mm256_movemask_epi8(cmp) != 0 {
            return true;
        }
        i += 4;
    }
    // Tail: scalar check for remaining 0-3 elements.
    while i < len {
        if haystack[i] == needle {
            return true;
        }
        i += 1;
    }
    false
}

#[cfg(target_arch = "wasm32")]
#[inline]
unsafe fn simd_bytes_eq_wasm32(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = v128_load(a.add(i) as *const v128);
            let vb = v128_load(b.add(i) as *const v128);
            let cmp = u8x16_eq(va, vb);
            if u8x16_bitmask(cmp) != 0xFFFF {
                return false;
            }
            i += 16;
        }
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_bytes_eq_sse2(a: *const u8, b: *const u8, len: usize) -> bool {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    while i + 16 <= len {
        let va = _mm_loadu_si128(a.add(i) as *const __m128i);
        let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(va, vb);
        if _mm_movemask_epi8(cmp) != 0xFFFF {
            return false;
        }
        i += 16;
    }
    // Tail: compare remaining bytes
    std::slice::from_raw_parts(a.add(i), len - i) == std::slice::from_raw_parts(b.add(i), len - i)
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_bytes_eq_avx2(a: *const u8, b: *const u8, len: usize) -> bool {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    while i + 32 <= len {
        let va = _mm256_loadu_si256(a.add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.add(i) as *const __m256i);
        let cmp = _mm256_cmpeq_epi8(va, vb);
        if _mm256_movemask_epi8(cmp) != -1i32 {
            return false;
        }
        i += 32;
    }
    // SSE2 tail for 16-byte remainder
    if i + 16 <= len {
        let va = _mm_loadu_si128(a.add(i) as *const __m128i);
        let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(va, vb);
        if _mm_movemask_epi8(cmp) != 0xFFFF {
            return false;
        }
        i += 16;
    }
    std::slice::from_raw_parts(a.add(i), len - i) == std::slice::from_raw_parts(b.add(i), len - i)
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn simd_bytes_eq_neon(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = vld1q_u8(a.add(i));
            let vb = vld1q_u8(b.add(i));
            let cmp = vceqq_u8(va, vb);
            // vminvq_u8 returns 0xFF if all lanes equal, < 0xFF if any differ
            if vminvq_u8(cmp) != 0xFF {
                return false;
            }
            i += 16;
        }
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

unsafe fn string_bits_eq(a_bits: u64, b_bits: u64) -> Option<bool> {
    unsafe {
        let a_obj = obj_from_bits(a_bits);
        let b_obj = obj_from_bits(b_bits);
        let a_ptr = a_obj.as_ptr()?;
        let b_ptr = b_obj.as_ptr()?;
        if object_type_id(a_ptr) != TYPE_ID_STRING || object_type_id(b_ptr) != TYPE_ID_STRING {
            return None;
        }
        if a_ptr == b_ptr {
            return Some(true);
        }
        let a_len = string_len(a_ptr);
        let b_len = string_len(b_ptr);
        if a_len != b_len {
            return Some(false);
        }
        Some(simd_bytes_eq(
            string_bytes(a_ptr),
            string_bytes(b_ptr),
            a_len,
        ))
    }
}

pub(crate) fn dict_find_entry_with_hash(
    _py: &PyToken<'_>,
    order: &[u64],
    table: &[usize],
    key_bits: u64,
    hash: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        if entry_idx * 2 >= order.len() {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx * 2];
        // Fast path: identical bit patterns are always equal.
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        if let Some(eq) = unsafe { string_bits_eq(entry_key, key_bits) } {
            if eq {
                return Some(entry_idx);
            }
            slot = (slot + 1) & mask;
            continue;
        }
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {}
            None => return None,
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn set_table_capacity(entries: usize) -> usize {
    dict_table_capacity(entries)
}

fn set_insert_entry(_py: &PyToken<'_>, order: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let key_bits = order[entry_idx];
    let mut slot = (hash_bits(_py, key_bits) as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}

fn set_insert_entry_with_hash(
    _py: &PyToken<'_>,
    _order: &[u64],
    table: &mut [usize],
    entry_idx: usize,
    hash: u64,
) {
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}
pub(super) fn set_rebuild(
    _py: &PyToken<'_>,
    order: &[u64],
    table: &mut Vec<usize>,
    capacity: usize,
) {
    crate::gil_assert();
    table.clear();
    table.resize(capacity, 0);
    for entry_idx in 0..order.len() {
        set_insert_entry(_py, order, table, entry_idx);
    }
}

pub(crate) fn set_find_entry_fast(
    _py: &PyToken<'_>,
    order: &[u64],
    table: &[usize],
    key_bits: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash_bits(_py, key_bits) as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        let entry_key = order[entry_idx];
        // Identity check first (CPython semantics: `x is y or x == y`).
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        if obj_eq(_py, obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn set_find_entry(
    _py: &PyToken<'_>,
    order: &[u64],
    table: &[usize],
    key_bits: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash_bits(_py, key_bits) as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        let entry_key = order[entry_idx];
        // Identity check first (CPython semantics: `x is y or x == y`).
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {}
            None => return None,
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn set_find_entry_with_hash(
    _py: &PyToken<'_>,
    order: &[u64],
    table: &[usize],
    key_bits: u64,
    hash: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        let entry_key = order[entry_idx];
        // Identity check first (CPython semantics: `x is y or x == y`).
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {}
            None => return None,
        }
        slot = (slot + 1) & mask;
    }
}

pub(super) fn concat_bytes_like(
    _py: &PyToken<'_>,
    left: &[u8],
    right: &[u8],
    type_id: u32,
) -> Option<u64> {
    let total = left.len().checked_add(right.len())?;
    if type_id == TYPE_ID_BYTEARRAY {
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(left);
        out.extend_from_slice(right);
        let ptr = alloc_bytearray(_py, &out);
        if ptr.is_null() {
            return None;
        }
        return Some(MoltObject::from_ptr(ptr).bits());
    }
    let ptr = alloc_bytes_like_with_len(_py, total, type_id);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(left.as_ptr(), data_ptr, left.len());
        std::ptr::copy_nonoverlapping(right.as_ptr(), data_ptr.add(left.len()), right.len());
    }
    Some(MoltObject::from_ptr(ptr).bits())
}

pub(super) fn fill_repeated_bytes(dst: &mut [u8], pattern: &[u8]) {
    if pattern.is_empty() {
        return;
    }
    if pattern.len() == 1 {
        dst.fill(pattern[0]);
        return;
    }
    let mut filled = pattern.len().min(dst.len());
    dst[..filled].copy_from_slice(&pattern[..filled]);
    while filled < dst.len() {
        let copy_len = std::cmp::min(filled, dst.len() - filled);
        let (head, tail) = dst.split_at_mut(filled);
        tail[..copy_len].copy_from_slice(&head[..copy_len]);
        filled += copy_len;
    }
}

pub(crate) unsafe fn dict_set_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) {
    unsafe {
        crate::gil_assert();
        // Fast path: inline NaN-boxed ints bypass all exception checks,
        // hashability validation, and refcounting overhead.
        let key_obj = obj_from_bits(key_bits);
        if let Some(i) = key_obj.as_int() {
            return dict_set_inline_int_in_place(_py, ptr, key_bits, i, val_bits);
        }
        let hash = if key_obj.as_ptr().is_none() {
            // Bool, None, or other inline -- still always hashable, use
            // the normal hash path but skip ensure_hashable.
            hash_bits(_py, key_bits)
        } else {
            // Heap-allocated key: need full hashability check.
            if !ensure_hashable(_py, key_bits) {
                return;
            }
            hash_bits(_py, key_bits)
        };
        if exception_pending(_py) {
            return;
        }
        let order = dict_order(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry_with_hash(_py, order, table, key_bits, hash);
        if exception_pending(_py) {
            return;
        }
        if let Some(entry_idx) = found {
            let val_idx = entry_idx * 2 + 1;
            let old_bits = order[val_idx];
            if old_bits != val_bits {
                dec_ref_bits(_py, old_bits);
                inc_ref_bits(_py, val_bits);
                order[val_idx] = val_bits;
                if crate::object::refcount_opt::is_heap_ref(val_bits) {
                    (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
                }
            }
            return;
        }

        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                return;
            }
        }

        order.push(key_bits);
        order.push(val_bits);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, val_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits)
            || crate::object::refcount_opt::is_heap_ref(val_bits)
        {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
}

/// Ultra-fast dict set for inline NaN-boxed integer keys AND values.
/// Skips: ensure_hashable (ints always hashable), exception_pending checks
/// (hash_int + bit-equality cannot raise), and inc_ref/dec_ref (inline
/// values have no heap allocation).
#[inline]
pub(crate) unsafe fn dict_set_inline_int_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    key_int: i64,
    val_bits: u64,
) {
    unsafe {
        let hash = hash_int(key_int) as u64;
        let order = dict_order(ptr);
        let table = dict_table(ptr);

        // Inline find: for integer keys, bit-equality is sufficient.
        if !table.is_empty() {
            let mask = table.len() - 1;
            let mut slot = (hash as usize) & mask;
            loop {
                let entry = table[slot];
                if entry == 0 {
                    break;
                }
                if entry != TABLE_TOMBSTONE {
                    let entry_idx = entry - 1;
                    if order[entry_idx * 2] == key_bits {
                        // Key exists -- update value in place.
                        let val_idx = entry_idx * 2 + 1;
                        let old_bits = order[val_idx];
                        if old_bits != val_bits {
                            let old_obj = obj_from_bits(old_bits);
                            let new_obj = obj_from_bits(val_bits);
                            if old_obj.as_ptr().is_some() {
                                dec_ref_bits(_py, old_bits);
                            }
                            if new_obj.as_ptr().is_some() {
                                inc_ref_bits(_py, val_bits);
                                (*header_from_obj_ptr(ptr)).flags |=
                                    crate::object::HEADER_FLAG_CONTAINS_REFS;
                            }
                            order[val_idx] = val_bits;
                        }
                        return;
                    }
                }
                slot = (slot + 1) & mask;
            }
        }

        // Key not found: insert.
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, table, capacity);
        }

        order.push(key_bits);
        order.push(val_bits);
        // key is inline int: no refcount needed.
        // value: only inc_ref if heap-allocated.
        let val_obj = obj_from_bits(val_bits);
        if val_obj.as_ptr().is_some() {
            inc_ref_bits(_py, val_bits);
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
    }
}

/// Ultra-fast dict get for inline NaN-boxed integer keys.
/// Skips: ensure_hashable, exception state save/restore, and the
/// string_bits_eq / eq_bool_from_bits fallback paths.
#[inline]
pub(crate) unsafe fn dict_get_inline_int_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    key_int: i64,
) -> Option<u64> {
    unsafe {
        let hash = hash_int(key_int) as u64;
        let order = dict_order(ptr);
        let table = dict_table(ptr);
        if table.is_empty() {
            return None;
        }
        let mask = table.len() - 1;
        let mut slot = (hash as usize) & mask;
        loop {
            let entry = table[slot];
            if entry == 0 {
                return None;
            }
            if entry != TABLE_TOMBSTONE {
                let entry_idx = entry - 1;
                if order[entry_idx * 2] == key_bits {
                    return Some(order[entry_idx * 2 + 1]);
                }
            }
            slot = (slot + 1) & mask;
        }
    }
}

#[allow(dead_code)]
pub(crate) unsafe fn dict_set_in_place_preserving_pending(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) {
    unsafe {
        crate::gil_assert();
        if !ensure_hashable(_py, key_bits) {
            return;
        }
        let pending_before = exception_pending(_py);
        let prev_exc_bits = if pending_before {
            exception_last_bits_noinc(_py).unwrap_or(0)
        } else {
            0
        };
        let hash = hash_bits(_py, key_bits);
        if exception_pending(_py) {
            if !pending_before {
                return;
            }
            let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
            if after_exc_bits != prev_exc_bits {
                return;
            }
        }
        let order = dict_order(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry_with_hash(_py, order, table, key_bits, hash);
        if exception_pending(_py) {
            if !pending_before {
                return;
            }
            let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
            if after_exc_bits != prev_exc_bits {
                return;
            }
        }
        if let Some(entry_idx) = found {
            let val_idx = entry_idx * 2 + 1;
            let old_bits = order[val_idx];
            if old_bits != val_bits {
                dec_ref_bits(_py, old_bits);
                inc_ref_bits(_py, val_bits);
                order[val_idx] = val_bits;
                if crate::object::refcount_opt::is_heap_ref(val_bits) {
                    (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
                }
            }
            return;
        }

        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                if !pending_before {
                    return;
                }
                let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
                if after_exc_bits != prev_exc_bits {
                    return;
                }
            }
        }

        order.push(key_bits);
        order.push(val_bits);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, val_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits)
            || crate::object::refcount_opt::is_heap_ref(val_bits)
        {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
}

pub(crate) unsafe fn set_add_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) {
    unsafe {
        crate::gil_assert();
        if !ensure_hashable(_py, key_bits) {
            return;
        }
        let hash = hash_bits(_py, key_bits);
        if exception_pending(_py) {
            return;
        }
        let order = set_order(ptr);
        let table = set_table(ptr);
        let found = set_find_entry_with_hash(_py, order, table, key_bits, hash);
        if exception_pending(_py) {
            return;
        }
        if found.is_some() {
            return;
        }

        let new_entries = order.len() + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = set_table_capacity(new_entries);
            set_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                return;
            }
        }

        order.push(key_bits);
        inc_ref_bits(_py, key_bits);
        let entry_idx = order.len() - 1;
        set_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits) {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
}

pub(crate) unsafe fn dict_get_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
) -> Option<u64> {
    unsafe {
        // Fast path for inline integer keys: skip all exception handling,
        // hashability checks, and the heavy dict_find_entry dispatch.
        let key_obj = obj_from_bits(key_bits);
        if let Some(i) = key_obj.as_int() {
            return dict_get_inline_int_in_place(_py, ptr, key_bits, i);
        }
        // Pre-materialize the key to force NaN-box pointer resolution and
        // hash caching. This prevents Cranelift-compiled code from producing
        // stale or incorrect hash values during dict_find_entry.
        if let Some(key_ptr) = key_obj.as_ptr()
            && object_type_id(key_ptr) == TYPE_ID_STRING
        {
            let len = string_len(key_ptr);
            if len > 0 {
                std::ptr::read_volatile(string_bytes(key_ptr));
            }
        }
        if !ensure_hashable(_py, key_bits) {
            return None;
        }
        let pending_before = exception_pending(_py);
        let prev_exc_bits = if pending_before {
            exception_last_bits_noinc(_py).unwrap_or(0)
        } else {
            0
        };
        let order = dict_order(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry(_py, order, table, key_bits);
        if exception_pending(_py) {
            if !pending_before {
                return None;
            }
            let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
            if after_exc_bits != prev_exc_bits {
                return None;
            }
        }
        found.map(|idx| order[idx * 2 + 1])
    }
}

pub(crate) unsafe fn dict_find_entry_kv_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
) -> Option<(u64, u64)> {
    unsafe {
        if !ensure_hashable(_py, key_bits) {
            return None;
        }
        let pending_before = exception_pending(_py);
        let prev_exc_bits = if pending_before {
            exception_last_bits_noinc(_py).unwrap_or(0)
        } else {
            0
        };
        let order = dict_order(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry(_py, order, table, key_bits);
        if exception_pending(_py) {
            if !pending_before {
                return None;
            }
            let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
            if after_exc_bits != prev_exc_bits {
                return None;
            }
        }
        let idx = found?;
        let key_idx = idx * 2;
        Some((order[key_idx], order[key_idx + 1]))
    }
}

pub(crate) unsafe fn set_del_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) -> bool {
    unsafe {
        if !ensure_hashable(_py, key_bits) {
            return false;
        }
        let order = set_order(ptr);
        let table = set_table(ptr);
        let found = set_find_entry(_py, order, table, key_bits);
        if exception_pending(_py) {
            return false;
        }
        let Some(entry_idx) = found else {
            return false;
        };
        let key_val = order[entry_idx];
        dec_ref_bits(_py, key_val);
        order.remove(entry_idx);
        let removed_slot_val = entry_idx + 1;
        let mut tombstones = 0usize;
        for slot in table.iter_mut() {
            if *slot == 0 {
                continue;
            }
            if *slot == TABLE_TOMBSTONE {
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot == removed_slot_val {
                *slot = TABLE_TOMBSTONE;
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot > removed_slot_val {
                *slot -= 1;
            }
        }
        let entries = order.len();
        let desired_capacity = set_table_capacity(entries.max(1));
        if table.len() > desired_capacity.saturating_mul(4)
            || tombstones.saturating_mul(4) > table.len()
        {
            set_rebuild(_py, order, table, desired_capacity);
        }
        true
    }
}

pub(crate) unsafe fn set_replace_entries(_py: &PyToken<'_>, ptr: *mut u8, entries: &[u64]) {
    unsafe {
        crate::gil_assert();
        let order = set_order(ptr);
        for entry in entries {
            inc_ref_bits(_py, *entry);
        }
        for entry in order.iter().copied() {
            dec_ref_bits(_py, entry);
        }
        order.clear();
        order.extend_from_slice(entries);
        let table = set_table(ptr);
        let capacity = set_table_capacity(order.len().max(1));
        set_rebuild(_py, order, table, capacity);
    }
}

pub(crate) unsafe fn dict_del_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) -> bool {
    unsafe {
        if !ensure_hashable(_py, key_bits) {
            return false;
        }
        let order = dict_order(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry(_py, order, table, key_bits);
        if exception_pending(_py) {
            return false;
        }
        let Some(entry_idx) = found else {
            return false;
        };
        let key_idx = entry_idx * 2;
        let val_idx = key_idx + 1;
        let key_val = order[key_idx];
        let val_val = order[val_idx];
        dec_ref_bits(_py, key_val);
        dec_ref_bits(_py, val_val);
        order.drain(key_idx..=val_idx);
        let removed_slot_val = entry_idx + 1;
        let mut tombstones = 0usize;
        for slot in table.iter_mut() {
            if *slot == 0 {
                continue;
            }
            if *slot == TABLE_TOMBSTONE {
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot == removed_slot_val {
                *slot = TABLE_TOMBSTONE;
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot > removed_slot_val {
                *slot -= 1;
            }
        }
        let entries = order.len() / 2;
        let desired_capacity = dict_table_capacity(entries.max(1));
        if table.len() > desired_capacity.saturating_mul(4)
            || tombstones.saturating_mul(4) > table.len()
        {
            dict_rebuild(_py, order, table, desired_capacity);
        }
        true
    }
}

pub(crate) unsafe fn dict_clear_in_place(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        let order = dict_order(ptr);
        for pair in order.chunks_exact(2) {
            dec_ref_bits(_py, pair[0]);
            dec_ref_bits(_py, pair[1]);
        }
        order.clear();
        let table = dict_table(ptr);
        table.clear();
    }
}

/// Outlined class definition helper.  Replaces the multi-op inline sequence
/// (`class_new` + `class_set_base` + N x `set_attr_generic_obj` +
/// `class_apply_set_name` + `__init_subclass__` dispatch +
/// `class_set_layout_version`) with a single runtime call.
#[unsafe(no_mangle)]
pub extern "C" fn molt_guarded_class_def(
    name_bits: u64,
    bases_ptr: *const u64,
    nbases: u64,
    attrs_ptr: *const u64,
    nattrs: u64,
    layout_size: i64,
    layout_version: i64,
    flags: u64,
) -> u64 {
    use crate::builtins::attributes::molt_set_attr_name;
    use crate::builtins::types::{
        molt_class_apply_set_name, molt_class_new, molt_class_set_base,
        molt_class_set_layout_version,
    };
    use molt_obj_model::MoltObject;

    let none = MoltObject::none().bits();
    let class_bits = molt_class_new(name_bits);
    if class_bits == none {
        return class_bits;
    }

    let nb = nbases as usize;
    let bases_vec = if nb > 0 {
        unsafe { std::slice::from_raw_parts(bases_ptr, nb).to_vec() }
    } else {
        Vec::new()
    };
    if nb > 0 {
        if nb == 1 {
            molt_class_set_base(class_bits, bases_vec[0]);
        } else {
            crate::with_gil_entry!(_py, {
                let tuple_ptr = crate::object::builders::alloc_tuple(_py, &bases_vec);
                if !tuple_ptr.is_null() {
                    let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    molt_class_set_base(class_bits, tuple_bits);
                    crate::dec_ref_bits(_py, tuple_bits);
                }
            });
        }
    }

    let na = nattrs as usize;
    let attrs_vec = if na > 0 {
        unsafe { std::slice::from_raw_parts(attrs_ptr, na * 2).to_vec() }
    } else {
        Vec::new()
    };
    if na > 0 {
        for pair in attrs_vec.chunks_exact(2) {
            molt_set_attr_name(class_bits, pair[0], pair[1]);
        }
    }

    crate::with_gil_entry!(_py, {
        let size_obj = MoltObject::from_int(layout_size).bits();
        let layout_attr = crate::intern_static_name(
            _py,
            &crate::runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        molt_set_attr_name(class_bits, layout_attr, size_obj);
        crate::dec_ref_bits(_py, size_obj);
    });

    molt_class_apply_set_name(class_bits);

    if (flags & 1) != 0 && nb > 0 {
        crate::with_gil_entry!(_py, {
            let init_name = crate::intern_static_name(
                _py,
                &crate::runtime_state(_py).interned.init_subclass_name,
                b"__init_subclass__",
            );
            for &base in &bases_vec {
                // Snapshot `bases_vec`/`attrs_vec` before any nested call:
                // linked wasm class lowering passes pointers into a shared
                // scratch region, and reentrant compiled calls can overwrite
                // that storage if we keep borrowing it directly.
                let base_obj = obj_from_bits(base);
                let Some(base_ptr) = base_obj.as_ptr() else {
                    continue;
                };
                if unsafe { object_type_id(base_ptr) } != TYPE_ID_TYPE {
                    continue;
                }
                let init_attr =
                    crate::builtins::attributes::molt_get_attr_name_default(base, init_name, none);
                if init_attr != none {
                    if unsafe { crate::call::type_policy::callable_function_addr(Some(init_attr)) }
                        == Some(fn_addr!(crate::molt_object_init_subclass))
                    {
                        crate::dec_ref_bits(_py, init_attr);
                        continue;
                    }
                    let init_obj = obj_from_bits(init_attr);
                    let needs_kwargs = unsafe {
                        match init_obj.as_ptr() {
                            Some(ptr) if object_type_id(ptr) == TYPE_ID_FUNCTION => {
                                function_arity(ptr) > 1
                            }
                            _ => false,
                        }
                    };
                    if needs_kwargs {
                        let empty_dict = crate::builtins::containers_alloc::molt_dict_new(0);
                        let _ = unsafe {
                            crate::call::dispatch::call_callable2(
                                _py, init_attr, class_bits, empty_dict,
                            )
                        };
                        crate::dec_ref_bits(_py, empty_dict);
                    } else {
                        let _ = unsafe {
                            crate::call::dispatch::call_callable1(_py, init_attr, class_bits)
                        };
                    }
                    crate::dec_ref_bits(_py, init_attr);
                }
            }
            // init_name is globally interned — do NOT dec_ref it.
        });
    }

    let version_obj = MoltObject::from_int(layout_version).bits();
    molt_class_set_layout_version(class_bits, version_obj);
    crate::with_gil_entry!(_py, {
        crate::dec_ref_bits(_py, version_obj);
    });

    class_bits
}

/// Build an f-string from interleaved literal and value parts in a single call.
///
/// The parts array contains `(is_literal, value)` pairs as consecutive u64s:
/// - If `is_literal` is truthy: `value` is already a string (use directly)
/// - If `is_literal` is falsy: `value` needs conversion via `str()`
///
/// This consolidates the multi-op f-string assembly (N const_str + N string_format
/// + tuple_new + string_join) into a single runtime call.
#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn molt_fstring_build(parts_ptr: *const u64, n_parts: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n = n_parts as usize;
        if n == 0 {
            let ptr = alloc_string(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        }

        // Collect string parts — resolve values via str() as needed.
        let mut parts: Vec<(u64, bool)> = Vec::with_capacity(n); // (string_bits, owned)
        let mut total_len: usize = 0;

        for i in 0..n {
            let is_literal = unsafe { *parts_ptr.add(i * 2) };
            let value_bits = unsafe { *parts_ptr.add(i * 2 + 1) };

            let string_bits = if is_literal != 0 {
                // Literal — already a string, borrow it.
                (value_bits, false)
            } else {
                // Value — convert via str().
                let converted = molt_str_from_obj(value_bits);
                if obj_from_bits(converted).is_none() && exception_pending(_py) {
                    // Clean up previously owned parts.
                    for &(bits, owned) in &parts {
                        if owned {
                            dec_ref_bits(_py, bits);
                        }
                    }
                    return MoltObject::none().bits();
                }
                (converted, true)
            };

            // Get string length.
            if let Some(ptr) = obj_from_bits(string_bits.0).as_ptr() {
                unsafe {
                    if object_type_id(ptr) == TYPE_ID_STRING {
                        total_len += string_len(ptr);
                    }
                }
            }
            parts.push(string_bits);
        }

        // Single part — return it directly.
        if parts.len() == 1 {
            let (bits, owned) = parts[0];
            if !owned {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }

        // Allocate output buffer and copy all parts.
        let out_ptr = alloc_bytes_like_with_len(_py, total_len, TYPE_ID_STRING);
        if out_ptr.is_null() {
            for &(bits, owned) in &parts {
                if owned {
                    dec_ref_bits(_py, bits);
                }
            }
            return MoltObject::none().bits();
        }

        unsafe {
            let data_base = out_ptr.add(std::mem::size_of::<usize>());
            let mut offset = 0;
            for &(bits, _) in &parts {
                if let Some(ptr) = obj_from_bits(bits).as_ptr() {
                    if object_type_id(ptr) == TYPE_ID_STRING {
                        let len = string_len(ptr);
                        if len > 0 {
                            std::ptr::copy_nonoverlapping(
                                string_bytes(ptr),
                                data_base.add(offset),
                                len,
                            );
                            offset += len;
                        }
                    }
                }
            }
        }

        // Release owned references.
        for &(bits, owned) in &parts {
            if owned {
                dec_ref_bits(_py, bits);
            }
        }

        MoltObject::from_ptr(out_ptr).bits()
    })
}

/// Returns a list element WITHOUT incrementing the refcount.
/// The list holds the element alive. This mirrors CPython's
/// `PyList_GetItem()` borrowed-reference semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_getitem_borrowed(list_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(list_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                return 0;
            }
            let key = obj_from_bits(index_bits);
            let idx = if let Some(i) = to_i64(key) {
                i
            } else {
                return 0;
            };
            let len = list_len(ptr) as i64;
            let mut i = idx;
            if i < 0 {
                i += len;
            }
            if i < 0 || i >= len {
                return 0;
            }
            let elems = seq_vec_ref(ptr);
            // Borrowed: do NOT inc_ref
            elems[i as usize]
        }
    })
}

/// Returns a tuple element WITHOUT incrementing the refcount.
/// The tuple holds the element alive. This mirrors CPython's
/// `PyTuple_GetItem()` borrowed-reference semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_getitem_borrowed(tuple_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(tuple_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TUPLE {
                return 0;
            }
            let key = obj_from_bits(index_bits);
            let idx = if let Some(i) = to_i64(key) {
                i
            } else {
                return 0;
            };
            let len = tuple_len(ptr) as i64;
            let mut i = idx;
            if i < 0 {
                i += len;
            }
            if i < 0 || i >= len {
                return 0;
            }
            let elems = seq_vec_ref(ptr);
            // Borrowed: do NOT inc_ref
            elems[i as usize]
        }
    })
}

// ── Shared bytes/string helper functions (used by ops_bytes.rs and ops_string.rs) ──

#[inline]
pub(crate) fn bytes_ascii_upper(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0usize;
    // SIMD: clear bit 5 on lowercase bytes [a-z] → [A-Z]
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    let clear = vandq_u8(is_lower, case_bit);
                    let result = veorq_u8(v, clear); // XOR clears bit 5 on lowercase
                    vst1q_u8(out.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), v);
                    let is_lower = _mm_and_si128(ge_a, le_z);
                    let clear = _mm_and_si128(is_lower, case_bit);
                    let result = _mm_xor_si128(v, clear);
                    _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let lower_a = u8x16_splat(b'a');
            let lower_z = u8x16_splat(b'z');
            let case_bit = u8x16_splat(0x20);
            while i + 16 <= bytes.len() {
                let v = v128_load(bytes.as_ptr().add(i) as *const v128);
                let ge_a = u8x16_ge(v, lower_a);
                let le_z = u8x16_le(v, lower_z);
                let is_lower = v128_and(ge_a, le_z);
                let clear = v128_and(is_lower, case_bit);
                let result = v128_xor(v, clear);
                v128_store(out.as_mut_ptr().add(i) as *mut v128, result);
                i += 16;
            }
        }
    }
    for j in i..bytes.len() {
        out[j] = if bytes[j].is_ascii_lowercase() {
            bytes[j].to_ascii_uppercase()
        } else {
            bytes[j]
        };
    }
    out
}

#[inline]
pub(crate) fn bytes_ascii_lower(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0usize;
    // SIMD: set bit 5 on uppercase bytes [A-Z] → [a-z]
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let to_lower = vandq_u8(is_upper, case_bit);
                    let result = vorrq_u8(v, to_lower);
                    vst1q_u8(out.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'A' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'Z' as i8 + 1), v);
                    let is_upper = _mm_and_si128(ge_a, le_z);
                    let to_lower = _mm_and_si128(is_upper, case_bit);
                    let result = _mm_or_si128(v, to_lower);
                    _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let upper_a = u8x16_splat(b'A');
            let upper_z = u8x16_splat(b'Z');
            let case_bit = u8x16_splat(0x20);
            while i + 16 <= bytes.len() {
                let v = v128_load(bytes.as_ptr().add(i) as *const v128);
                let ge_a = u8x16_ge(v, upper_a);
                let le_z = u8x16_le(v, upper_z);
                let is_upper = v128_and(ge_a, le_z);
                let to_lower = v128_and(is_upper, case_bit);
                let result = v128_or(v, to_lower);
                v128_store(out.as_mut_ptr().add(i) as *mut v128, result);
                i += 16;
            }
        }
    }
    for j in i..bytes.len() {
        out[j] = if bytes[j].is_ascii_uppercase() {
            bytes[j].to_ascii_lowercase()
        } else {
            bytes[j]
        };
    }
    out
}

pub(crate) fn simd_is_all_ascii_whitespace(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;
    let ptr = bytes.as_ptr();

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let space = vdupq_n_u8(b' ');
            let tab = vdupq_n_u8(b'\t');
            let nl = vdupq_n_u8(b'\n');
            let cr = vdupq_n_u8(b'\r');
            let vt = vdupq_n_u8(0x0b);
            let ff = vdupq_n_u8(0x0c);
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(ptr.add(i));
                let is_ws = vorrq_u8(
                    vorrq_u8(
                        vorrq_u8(vceqq_u8(chunk, space), vceqq_u8(chunk, tab)),
                        vceqq_u8(chunk, nl),
                    ),
                    vorrq_u8(
                        vceqq_u8(chunk, cr),
                        vorrq_u8(vceqq_u8(chunk, vt), vceqq_u8(chunk, ff)),
                    ),
                );
                // If any byte is NOT whitespace, vminvq will be 0
                if vminvq_u8(is_ws) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            let space = _mm_set1_epi8(b' ' as i8);
            let tab = _mm_set1_epi8(b'\t' as i8);
            let nl = _mm_set1_epi8(b'\n' as i8);
            let cr = _mm_set1_epi8(b'\r' as i8);
            let vt = _mm_set1_epi8(0x0b);
            let ff = _mm_set1_epi8(0x0c);
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(ptr.add(i) as *const __m128i);
                let is_ws = _mm_or_si128(
                    _mm_or_si128(
                        _mm_or_si128(_mm_cmpeq_epi8(chunk, space), _mm_cmpeq_epi8(chunk, tab)),
                        _mm_cmpeq_epi8(chunk, nl),
                    ),
                    _mm_or_si128(
                        _mm_cmpeq_epi8(chunk, cr),
                        _mm_or_si128(_mm_cmpeq_epi8(chunk, vt), _mm_cmpeq_epi8(chunk, ff)),
                    ),
                );
                // All bytes must be whitespace → all mask bits must be set
                if _mm_movemask_epi8(is_ws) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let space = u8x16_splat(b' ');
            let tab = u8x16_splat(b'\t');
            let nl = u8x16_splat(b'\n');
            let cr = u8x16_splat(b'\r');
            let vt = u8x16_splat(0x0b);
            let ff = u8x16_splat(0x0c);
            while i + 16 <= bytes.len() {
                let chunk = v128_load(ptr.add(i) as *const v128);
                let is_ws = v128_or(
                    v128_or(
                        v128_or(u8x16_eq(chunk, space), u8x16_eq(chunk, tab)),
                        u8x16_eq(chunk, nl),
                    ),
                    v128_or(
                        u8x16_eq(chunk, cr),
                        v128_or(u8x16_eq(chunk, vt), u8x16_eq(chunk, ff)),
                    ),
                );
                // All bytes must be whitespace → all bitmask bits set
                if u8x16_bitmask(is_ws) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    // Scalar tail
    while i < bytes.len() {
        if !bytes_ascii_space(bytes[i]) {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD-accelerated check: are ALL bytes ASCII alphabetic [A-Za-z]?
pub(crate) fn simd_is_all_ascii_alpha(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let case_bit = vdupq_n_u8(0x20); // bit 5 forces lowercase
            let a_lower = vdupq_n_u8(b'a');
            let z_lower = vdupq_n_u8(b'z');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                // Force lowercase via OR with 0x20, then range check 'a'-'z'
                let lowered = vorrq_u8(chunk, case_bit);
                let is_alpha = vandq_u8(vcgeq_u8(lowered, a_lower), vcleq_u8(lowered, z_lower));
                if vminvq_u8(is_alpha) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            let case_bit = _mm_set1_epi8(0x20);
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let lowered = _mm_or_si128(chunk, case_bit);
                let ge_a = _mm_cmpgt_epi8(lowered, _mm_set1_epi8((b'a' - 1) as i8));
                let le_z = _mm_cmpgt_epi8(_mm_set1_epi8((b'z' + 1) as i8), lowered);
                let is_alpha = _mm_and_si128(ge_a, le_z);
                if _mm_movemask_epi8(is_alpha) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let case_bit = u8x16_splat(0x20);
            let a_lower = u8x16_splat(b'a');
            let z_lower = u8x16_splat(b'z');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let lowered = v128_or(chunk, case_bit);
                // Range check: a <= lowered <= z
                // lowered >= a: use unsigned saturating sub; if (lowered - a) didn't underflow, >= a
                let ge_a = u8x16_ge(lowered, a_lower);
                let le_z = u8x16_le(lowered, z_lower);
                let is_alpha = v128_and(ge_a, le_z);
                if u8x16_bitmask(is_alpha) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if !bytes[i].is_ascii_alphabetic() {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD-accelerated check: are ALL bytes ASCII digits [0-9]?
pub(crate) fn simd_is_all_ascii_digit(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let zero = vdupq_n_u8(b'0');
            let nine = vdupq_n_u8(b'9');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let is_digit = vandq_u8(vcgeq_u8(chunk, zero), vcleq_u8(chunk, nine));
                if vminvq_u8(is_digit) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let ge_0 = _mm_cmpgt_epi8(chunk, _mm_set1_epi8((b'0' - 1) as i8));
                let le_9 = _mm_cmpgt_epi8(_mm_set1_epi8((b'9' + 1) as i8), chunk);
                let is_digit = _mm_and_si128(ge_0, le_9);
                if _mm_movemask_epi8(is_digit) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let zero = u8x16_splat(b'0');
            let nine = u8x16_splat(b'9');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let ge_0 = u8x16_ge(chunk, zero);
                let le_9 = u8x16_le(chunk, nine);
                let is_digit = v128_and(ge_0, le_9);
                if u8x16_bitmask(is_digit) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD-accelerated check: are ALL bytes ASCII alphanumeric [A-Za-z0-9]?
pub(crate) fn simd_is_all_ascii_alnum(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let case_bit = vdupq_n_u8(0x20);
            let a_lower = vdupq_n_u8(b'a');
            let z_lower = vdupq_n_u8(b'z');
            let zero = vdupq_n_u8(b'0');
            let nine = vdupq_n_u8(b'9');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let lowered = vorrq_u8(chunk, case_bit);
                let is_alpha = vandq_u8(vcgeq_u8(lowered, a_lower), vcleq_u8(lowered, z_lower));
                let is_digit = vandq_u8(vcgeq_u8(chunk, zero), vcleq_u8(chunk, nine));
                let is_alnum = vorrq_u8(is_alpha, is_digit);
                if vminvq_u8(is_alnum) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            let case_bit = _mm_set1_epi8(0x20);
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let lowered = _mm_or_si128(chunk, case_bit);
                let ge_a = _mm_cmpgt_epi8(lowered, _mm_set1_epi8((b'a' - 1) as i8));
                let le_z = _mm_cmpgt_epi8(_mm_set1_epi8((b'z' + 1) as i8), lowered);
                let is_alpha = _mm_and_si128(ge_a, le_z);
                let ge_0 = _mm_cmpgt_epi8(chunk, _mm_set1_epi8((b'0' - 1) as i8));
                let le_9 = _mm_cmpgt_epi8(_mm_set1_epi8((b'9' + 1) as i8), chunk);
                let is_digit = _mm_and_si128(ge_0, le_9);
                let is_alnum = _mm_or_si128(is_alpha, is_digit);
                if _mm_movemask_epi8(is_alnum) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let case_bit = u8x16_splat(0x20);
            let a_lower = u8x16_splat(b'a');
            let z_lower = u8x16_splat(b'z');
            let zero = u8x16_splat(b'0');
            let nine = u8x16_splat(b'9');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let lowered = v128_or(chunk, case_bit);
                let is_alpha = v128_and(u8x16_ge(lowered, a_lower), u8x16_le(lowered, z_lower));
                let is_digit = v128_and(u8x16_ge(chunk, zero), u8x16_le(chunk, nine));
                let is_alnum = v128_or(is_alpha, is_digit);
                if u8x16_bitmask(is_alnum) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if !bytes[i].is_ascii_alphanumeric() {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD-accelerated check: are ALL bytes ASCII printable [0x20..0x7E]?
pub(crate) fn simd_is_all_ascii_printable(bytes: &[u8]) -> bool {
    // Empty string is "printable" per Python semantics
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let lo = vdupq_n_u8(0x20);
            let hi = vdupq_n_u8(0x7E);
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let is_print = vandq_u8(vcgeq_u8(chunk, lo), vcleq_u8(chunk, hi));
                if vminvq_u8(is_print) == 0 {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let ge_lo = _mm_cmpgt_epi8(chunk, _mm_set1_epi8(0x1F));
                let le_hi = _mm_cmpgt_epi8(_mm_set1_epi8(0x7F_u8 as i8), chunk);
                let is_print = _mm_and_si128(ge_lo, le_hi);
                if _mm_movemask_epi8(is_print) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let lo = u8x16_splat(0x20);
            let hi = u8x16_splat(0x7E);
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let is_print = v128_and(u8x16_ge(chunk, lo), u8x16_le(chunk, hi));
                if u8x16_bitmask(is_print) != 0xFFFF {
                    return false;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        let b = bytes[i];
        if !(0x20..=0x7E).contains(&b) {
            return false;
        }
        i += 1;
    }
    true
}

/// SIMD check: does the buffer contain ANY uppercase ASCII letter [A-Z]?
pub(crate) fn simd_has_any_ascii_upper(bytes: &[u8]) -> bool {
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let a_upper = vdupq_n_u8(b'A');
            let z_upper = vdupq_n_u8(b'Z');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let is_upper = vandq_u8(vcgeq_u8(chunk, a_upper), vcleq_u8(chunk, z_upper));
                if vmaxvq_u8(is_upper) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let ge_a = _mm_cmpgt_epi8(chunk, _mm_set1_epi8((b'A' - 1) as i8));
                let le_z = _mm_cmpgt_epi8(_mm_set1_epi8((b'Z' + 1) as i8), chunk);
                let is_upper = _mm_and_si128(ge_a, le_z);
                if _mm_movemask_epi8(is_upper) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let a_upper = u8x16_splat(b'A');
            let z_upper = u8x16_splat(b'Z');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let is_upper = v128_and(u8x16_ge(chunk, a_upper), u8x16_le(chunk, z_upper));
                if u8x16_bitmask(is_upper) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if bytes[i].is_ascii_uppercase() {
            return true;
        }
        i += 1;
    }
    false
}

/// SIMD check: does the buffer contain ANY lowercase ASCII letter [a-z]?
pub(crate) fn simd_has_any_ascii_lower(bytes: &[u8]) -> bool {
    let mut i = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let a_lower = vdupq_n_u8(b'a');
            let z_lower = vdupq_n_u8(b'z');
            while i + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(i));
                let is_lower = vandq_u8(vcgeq_u8(chunk, a_lower), vcleq_u8(chunk, z_lower));
                if vmaxvq_u8(is_lower) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while i + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                let ge_a = _mm_cmpgt_epi8(chunk, _mm_set1_epi8((b'a' - 1) as i8));
                let le_z = _mm_cmpgt_epi8(_mm_set1_epi8((b'z' + 1) as i8), chunk);
                let is_lower = _mm_and_si128(ge_a, le_z);
                if _mm_movemask_epi8(is_lower) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let a_lower = u8x16_splat(b'a');
            let z_lower = u8x16_splat(b'z');
            while i + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                let is_lower = v128_and(u8x16_ge(chunk, a_lower), u8x16_le(chunk, z_lower));
                if u8x16_bitmask(is_lower) != 0 {
                    return true;
                }
                i += 16;
            }
        }
    }

    while i < bytes.len() {
        if bytes[i].is_ascii_lowercase() {
            return true;
        }
        i += 1;
    }
    false
}

pub(crate) fn bytes_ascii_capitalize(bytes: &[u8]) -> Vec<u8> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; bytes.len()];
    // First byte: capitalize
    out[0] = if bytes[0].is_ascii_lowercase() {
        bytes[0].to_ascii_uppercase()
    } else {
        bytes[0]
    };
    // Rest: SIMD-accelerated lowercasing (set bit 5 on uppercase bytes)
    let rest = &bytes[1..];
    let mut i = 0usize;
    #[cfg(target_arch = "aarch64")]
    {
        if rest.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= rest.len() {
                    let v = vld1q_u8(rest.as_ptr().add(i));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let to_lower = vandq_u8(is_upper, case_bit);
                    let result = vorrq_u8(v, to_lower);
                    vst1q_u8(out.as_mut_ptr().add(1 + i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if rest.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= rest.len() {
                    let v = _mm_loadu_si128(rest.as_ptr().add(i) as *const __m128i);
                    let ge_a = _mm_cmpgt_epi8(v, _mm_set1_epi8(b'A' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'Z' as i8 + 1), v);
                    let is_upper = _mm_and_si128(ge_a, le_z);
                    let to_lower = _mm_and_si128(is_upper, case_bit);
                    let result = _mm_or_si128(v, to_lower);
                    _mm_storeu_si128(out.as_mut_ptr().add(1 + i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    // Scalar tail
    for j in i..rest.len() {
        out[1 + j] = if rest[j].is_ascii_uppercase() {
            rest[j].to_ascii_lowercase()
        } else {
            rest[j]
        };
    }
    out
}

pub(crate) fn bytes_ascii_swapcase(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0usize;
    // SIMD fast path: toggle bit 5 on alphabetic bytes (16 bytes at a time)
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');
                let case_bit = vdupq_n_u8(0x20);
                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let is_alpha = vorrq_u8(is_lower, is_upper);
                    let flip = vandq_u8(is_alpha, case_bit);
                    let result = veorq_u8(v, flip);
                    vst1q_u8(out.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);
                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    // Check lower: a <= v <= z (use unsigned saturation trick)
                    let shifted = _mm_or_si128(v, case_bit); // force to lowercase
                    let ge_a = _mm_cmpgt_epi8(shifted, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), shifted);
                    let is_alpha = _mm_and_si128(ge_a, le_z);
                    let flip = _mm_and_si128(is_alpha, case_bit);
                    let result = _mm_xor_si128(v, flip);
                    _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }
    // Scalar tail
    for j in i..bytes.len() {
        let b = bytes[j];
        out[j] = if b.is_ascii_lowercase() {
            b.to_ascii_uppercase()
        } else if b.is_ascii_uppercase() {
            b.to_ascii_lowercase()
        } else {
            b
        };
    }
    out
}

pub(crate) fn bytes_ascii_title(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; bytes.len()];
    let mut i = 0usize;
    let mut at_word_start = true;

    // SIMD fast path: process 16 bytes at a time.
    // For each chunk, classify bytes as alpha/non-alpha, then compute word-start
    // boundaries based on the at_word_start carry from the previous chunk.
    // Title case = uppercase at word start, lowercase otherwise, for alpha bytes.
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            unsafe {
                use std::arch::aarch64::*;
                let lower_a = vdupq_n_u8(b'a');
                let lower_z = vdupq_n_u8(b'z');
                let upper_a = vdupq_n_u8(b'A');
                let upper_z = vdupq_n_u8(b'Z');

                while i + 16 <= bytes.len() {
                    let v = vld1q_u8(bytes.as_ptr().add(i));
                    let is_lower = vandq_u8(vcgeq_u8(v, lower_a), vcleq_u8(v, lower_z));
                    let is_upper = vandq_u8(vcgeq_u8(v, upper_a), vcleq_u8(v, upper_z));
                    let is_alpha = vorrq_u8(is_lower, is_upper);

                    // Extract alpha mask to do sequential word-boundary tracking
                    let mut alpha_bytes = [0u8; 16];
                    vst1q_u8(alpha_bytes.as_mut_ptr(), is_alpha);
                    let mut src_bytes = [0u8; 16];
                    vst1q_u8(src_bytes.as_mut_ptr(), v);
                    let mut result_bytes = [0u8; 16];

                    for j in 0..16 {
                        let b = src_bytes[j];
                        if alpha_bytes[j] != 0 {
                            if at_word_start {
                                result_bytes[j] = b & !0x20; // to_ascii_uppercase
                                at_word_start = false;
                            } else {
                                result_bytes[j] = b | 0x20; // to_ascii_lowercase
                            }
                        } else {
                            result_bytes[j] = b;
                            at_word_start = true;
                        }
                    }

                    let result = vld1q_u8(result_bytes.as_ptr());
                    vst1q_u8(out.as_mut_ptr().add(i), result);
                    i += 16;
                }
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            unsafe {
                use std::arch::x86_64::*;
                let case_bit = _mm_set1_epi8(0x20);

                while i + 16 <= bytes.len() {
                    let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                    let shifted = _mm_or_si128(v, case_bit);
                    let ge_a = _mm_cmpgt_epi8(shifted, _mm_set1_epi8(b'a' as i8 - 1));
                    let le_z = _mm_cmpgt_epi8(_mm_set1_epi8(b'z' as i8 + 1), shifted);
                    let is_alpha = _mm_and_si128(ge_a, le_z);
                    let alpha_mask = _mm_movemask_epi8(is_alpha) as u32;

                    let mut src_bytes = [0u8; 16];
                    _mm_storeu_si128(src_bytes.as_mut_ptr() as *mut __m128i, v);
                    let mut result_bytes = [0u8; 16];

                    for j in 0..16 {
                        let b = src_bytes[j];
                        if alpha_mask & (1 << j) != 0 {
                            if at_word_start {
                                result_bytes[j] = b & !0x20;
                                at_word_start = false;
                            } else {
                                result_bytes[j] = b | 0x20;
                            }
                        } else {
                            result_bytes[j] = b;
                            at_word_start = true;
                        }
                    }

                    let result = _mm_loadu_si128(result_bytes.as_ptr() as *const __m128i);
                    _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
                    i += 16;
                }
            }
        }
    }

    // Scalar tail
    for j in i..bytes.len() {
        let b = bytes[j];
        if b.is_ascii_alphabetic() {
            if at_word_start {
                out[j] = b.to_ascii_uppercase();
                at_word_start = false;
            } else {
                out[j] = b.to_ascii_lowercase();
            }
        } else {
            out[j] = b;
            at_word_start = true;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// CPython specialized bytecode fast paths (BINARY_SUBSCR_LIST_INT,
// STORE_SUBSCR_LIST_INT, COMPARE_OP_INT, COMPARE_OP_STR).
// These functions are extern "C" so they can be emitted as direct calls by
// the AOT compiler back-end instead of routing through the generic dispatch.
// ---------------------------------------------------------------------------

/// Fast path: integer index into a list (BINARY_SUBSCR_LIST_INT).
///
/// Handles positive and negative indexing with direct array access.
/// On any failure (wrong type tags, out-of-bounds) falls through to
/// the full `molt_index` slow path.
///
/// Returns the element bits on success, or `u64::MAX` as a sentinel to
/// signal the caller to fall back to `molt_index`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_getitem_int_fast(list_bits: u64, index_bits: u64) -> u64 {
    // 1. Fast tag check: index must be a NaN-boxed int.
    let index_obj = obj_from_bits(index_bits);
    if !index_obj.is_int() {
        return molt_index(list_bits, index_bits);
    }
    // 2. List must be a heap pointer.
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return molt_index(list_bits, index_bits);
    };
    unsafe {
        // 3. Must actually be a list.
        if object_type_id(ptr) != TYPE_ID_LIST {
            return molt_index(list_bits, index_bits);
        }
        // 4. Extract index and list length.
        let mut idx = index_obj.as_int_unchecked();
        let elems = seq_vec_ref(ptr);
        let len = elems.len() as i64;
        // 5. Handle negative indexing.
        if idx < 0 {
            idx += len;
        }
        // 6. Bounds check.
        if idx < 0 || idx >= len {
            return molt_index(list_bits, index_bits);
        }
        // 7. Direct array load and reference-count increment.
        // Skip with_gil_entry! — compiled code already holds the GIL and
        // inc_ref is just an atomic fetch_add that cannot panic. Eliminating
        // catch_unwind saves ~15ns per list access in hot loops.
        let val = elems[idx as usize];
        let val_obj = obj_from_bits(val);
        if let Some(val_ptr) = val_obj.as_ptr() {
            let header = val_ptr.sub(std::mem::size_of::<crate::object::MoltHeader>())
                as *mut crate::object::MoltHeader;
            if ((*header).flags & crate::object::HEADER_FLAG_IMMORTAL) == 0 {
                (*header).ref_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        val
    }
}

// ── Specialized list[int] operations ────────────────────────────────
//
// When the compiler proves a list contains only integers, it uses these
// specialized functions that store raw i64 values without NaN-boxing.
// Element access is a single array load + box_int on return.
// No refcounting needed (ints are NaN-boxed inline, not heap-allocated).

/// Allocate a specialized list[int] with raw i64 storage.
/// Elements are stored as raw i64 (NOT NaN-boxed).
/// Returns a NaN-boxed pointer to the list object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_new(count: u64, fill_value: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Both arguments are NaN-boxed — unbox the count
        let count_obj = obj_from_bits(count);
        let n = if count_obj.is_int() {
            let v = count_obj.as_int_unchecked();
            if v < 0 { 0usize } else { v as usize }
        } else if count_obj.is_bool() {
            if count_obj.as_bool().unwrap_or(false) { 1 } else { 0 }
        } else {
            return MoltObject::none().bits();
        };
        // Extract raw int from the NaN-boxed fill value
        let fill_obj = obj_from_bits(fill_value);
        let fill_raw = if fill_obj.is_none() {
            0i64
        } else if fill_obj.is_int() {
            fill_obj.as_int_unchecked()
        } else if fill_obj.is_bool() {
            if fill_obj.as_bool().unwrap_or(false) { 1i64 } else { 0i64 }
        } else {
            // Not an int — fall back to regular list
            return MoltObject::none().bits();
        };

        let total = std::mem::size_of::<crate::object::MoltHeader>()
            + std::mem::size_of::<*mut Vec<i64>>()  // Vec pointer
            + std::mem::size_of::<u64>();            // padding
        let ptr = alloc_object(_py, total, TYPE_ID_LIST_INT);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let mut vec = Vec::with_capacity(n);
            vec.resize(n, fill_raw);
            let vec_ptr = Box::into_raw(Box::new(vec));
            *(ptr as *mut *mut Vec<i64>) = vec_ptr;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Get element from a specialized list[int].
/// Returns a NaN-boxed int (boxes the raw i64 on return).
/// No refcounting needed — ints are inline NaN-boxed values.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem(list_bits: u64, index_bits: u64) -> u64 {
    let index_obj = obj_from_bits(index_bits);
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        let mut idx = if index_obj.is_int() {
            index_obj.as_int_unchecked()
        } else {
            return molt_index(list_bits, index_bits);
        };
        let vec_ptr = *(ptr as *mut *mut Vec<i64>);
        let data = (*vec_ptr).as_ptr();
        let len = (*vec_ptr).len() as i64;
        if idx < 0 { idx += len; }
        if idx < 0 || idx >= len {
            return molt_index(list_bits, index_bits);
        }
        let raw_val = *data.add(idx as usize);
        // Box the raw i64 into a NaN-boxed int — no heap allocation, no refcount
        MoltObject::from_int(raw_val).bits()
    }
}

/// Raw-register fast path for list[int] getitem.
/// Takes a raw i64 index (NOT NaN-boxed) and returns a raw i64 value (NOT NaN-boxed).
/// Eliminates NaN-box/unbox round-trips when both index and result stay in raw_int_shadow.
/// Returns 0 on out-of-bounds (matching Python's behavior for sieve-like patterns where
/// the caller checks truthiness — 0 is falsy).
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_raw(list_bits: u64, raw_index: i64) -> i64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return 0;
    };
    unsafe {
        let vec_ptr = *(ptr as *mut *mut Vec<i64>);
        let data = (*vec_ptr).as_ptr();
        let len = (*vec_ptr).len() as i64;
        let mut idx = raw_index;
        if idx < 0 { idx += len; }
        if idx < 0 || idx >= len {
            return 0;
        }
        *data.add(idx as usize)
    }
}

/// Raw-register fast path for list[int] setitem.
/// Takes raw i64 index and value (NOT NaN-boxed). Stores value directly into the flat i64 array.
/// Returns list_bits unchanged (matching molt_list_int_setitem contract).
/// No-op on out-of-bounds.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_setitem_raw(list_bits: u64, raw_index: i64, raw_value: i64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return list_bits;
    };
    unsafe {
        let vec_ptr = *(ptr as *mut *mut Vec<i64>);
        let vec = &mut *vec_ptr;
        let len = vec.len() as i64;
        let mut idx = raw_index;
        if idx < 0 { idx += len; }
        if idx < 0 || idx >= len {
            return list_bits;
        }
        vec[idx as usize] = raw_value;
        list_bits
    }
}

/// GIL-free list[int] getitem with NaN-boxed interface.
///
/// Identical to `molt_list_int_getitem` (which already skips GIL), but named
/// `_nogil` to make the contract explicit for the compiler backend.
/// No GIL acquisition, no catch_unwind, no signal checks.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_nogil(list_bits: u64, index_bits: u64) -> u64 {
    molt_list_int_getitem(list_bits, index_bits)
}

/// GIL-free list[int] setitem with NaN-boxed interface.
///
/// Identical to `molt_list_int_setitem` (which already skips GIL), but named
/// `_nogil` to make the contract explicit for the compiler backend.
/// No GIL acquisition, no catch_unwind, no signal checks.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_setitem_nogil(list_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    molt_list_int_setitem(list_bits, index_bits, value_bits)
}

/// Set element in a specialized list[int].
/// Expects a NaN-boxed int value — extracts raw i64 and stores directly.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_setitem(list_bits: u64, index_bits: u64, value_bits: u64) -> u64 {
    let index_obj = obj_from_bits(index_bits);
    let list_obj = obj_from_bits(list_bits);
    let value_obj = obj_from_bits(value_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        let mut idx = if index_obj.is_int() {
            index_obj.as_int_unchecked()
        } else {
            return MoltObject::none().bits();
        };
        let raw_value = if value_obj.is_int() {
            value_obj.as_int_unchecked()
        } else if value_obj.is_bool() {
            if value_obj.as_bool().unwrap_or(false) { 1i64 } else { 0i64 }
        } else {
            0i64 // store 0 for non-int values (False → 0)
        };
        let vec_ptr = *(ptr as *mut *mut Vec<i64>);
        let vec = &mut *vec_ptr;
        let len = vec.len() as i64;
        if idx < 0 { idx += len; }
        if idx < 0 || idx >= len {
            return MoltObject::none().bits();
        }
        vec[idx as usize] = raw_value;
        // No refcount changes — raw i64 values have no heap allocation.
        // Return the list itself to match the molt_store_index contract
        // (the compiler may assign the result back to the container variable).
        list_bits
    }
}

/// Return the raw data pointer of a list (regular or list_int).
///
/// Works for both regular lists (Vec<u64>) and list_int (Vec<i64>) since both
/// have the same memory layout: obj_ptr stores `*mut Vec<T>` at offset 0, and
/// Vec's data pointer is at offset 0/8 of the Vec struct (platform-dependent
/// but stable via Vec::as_ptr).
/// The returned pointer is valid only as long as the list is not resized.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_data(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return 0;
    };
    unsafe {
        let vec_ptr = *(ptr as *mut *mut Vec<u64>);
        (*vec_ptr).as_ptr() as u64
    }
}

/// Return the length of a list (regular or list_int) as a raw u64.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_len_raw(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return 0;
    };
    unsafe {
        let vec_ptr = *(ptr as *mut *mut Vec<u64>);
        (*vec_ptr).len() as u64
    }
}

/// Get length of a specialized list[int].
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_len(list_bits: u64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::from_int(0).bits();
    };
    unsafe {
        let vec_ptr = *(ptr as *mut *mut Vec<i64>);
        MoltObject::from_int((*vec_ptr).len() as i64).bits()
    }
}

/// Check if value is truthy in a specialized list[int] element context.
/// Raw i64: 0 is falsy, everything else is truthy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_int_getitem_truthy(list_bits: u64, index_bits: u64) -> u64 {
    let index_obj = obj_from_bits(index_bits);
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return MoltObject::from_bool(false).bits();
    };
    unsafe {
        let mut idx = if index_obj.is_int() {
            index_obj.as_int_unchecked()
        } else {
            return MoltObject::from_bool(false).bits();
        };
        let vec_ptr = *(ptr as *mut *mut Vec<i64>);
        let data = (*vec_ptr).as_ptr();
        let len = (*vec_ptr).len() as i64;
        if idx < 0 { idx += len; }
        if idx < 0 || idx >= len {
            return MoltObject::from_bool(false).bits();
        }
        let raw_val = *data.add(idx as usize);
        MoltObject::from_bool(raw_val != 0).bits()
    }
}

/// Fast path: integer index store into a list (STORE_SUBSCR_LIST_INT).
///
/// On any failure falls through to the full `molt_store_index` slow path.
/// Returns the container bits on success (matching `molt_store_index`),
/// or `MoltObject::none().bits()` on error.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_setitem_int_fast(
    list_bits: u64,
    index_bits: u64,
    val_bits: u64,
) -> u64 {
    // 1. Fast tag check: index must be a NaN-boxed int.
    let index_obj = obj_from_bits(index_bits);
    if !index_obj.is_int() {
        return molt_store_index(list_bits, index_bits, val_bits);
    }
    // 2. List must be a heap pointer.
    let list_obj = obj_from_bits(list_bits);
    let Some(ptr) = list_obj.as_ptr() else {
        return molt_store_index(list_bits, index_bits, val_bits);
    };
    unsafe {
        // 3. Must actually be a list.
        if object_type_id(ptr) != TYPE_ID_LIST {
            return molt_store_index(list_bits, index_bits, val_bits);
        }
        // 4. Extract index and list length.
        let mut idx = index_obj.as_int_unchecked();
        let len = list_len(ptr) as i64;
        // 5. Handle negative indexing.
        if idx < 0 {
            idx += len;
        }
        // 6. Bounds check — fall through to slow path which raises IndexError.
        if idx < 0 || idx >= len {
            return molt_store_index(list_bits, index_bits, val_bits);
        }
        // 7. Direct array store with reference count update.
        crate::with_gil_entry!(_py, {
            let elems = seq_vec(ptr);
            let old_bits = elems[idx as usize];
            if old_bits != val_bits {
                dec_ref_bits(_py, old_bits);
                inc_ref_bits(_py, val_bits);
                elems[idx as usize] = val_bits;
                if crate::object::refcount_opt::is_heap_ref(val_bits) {
                    (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
                }
            }
            list_bits
        })
    }
}

/// Fast path: compare two NaN-boxed integers (COMPARE_OP_INT).
///
/// `op` encodes the comparison:
///   0 = Lt, 1 = Le, 2 = Eq, 3 = Ne, 4 = Gt, 5 = Ge
///
/// If either operand is not a NaN-boxed int the call falls through to the
/// appropriate generic comparison function.  Both booleans and int subclasses
/// are handled by the slow path.
#[unsafe(no_mangle)]
pub extern "C" fn molt_compare_int_fast(a: u64, b: u64, op: u32) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    // Both operands must be plain NaN-boxed ints (not bools, not subclasses).
    if lhs.is_int() && rhs.is_int() {
        let li = lhs.as_int_unchecked();
        let ri = rhs.as_int_unchecked();
        let result = match op {
            0 => li < ri,
            1 => li <= ri,
            2 => li == ri,
            3 => li != ri,
            4 => li > ri,
            5 => li >= ri,
            _ => return molt_eq(a, b), // unknown op: safe fallback
        };
        return MoltObject::from_bool(result).bits();
    }
    // Slow path: delegate to the full generic comparison.
    match op {
        0 => molt_lt(a, b),
        1 => molt_le(a, b),
        2 => molt_eq(a, b),
        3 => molt_ne(a, b),
        4 => molt_gt(a, b),
        5 => molt_ge(a, b),
        _ => molt_eq(a, b),
    }
}

/// Fast path: string equality using pointer identity (COMPARE_OP_STR).
///
/// In Molt, every string object has a unique allocation, so pointer equality
/// immediately proves equality.  If the pointers are the same we return `true`
/// without touching the bytes.  If they differ we fall back to the byte-wise
/// comparison already inside `molt_string_eq`.
///
/// This function is intentionally `unsafe`-free at the call site — it wraps
/// the unsafe pointer dereferences internally and returns an `Option`:
///   `Some(true)`  — pointers equal → strings equal
///   `Some(false)` — strings are TYPE_ID_STRING but different lengths
///   `None`        — one or both operands are not strings; caller should
///                   fall through to `molt_eq`
#[inline]
fn string_eq_fast(a: u64, b: u64) -> Option<bool> {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    let lp = lhs.as_ptr()?;
    let rp = rhs.as_ptr()?;
    unsafe {
        if object_type_id(lp) != TYPE_ID_STRING || object_type_id(rp) != TYPE_ID_STRING {
            return None;
        }
        // Pointer equality: same allocation → same content.
        if lp == rp {
            return Some(true);
        }
        // Length mismatch: definitely not equal (avoids byte scan).
        let l_len = string_len(lp);
        let r_len = string_len(rp);
        if l_len != r_len {
            return Some(false);
        }
        // Fall through to byte comparison.
        None
    }
}

/// Extern fast-path wrapper for string equality (COMPARE_OP_STR).
///
/// Uses `string_eq_fast` for the pointer/length checks, then delegates to
/// `molt_string_eq` for byte comparison when needed.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_eq_fast(a: u64, b: u64) -> u64 {
    match string_eq_fast(a, b) {
        Some(result) => MoltObject::from_bool(result).bits(),
        None => molt_string_eq(a, b),
    }
}

/// Unchecked list getitem — used when BCE (Bounds Check Elimination) has proven
/// the index is in bounds.
///
/// # Safety
/// The caller guarantees:
///   - `list_bits` is a valid NaN-boxed heap pointer to a TYPE_ID_LIST object.
///   - `0 <= index < len(list)` — no bounds check is performed.
///   - The list is not mutated concurrently (GIL must be held by the caller).
///
/// Violating any of these preconditions causes undefined behaviour.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_getitem_unchecked(list_bits: u64, index: i64) -> u64 {
    let list_obj = obj_from_bits(list_bits);
    // Safety: caller guarantees list_bits is a valid list heap pointer.
    let ptr = unsafe { list_obj.as_ptr().unwrap_unchecked() };
    unsafe {
        let elems = seq_vec_ref(ptr);
        // Safety: caller guarantees 0 <= index < len.
        let val = *elems.get_unchecked(index as usize);
        crate::with_gil_entry!(_py, {
            inc_ref_bits(_py, val);
            val
        })
    }
}
