// Re-export iter impl functions for backward compatibility with crate::object::ops::* paths
use crate::object::{ClassEdgeOwnership, object_replace_class_edge};

#[cfg(test)]
mod profile_epoch_tests;
#[cfg(test)]
mod unpack_transaction_tests;

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
    HashContext, HashSecret, ensure_hashable, fatal_hash_seed, hash_bits, hash_bits_signed,
    hash_int, hash_pointer, hash_slice_bits, hash_string_bytes,
};

// Re-export encoding functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_encoding::{
    DecodeTextError, EncodeError, decode_bytes_text, decode_error_byte, decode_error_range,
    encode_error_reason, encode_string_with_errors, encoding_kind_name, is_surrogate,
    normalize_encoding, unicode_escape,
};

mod ascii_bytes;
mod dict_set_tables;
mod equality;
mod fast_compare;
mod specialized_list;
mod subscript;

#[cfg(target_os = "windows")]
pub use super::ops_sys::molt_set_argv_utf16;
pub(crate) use super::ops_sys::parse_codec_arg;
pub use super::ops_sys::{
    molt_chr, molt_ellipsis, molt_gc_callbacks, molt_gc_collect, molt_gc_disable, molt_gc_enable,
    molt_gc_freeze, molt_gc_garbage, molt_gc_get_count, molt_gc_get_debug,
    molt_gc_get_freeze_count, molt_gc_get_objects, molt_gc_get_referents, molt_gc_get_referrers,
    molt_gc_get_stats, molt_gc_get_threshold, molt_gc_is_finalized, molt_gc_is_tracked,
    molt_gc_isenabled, molt_gc_set_debug, molt_gc_set_threshold, molt_gc_unfreeze, molt_getargv,
    molt_getpid, molt_getrecursionlimit, molt_hash_builtin, molt_id, molt_int_from_bytes,
    molt_int_to_bytes, molt_len, molt_len_dict, molt_len_list, molt_len_set, molt_len_str,
    molt_len_tuple, molt_missing, molt_not_implemented, molt_object_hash, molt_ord, molt_pending,
    molt_raise_recursion_error, molt_recursion_enter_fast, molt_recursion_exit_fast,
    molt_recursion_guard_enter, molt_recursion_guard_exit, molt_set_argv, molt_setrecursionlimit,
    molt_signal_raise, molt_sys_abiflags, molt_sys_api_version, molt_sys_executable,
    molt_sys_flags_payload, molt_sys_hexversion, molt_sys_implementation_payload,
    molt_sys_set_version_info, molt_sys_version, molt_sys_version_info, molt_time_altzone,
    molt_time_asctime, molt_time_daylight, molt_time_get_clock_info, molt_time_gmtime,
    molt_time_localtime, molt_time_mktime, molt_time_monotonic, molt_time_monotonic_ns,
    molt_time_perf_counter, molt_time_perf_counter_ns, molt_time_process_time,
    molt_time_process_time_ns, molt_time_sleep, molt_time_strftime, molt_time_time,
    molt_time_time_ns, molt_time_timegm, molt_time_timezone, molt_time_tzname,
    molt_traceback_exception_chain_payload, molt_traceback_exception_components,
    molt_traceback_extract_tb, molt_traceback_format_caret_line, molt_traceback_format_exc,
    molt_traceback_format_exception, molt_traceback_format_exception_only,
    molt_traceback_format_stack, molt_traceback_format_tb, molt_traceback_infer_col_offsets,
    molt_traceback_payload, molt_traceback_source_line,
};
pub(crate) use ascii_bytes::{
    bytes_ascii_capitalize, bytes_ascii_lower, bytes_ascii_swapcase, bytes_ascii_title,
    bytes_ascii_upper, simd_has_any_ascii_lower, simd_has_any_ascii_upper, simd_is_all_ascii_alnum,
    simd_is_all_ascii_alpha, simd_is_all_ascii_digit, simd_is_all_ascii_printable,
    simd_is_all_ascii_whitespace,
};
pub(in crate::object) use dict_set_tables::simd_contains_u64;
pub(crate) use dict_set_tables::{
    checked_dict_table_capacity, dict_clear_in_place, dict_clear_in_place_shutdown,
    dict_clear_method, dict_copy_method, dict_del_in_place, dict_find_entry, dict_find_entry_fast,
    dict_find_entry_kv_in_place, dict_fromkeys_method, dict_get_in_place, dict_get_method,
    dict_get_str_bytes_borrowed, dict_inc_in_place, dict_inc_prehashed_string_key_in_place,
    dict_items_method, dict_keys_method, dict_popitem_method, dict_rebuild, dict_set_in_place,
    dict_set_inline_int_in_place, dict_setdefault_method, dict_table_capacity, dict_update_method,
    dict_update_set_via_store, dict_values_method, set_add_in_place, set_del_in_place,
    set_find_entry, set_find_entry_fast, set_replace_entries, set_table_capacity,
};
pub(super) use dict_set_tables::{
    concat_bytes_like, fill_repeated_bytes, set_rebuild, simd_bytes_eq,
};
pub use dict_set_tables::{
    molt_string_split_sep_dict_inc, molt_string_split_ws_dict_inc, molt_taq_ingest_line,
};
pub(crate) use equality::obj_eq;
pub(super) use equality::{
    BinaryDunderOutcome, call_binary_dunder, call_dunder_raw, call_inplace_dunder,
    eq_bool_from_bits,
};
pub use fast_compare::{molt_compare_int_fast, molt_string_eq_fast};
pub use specialized_list::{
    molt_list_bool_getitem, molt_list_bool_setitem, molt_list_fill_new, molt_list_getitem_int_fast,
    molt_list_getitem_raw_idx, molt_list_getitem_unchecked, molt_list_int_data,
    molt_list_int_getitem, molt_list_int_getitem_raw, molt_list_int_getitem_raw_checked,
    molt_list_int_getitem_truthy, molt_list_int_len, molt_list_int_len_raw, molt_list_int_new,
    molt_list_int_setitem,
};
pub(crate) use subscript::value_supports_mp_subscript;
pub use subscript::{
    molt_contains, molt_del_index, molt_delitem_method, molt_getitem_method,
    molt_getitem_unchecked, molt_index, molt_list_contains, molt_ord_at, molt_setitem_method,
    molt_store_index, molt_str_contains,
};

use crate::object::layout::{range_start_bits, range_step_bits, range_stop_bits};
use crate::object::ops_bytes::{
    BytesCtorKind, bytes_ascii_space, bytes_item_to_u8, collect_bytearray_assign_bytes,
};
use crate::*;
use memchr::{memchr, memmem};
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::{Mutex, OnceLock};

use super::ops_string::{push_wtf8_codepoint, utf8_char_to_byte_index_cached, wtf8_codepoint_at};
use super::ops_sys::runtime_target_minor;
use crate::runtime_profile_schema_generated::{
    PROFILE_EPOCH_KIND, PROFILE_EPOCH_SCHEMA_VERSION, RuntimeProfileMetricSemantic,
    RuntimeProfileSnapshot, runtime_profile_epoch_memory_payload, runtime_profile_memory_payload,
    runtime_profile_metric_semantic,
};

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
    if let Some(f) = as_float_extended(val) {
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

#[inline]
fn debug_subscript_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_SUBSCRIPT").as_deref() == Ok("1"))
}

/// Cached `MOLT_DEBUG_DICT_SUBCLASS` flag. `dict_subclass_storage_bits` runs on
/// every dict-subclass storage access (hot for Counter/OrderedDict-style
/// subclasses); reading the env var there would take the libc environ lock and
/// heap-allocate per access. Cache it like the sibling debug flags above.
#[inline]
fn debug_dict_subclass_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_DICT_SUBCLASS").as_deref() == Ok("1"))
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

// ---------------------------------------------------------------------------
// Heap-allocated NaN floats
// ---------------------------------------------------------------------------
//
// All non-NaN floats are stored inline in the NaN-box (zero overhead).
// NaN floats are heap-allocated as TYPE_ID_FLOAT pointer-tagged objects so
// that each `float('nan')` call produces a unique pointer address, making
// bit-equality correct for identity checks (`nan is nan` → True,
// `float('nan') is float('nan')` → False).
//
// Layout: MoltHeader (16 bytes) + f64 (8 bytes) = 24 bytes total.

/// Allocate a heap float and return its NaN-boxed bits (pointer-tagged).
///
/// # Safety
/// Caller must hold the GIL.
#[unsafe(no_mangle)]
pub extern "C" fn molt_alloc_heap_float(value: f64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { alloc_heap_float(_py, value) })
}

/// Internal helper: allocate a heap float, returning NaN-boxed pointer bits.
pub(crate) fn alloc_heap_float(_py: &PyToken<'_>, value: f64) -> u64 {
    let header_size = std::mem::size_of::<MoltHeader>();
    let total = header_size + std::mem::size_of::<f64>();
    let ptr = alloc_object(_py, total, TYPE_ID_FLOAT);
    if ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "");
    }
    unsafe {
        *(ptr as *mut f64) = value;
    }
    MoltObject::from_ptr(ptr).bits()
}

/// Extract the f64 value from a heap float pointer (TYPE_ID_FLOAT).
///
/// # Safety
/// `ptr` must be a valid pointer to the payload of a TYPE_ID_FLOAT object.
#[inline(always)]
pub(crate) unsafe fn heap_float_value(ptr: *mut u8) -> f64 {
    unsafe { *(ptr as *const f64) }
}

/// Produce NaN-boxed bits for a float result.  Non-NaN values are stored
/// inline (zero overhead); NaN values are heap-allocated.
#[inline(always)]
pub(crate) fn float_result_bits(_py: &PyToken<'_>, value: f64) -> u64 {
    if value.is_nan() {
        alloc_heap_float(_py, value)
    } else {
        MoltObject::from_float(value).bits()
    }
}

/// Extended float check: returns true for both inline floats (non-NaN)
/// AND heap-allocated floats (TYPE_ID_FLOAT).
#[inline(always)]
pub(crate) fn is_float_extended(obj: MoltObject) -> bool {
    if obj.is_float() {
        return true;
    }
    if let Some(ptr) = obj.as_ptr() {
        return unsafe { object_type_id(ptr) } == TYPE_ID_FLOAT;
    }
    false
}

/// Extended float extraction: returns the f64 value for both inline floats
/// AND heap-allocated floats (TYPE_ID_FLOAT).
#[inline(always)]
pub(crate) fn as_float_extended(obj: MoltObject) -> Option<f64> {
    if let Some(f) = obj.as_float() {
        return Some(f);
    }
    if let Some(ptr) = obj.as_ptr()
        && unsafe { object_type_id(ptr) } == TYPE_ID_FLOAT
    {
        return Some(unsafe { heap_float_value(ptr) });
    }
    None
}

// --- NaN-boxed ops ---

pub(crate) fn runtime_profile_payload(
    _py: &PyToken<'_>,
    emit_leak_warning: bool,
) -> serde_json::Value {
    let call_dispatch = CALL_DISPATCH_COUNT.load(AtomicOrdering::Relaxed);
    let cache_hit = STRING_COUNT_CACHE_HIT_COUNT.load(AtomicOrdering::Relaxed);
    let cache_miss = STRING_COUNT_CACHE_MISS_COUNT.load(AtomicOrdering::Relaxed);
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
        GUARD_DICT_SHAPE_LAYOUT_FAIL_EXPECTED_VERSION_INVALID_COUNT.load(AtomicOrdering::Relaxed);
    let guard_dict_shape_layout_fail_version_mismatch =
        GUARD_DICT_SHAPE_LAYOUT_FAIL_VERSION_MISMATCH_COUNT.load(AtomicOrdering::Relaxed);
    let attr_site_name_hit = ATTR_SITE_NAME_CACHE_HIT_COUNT.load(AtomicOrdering::Relaxed);
    let attr_site_name_miss = ATTR_SITE_NAME_CACHE_MISS_COUNT.load(AtomicOrdering::Relaxed);
    let split_ws_ascii = SPLIT_WS_ASCII_FAST_PATH_COUNT.load(AtomicOrdering::Relaxed);
    let split_ws_unicode = SPLIT_WS_UNICODE_PATH_COUNT.load(AtomicOrdering::Relaxed);
    let dict_str_int_prehash_hit = DICT_STR_INT_PREHASH_HIT_COUNT.load(AtomicOrdering::Relaxed);
    let dict_str_int_prehash_miss = DICT_STR_INT_PREHASH_MISS_COUNT.load(AtomicOrdering::Relaxed);
    let dict_str_int_prehash_deopt = DICT_STR_INT_PREHASH_DEOPT_COUNT.load(AtomicOrdering::Relaxed);
    let taq_ingest_calls = TAQ_INGEST_CALL_COUNT.load(AtomicOrdering::Relaxed);
    let taq_ingest_skip_marker = TAQ_INGEST_SKIP_MARKER_COUNT.load(AtomicOrdering::Relaxed);
    let ascii_i64_parse_fail = ASCII_I64_PARSE_FAIL_COUNT.load(AtomicOrdering::Relaxed);
    let alloc_bytes_total = ALLOC_BYTES_TOTAL.load(AtomicOrdering::Relaxed);
    let alloc_bytes_string = ALLOC_BYTES_STRING.load(AtomicOrdering::Relaxed);
    let alloc_bytes_dict = ALLOC_BYTES_DICT.load(AtomicOrdering::Relaxed);
    let alloc_bytes_tuple = ALLOC_BYTES_TUPLE.load(AtomicOrdering::Relaxed);
    let alloc_bytes_list = ALLOC_BYTES_LIST.load(AtomicOrdering::Relaxed);
    let alloc_bytes_exception = ALLOC_BYTES_EXCEPTION.load(AtomicOrdering::Relaxed);
    let aux_class_inline = AUX_CLASS_INLINE_COUNT.load(AtomicOrdering::Relaxed);
    let aux_state_inline = AUX_STATE_INLINE_COUNT.load(AtomicOrdering::Relaxed);
    let aux_sidecar_alloc = AUX_SIDECAR_COUNT.load(AtomicOrdering::Relaxed);
    let aux_sidecar_free = AUX_SIDECAR_FREE_COUNT.load(AtomicOrdering::Relaxed);
    let aux_sidecar_alloc_failure = AUX_SIDECAR_ALLOC_FAILURE_COUNT.load(AtomicOrdering::Relaxed);
    let aux_sidecar_alloc_bytes = AUX_SIDECAR_BYTES.load(AtomicOrdering::Relaxed);
    let aux_sidecar_free_bytes = AUX_SIDECAR_FREE_BYTES.load(AtomicOrdering::Relaxed);
    let gc_track = GC_TRACK_COUNT.load(AtomicOrdering::Relaxed);
    let gc_untrack = GC_UNTRACK_COUNT.load(AtomicOrdering::Relaxed);
    let gc_registry_lock_contention =
        GC_REGISTRY_LOCK_CONTENTION_COUNT.load(AtomicOrdering::Relaxed);
    let gc_registry_lock_wait_ns = GC_REGISTRY_LOCK_WAIT_NS.load(AtomicOrdering::Relaxed);
    let gc_tracked_high_water = GC_TRACKED_HIGH_WATER.load(AtomicOrdering::Relaxed);
    let execution_drop = crate::concurrency::execution::execution_drop_panic_diagnostics();
    // RC drop-insertion substrate (design 20): the leak gauge.
    let deallocs = DEALLOC_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_bytes_total = DEALLOC_BYTES_TOTAL.load(AtomicOrdering::Relaxed);
    let dealloc_objects = DEALLOC_OBJECT_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_bigints = DEALLOC_BIGINT_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_strings = DEALLOC_STRING_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_dicts = DEALLOC_DICT_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_tuples = DEALLOC_TUPLE_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_exceptions = DEALLOC_EXCEPTION_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_bytes_exception = DEALLOC_BYTES_EXCEPTION.load(AtomicOrdering::Relaxed);
    let memory_snapshot = process_memory_snapshot();
    // RC drop-insertion substrate (design 20): the leak report. `live` is the
    // count of objects whose final dec-ref never fired by process exit — the
    // immortal bootstrap roots (module dict, builtin types) legitimately survive
    // (`EXPECTED_LIVE_OBJECTS`), so a healthy program reports `live ≈
    // EXPECTED_LIVE_OBJECTS` and a leaking one reports far more.
    let live_objects = allocs.saturating_sub(deallocs);
    let live_bytes = alloc_bytes_total.saturating_sub(dealloc_bytes_total);
    let live_exceptions = alloc_exceptions.saturating_sub(dealloc_exceptions);
    let live_bytes_exception = alloc_bytes_exception.saturating_sub(dealloc_bytes_exception);
    let aux_sidecar_live = aux_sidecar_alloc.saturating_sub(aux_sidecar_free);
    let aux_sidecar_live_bytes = aux_sidecar_alloc_bytes.saturating_sub(aux_sidecar_free_bytes);
    let gc_tracked_live = GC_TRACKED_LIVE.load(AtomicOrdering::Relaxed);
    if emit_leak_warning && live_objects > crate::EXPECTED_LIVE_OBJECTS {
        crate::diagnostics::emit_line(&format!(
            "[MOLT_PROFILE] LEAK WARNING: {} objects not freed at process exit (expected_live={})",
            live_objects.saturating_sub(crate::EXPECTED_LIVE_OBJECTS),
            crate::EXPECTED_LIVE_OBJECTS,
        ));
    }
    RuntimeProfileSnapshot {
        call_dispatch,
        string_count_cache_hit: cache_hit,
        string_count_cache_miss: cache_miss,
        struct_field_store: struct_stores,
        attr_lookup: attr_lookups,
        handle_resolve: handle_resolves,
        layout_guard,
        layout_guard_fail,
        alloc_count: allocs,
        alloc_object: alloc_objects,
        alloc_exception: alloc_exceptions,
        alloc_dict: alloc_dicts,
        alloc_tuple: alloc_tuples,
        alloc_string: alloc_strings,
        alloc_callargs,
        alloc_bytes_callargs,
        tb_builds,
        tb_frames,
        tb_suppressed,
        async_polls,
        async_pending,
        async_wakeups,
        async_sleep_register: async_sleep_reg,
        alloc_bytes_total,
        alloc_bytes_string,
        alloc_bytes_dict,
        alloc_bytes_tuple,
        alloc_bytes_list,
        alloc_bytes_exception,
        dealloc_count: deallocs,
        dealloc_bytes_total,
        dealloc_object: dealloc_objects,
        dealloc_bigint: dealloc_bigints,
        dealloc_string: dealloc_strings,
        dealloc_dict: dealloc_dicts,
        dealloc_tuple: dealloc_tuples,
        dealloc_exception: dealloc_exceptions,
        dealloc_bytes_exception,
        live_objects,
        live_bytes,
        live_exception: live_exceptions,
        live_bytes_exception,
        expected_live: crate::EXPECTED_LIVE_OBJECTS,
        aux_class_inline_count: aux_class_inline,
        aux_state_inline_count: aux_state_inline,
        aux_sidecar_alloc_count: aux_sidecar_alloc,
        aux_sidecar_free_count: aux_sidecar_free,
        aux_sidecar_live_count: aux_sidecar_live,
        aux_sidecar_alloc_failure_count: aux_sidecar_alloc_failure,
        aux_sidecar_alloc_bytes,
        aux_sidecar_free_bytes,
        aux_sidecar_live_bytes,
        gc_track_count: gc_track,
        gc_untrack_count: gc_untrack,
        gc_tracked_live,
        gc_tracked_high_water,
        gc_registry_lock_contention_count: gc_registry_lock_contention,
        gc_registry_lock_wait_ns,
        gc_snapshot_alloc_failure_count: GC_SNAPSHOT_ALLOC_FAILURE_COUNT
            .load(AtomicOrdering::Relaxed),
        drop_panic_count: execution_drop.total,
        drop_cleanup_panic_count: execution_drop.cleanup,
        drop_detach_panic_count: execution_drop.detach,
        drop_release_panic_count: execution_drop.release,
        shutdown_drop_cext_panic_count: execution_drop.shutdown_cext,
        shutdown_drop_depth_panic_count: execution_drop.shutdown_depth,
        call_bind_ic_hit,
        call_bind_ic_miss,
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
        call_indirect_noncallable: call_indirect_noncallable_deopt,
        invoke_ffi_bridge_capability_denied,
        guard_tag_type_mismatch: guard_tag_type_mismatch_deopt,
        guard_dict_shape_layout_mismatch: guard_dict_shape_layout_mismatch_deopt,
        guard_dict_shape_layout_fail_null_obj,
        guard_dict_shape_layout_fail_non_object,
        guard_dict_shape_layout_fail_class_mismatch,
        guard_dict_shape_layout_fail_non_type_class,
        guard_dict_shape_layout_fail_expected_version_invalid,
        guard_dict_shape_layout_fail_version_mismatch,
    }
    .into_process_payload(runtime_profile_memory_payload(
        memory_snapshot.source,
        memory_snapshot.available_flag(),
        memory_snapshot.current_rss_bytes,
        memory_snapshot.peak_rss_bytes,
    ))
}

pub(crate) fn profile_dump_with_gil(_py: &PyToken<'_>) {
    if !profile_enabled(_py) {
        return;
    }
    let payload = runtime_profile_payload(_py, true);
    crate::diagnostics::emit_line(&format!("molt_profile_json {}", payload));
    maybe_emit_runtime_feedback_file(&payload);
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_profile_dump() {
    crate::with_gil_entry_nopanic!(_py, {
        profile_dump_with_gil(_py);
    })
}

struct RuntimeProfileEpochBaseline {
    generation: u64,
    label: String,
    payload: serde_json::Value,
}

#[derive(Default)]
struct RuntimeProfileEpochState {
    generation: u64,
    active: Option<RuntimeProfileEpochBaseline>,
}

static RUNTIME_PROFILE_EPOCH: OnceLock<Mutex<RuntimeProfileEpochState>> = OnceLock::new();

fn runtime_profile_epoch_state() -> &'static Mutex<RuntimeProfileEpochState> {
    RUNTIME_PROFILE_EPOCH.get_or_init(|| Mutex::new(RuntimeProfileEpochState::default()))
}

pub(crate) fn profile_epoch_clear() {
    if let Some(state) = RUNTIME_PROFILE_EPOCH.get() {
        drop(
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .active
                .take(),
        );
    }
}

fn runtime_profile_is_gauge(section: &str, metric: &str) -> bool {
    runtime_profile_metric_semantic(section, metric) == Some(RuntimeProfileMetricSemantic::Gauge)
}

fn runtime_profile_metric(payload: &serde_json::Value, section: &str, metric: &str) -> Option<u64> {
    payload
        .get(section)
        .and_then(|value| value.get(metric))
        .and_then(serde_json::Value::as_u64)
}

fn runtime_profile_set_settled_memory_snapshot(payload: &mut serde_json::Value) {
    if let Some(memory) = payload.get_mut("memory") {
        let snapshot = process_memory_snapshot();
        *memory = runtime_profile_memory_payload(
            snapshot.source,
            snapshot.available_flag(),
            snapshot.current_rss_bytes,
            snapshot.peak_rss_bytes,
        );
    }
}

fn runtime_profile_signed_delta(end: u64, start: u64) -> i64 {
    if end >= start {
        i64::try_from(end - start).unwrap_or(i64::MAX)
    } else {
        -i64::try_from(start - end).unwrap_or(i64::MAX)
    }
}

fn runtime_profile_epoch_delta_payload(
    baseline: &RuntimeProfileEpochBaseline,
    end: &serde_json::Value,
) -> Result<serde_json::Value, &'static str> {
    let mut counter_sections = serde_json::Map::new();
    let mut counter_regression_sections = serde_json::Map::new();
    let mut gauge_sections = serde_json::Map::new();
    let Some(end_sections) = end.as_object() else {
        return Err("runtime profile payload must be an object");
    };
    // Discover metric sections from the canonical process profile itself. New
    // numeric counter families are therefore included automatically instead of
    // requiring a second section allowlist to remain in sync.
    for (section, end_section) in end_sections {
        if section == "memory" {
            continue;
        }
        let Some(end_metrics) = end_section.as_object() else {
            continue;
        };
        let mut counters = serde_json::Map::new();
        let mut counter_regressions = serde_json::Map::new();
        let mut gauges = serde_json::Map::new();
        for (metric, end_value) in end_metrics {
            let Some(end_value) = end_value.as_u64() else {
                continue;
            };
            let Some(start_value) = runtime_profile_metric(&baseline.payload, section, metric)
            else {
                return Err("runtime profile baseline is missing a canonical counter");
            };
            if runtime_profile_is_gauge(section, metric) {
                gauges.insert(
                    metric.clone(),
                    serde_json::json!({
                        "start": start_value,
                        "end": end_value,
                        "delta": runtime_profile_signed_delta(end_value, start_value),
                    }),
                );
            } else {
                if end_value < start_value {
                    counter_regressions.insert(
                        metric.clone(),
                        serde_json::json!({
                            "start": start_value,
                            "end": end_value,
                        }),
                    );
                    counters.insert(metric.clone(), serde_json::Value::Null);
                } else {
                    counters.insert(
                        metric.clone(),
                        serde_json::Value::from(end_value - start_value),
                    );
                }
            }
        }
        if !counters.is_empty() {
            counter_sections.insert(section.clone(), serde_json::Value::Object(counters));
        }
        if !counter_regressions.is_empty() {
            counter_regression_sections.insert(
                section.clone(),
                serde_json::Value::Object(counter_regressions),
            );
        }
        if !gauges.is_empty() {
            gauge_sections.insert(section.clone(), serde_json::Value::Object(gauges));
        }
    }

    let Some(memory_start) = baseline.payload.get("memory").cloned() else {
        return Err("runtime profile baseline is missing memory telemetry");
    };
    let Some(memory_end) = end.get("memory").cloned() else {
        return Err("runtime profile end snapshot is missing memory telemetry");
    };
    let rss_delta = match (
        runtime_profile_metric(&baseline.payload, "memory", "current_rss_bytes"),
        runtime_profile_metric(end, "memory", "current_rss_bytes"),
    ) {
        (Some(start), Some(end)) => Some(runtime_profile_signed_delta(end, start)),
        _ => None,
    };
    let claimable = counter_regression_sections.is_empty();
    Ok(serde_json::json!({
        "schema_version": PROFILE_EPOCH_SCHEMA_VERSION,
        "kind": PROFILE_EPOCH_KIND,
        "generation": baseline.generation,
        "label": baseline.label.as_str(),
        "claimable": claimable,
        "delta": serde_json::Value::Object(counter_sections),
        "counter_regressions": serde_json::Value::Object(counter_regression_sections),
        "gauges": serde_json::Value::Object(gauge_sections),
        "memory": runtime_profile_epoch_memory_payload(memory_start, memory_end, rss_delta),
    }))
}

/// Reset the process-wide measurement epoch to a fresh baseline without
/// mutating the monotonic lifetime counters used by leak diagnostics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_profile_epoch_reset(label_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !profile_enabled(_py) {
            return MoltObject::from_bool(false).bits();
        }
        let Some(label) = string_obj_to_owned(obj_from_bits(label_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "profile epoch label must be str");
        };
        // Serialize the full reset transaction, not just slot replacement.
        // This makes the sole process-wide epoch deterministic under a future
        // free-threaded caller: reset cannot race dump or publish an older
        // snapshot after a newer reset.
        let mut state = runtime_profile_epoch_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(generation) = state.generation.checked_add(1) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "profile epoch generation exhausted",
            );
        };
        // Drop any abandoned epoch before measuring the new baseline. This
        // makes reset a real reset and prevents stale profiler storage from
        // contaminating the new RSS floor.
        drop(state.active.take());
        state.generation = generation;
        let mut payload = runtime_profile_payload(_py, false);
        // The label and baseline map are measurement infrastructure. Settle
        // them before recording the RSS floor so they do not appear as work.
        runtime_profile_set_settled_memory_snapshot(&mut payload);
        let baseline = RuntimeProfileEpochBaseline {
            generation,
            label,
            payload,
        };
        state.active = Some(baseline);
        MoltObject::from_bool(true).bits()
    })
}

/// End and consume the active measurement epoch, emitting a delta schema on
/// the diagnostics channel. Snapshotting precedes JSON construction, so the
/// reporting path cannot contaminate the measured Molt allocation counters.
#[unsafe(no_mangle)]
pub extern "C" fn molt_profile_epoch_dump() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !profile_enabled(_py) {
            return MoltObject::from_bool(false).bits();
        }
        // Snapshot and consume are one transaction. In particular, a reset
        // cannot replace the baseline between the end snapshot and the take.
        let mut state = runtime_profile_epoch_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let end = runtime_profile_payload(_py, false);
        let Some(baseline) = state.active.take() else {
            return raise_exception::<u64>(_py, "RuntimeError", "profile epoch is not active");
        };
        drop(state);
        let payload = match runtime_profile_epoch_delta_payload(&baseline, &end) {
            Ok(payload) => payload,
            Err(message) => return raise_exception::<u64>(_py, "RuntimeError", message),
        };
        crate::diagnostics::emit_line(&format!("molt_profile_epoch_json {payload}"));
        MoltObject::from_bool(true).bits()
    })
}

/// RC drop-insertion substrate (design 20): the `MOLT_ASSERT_NO_LEAK` gate.
///
/// When `MOLT_ASSERT_NO_LEAK` is set, the alloc/dealloc counters are
/// force-enabled (see `metrics::profile_env_enabled`). At process exit — BEFORE
/// the immortal roots are torn down — this asserts that no more than
/// `EXPECTED_LIVE_OBJECTS` objects survived. A per-iteration leak grows `live`
/// without bound, so it fails decisively; a healthy program reports `live` at or
/// near the immortal-bootstrap floor. On failure it prints the per-type
/// breakdown and aborts with a non-zero exit (`process::exit(137)` to mirror the
/// `safe_run.py` RSS-cap convention the harness keys on).
pub(crate) fn assert_no_leak_at_exit(_py: &PyToken<'_>) {
    if !crate::leak_assertion_enabled() {
        return;
    }
    // Pre-teardown RUNAWAY / peak-working-set guard. Runs BEFORE teardown, while
    // the program's full live working set is still resident, so `live` here is a
    // reachable high-water-mark (NOT a leak — teardown reclaims every reachable
    // ACYCLIC graph, including user __main__ globals via modules_clear_runtime_state).
    // This is a coarse OOM/runaway canary at EXPECTED_LIVE_OBJECTS. Genuine leak
    // detection is the SEPARATE post-teardown gauge `assert_no_true_leak_post_teardown`.
    let allocs = ALLOC_COUNT.load(AtomicOrdering::Relaxed);
    let deallocs = DEALLOC_COUNT.load(AtomicOrdering::Relaxed);
    let live = allocs.saturating_sub(deallocs);
    if live <= crate::EXPECTED_LIVE_OBJECTS {
        return;
    }
    emit_leak_breakdown(
        "MOLT_ASSERT_NO_LEAK",
        live,
        crate::EXPECTED_LIVE_OBJECTS,
        allocs,
        deallocs,
    );
    std::process::exit(137);
}

/// Post-teardown TRUE-LEAK gauge (ownership_lattice_phase0.md §2.4).
///
/// Runs AFTER `runtime_teardown_for_process_exit` has reclaimed every reachable
/// acyclic graph — including user `__main__` globals via
/// `modules_clear_runtime_state` — so the only survivors now are the immortal
/// heap floor plus genuine leaks. molt is reference-counted with NO cycle
/// collector (`formal/quint/molt_gc_safety.qnt` scopes its no-leak proof to
/// ACYCLIC graphs), so the canonical leak class is an *unreachable reference
/// cycle*: RC pins it at refcount ≥ 1 forever and nothing reclaims it (CPython's
/// cyclic gc would). In exact mode (`MOLT_LEAK_TOLERANCE` set) we gate
/// `live <= floor + tolerance`, catching a cycle leak that the coarse
/// pre-teardown ceiling launders. No-op in the default profile (which relies on
/// the pre-teardown runaway ceiling); this never changes default-profile behavior.
pub(crate) fn assert_no_true_leak_post_teardown(_py: &PyToken<'_>) {
    if !crate::leak_assertion_enabled() {
        return;
    }
    let (floor, tol) = match (
        crate::state::metrics::live_floor(),
        crate::state::metrics::leak_exact_tolerance(),
    ) {
        (Some(floor), Some(tol)) => (floor, tol),
        _ => return,
    };
    let allocs = ALLOC_COUNT.load(AtomicOrdering::Relaxed);
    let deallocs = DEALLOC_COUNT.load(AtomicOrdering::Relaxed);
    let live = allocs.saturating_sub(deallocs);
    let limit = floor.saturating_add(tol);
    if live <= limit {
        return;
    }
    emit_leak_breakdown("MOLT_ASSERT_NO_TRUE_LEAK", live, limit, allocs, deallocs);
    std::process::exit(137);
}

/// Shared per-type leak diagnostic emission — one authority for both the
/// pre-teardown runaway ceiling and the post-teardown true-leak gauge, so the two
/// gates never drift in their reporting format.
fn emit_leak_breakdown(label: &str, live: u64, limit: u64, allocs: u64, deallocs: u64) {
    let dealloc_objects = DEALLOC_OBJECT_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_bigints = DEALLOC_BIGINT_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_strings = DEALLOC_STRING_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_dicts = DEALLOC_DICT_COUNT.load(AtomicOrdering::Relaxed);
    let dealloc_tuples = DEALLOC_TUPLE_COUNT.load(AtomicOrdering::Relaxed);
    let alloc_objects = ALLOC_OBJECT_COUNT.load(AtomicOrdering::Relaxed);
    let alloc_strings = ALLOC_STRING_COUNT.load(AtomicOrdering::Relaxed);
    let alloc_dicts = ALLOC_DICT_COUNT.load(AtomicOrdering::Relaxed);
    let alloc_tuples = ALLOC_TUPLE_COUNT.load(AtomicOrdering::Relaxed);
    crate::diagnostics::emit_line(&format!(
        "[{}] FAIL: live_objects={} exceeds limit={} (floor={:?} alloc={} dealloc={})",
        label,
        live,
        limit,
        crate::state::metrics::live_floor(),
        allocs,
        deallocs,
    ));
    crate::diagnostics::emit_line(&format!(
        "[{}] per-type (alloc/dealloc/live): \
         object={}/{}/{} string={}/{}/{} dict={}/{}/{} tuple={}/{}/{} bigint=?/{}/?",
        label,
        alloc_objects,
        dealloc_objects,
        alloc_objects.saturating_sub(dealloc_objects),
        alloc_strings,
        dealloc_strings,
        alloc_strings.saturating_sub(dealloc_strings),
        alloc_dicts,
        dealloc_dicts,
        alloc_dicts.saturating_sub(dealloc_dicts),
        alloc_tuples,
        dealloc_tuples,
        alloc_tuples.saturating_sub(dealloc_tuples),
        dealloc_bigints,
    ));
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
}

// ---------------------------------------------------------------------------
// SIMD-accelerated float sum: SSE2 (2×f64), AVX2 (4×f64), NEON (2×f64)
// ---------------------------------------------------------------------------

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
    unsafe {
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
}

#[cfg(target_arch = "x86_64")]
unsafe fn find_first_mismatch_avx2(lhs: &[u64], rhs: &[u64], len: usize) -> usize {
    unsafe {
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
pub(crate) unsafe fn sum_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
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
}

#[cfg(target_arch = "x86_64")]
pub(crate) unsafe fn sum_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
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
pub(crate) unsafe fn prod_ints_unboxed_avx2_trivial(elems: &[i64]) -> Option<i64> {
    unsafe {
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
}

#[cfg(target_arch = "x86_64")]
pub(crate) unsafe fn min_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
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
}

#[cfg(target_arch = "x86_64")]
pub(crate) unsafe fn min_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
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
pub(crate) unsafe fn max_ints_simd_x86_64(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
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
}

#[cfg(target_arch = "x86_64")]
pub(crate) unsafe fn max_ints_simd_x86_64_avx2(elems: &[u64], acc: i64) -> Option<i64> {
    unsafe {
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
pub(crate) unsafe fn sum_ints_trusted_simd_x86_64(elems: &[u64], acc: i64) -> i64 {
    unsafe {
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
}

#[cfg(target_arch = "x86_64")]
pub(crate) unsafe fn sum_ints_trusted_simd_x86_64_avx2(elems: &[u64], acc: i64) -> i64 {
    unsafe {
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
        let Some(set_ptr) = obj_from_bits(set_bits).as_ptr() else {
            dec_ref_bits(_py, iter_bits);
            return None;
        };
        let done_true = MoltObject::from_bool(true).bits();
        let done_false = MoltObject::from_bool(false).bits();
        loop {
            let mut val_bits = 0;
            let done_bits = crate::object::ops_iter::molt_iter_next_unboxed(
                iter_bits,
                (&mut val_bits as *mut u64) as u64,
            );
            if done_bits == MoltObject::none().bits() || exception_pending(_py) {
                dec_ref_bits(_py, iter_bits);
                dec_ref_bits(_py, set_bits);
                return None;
            }
            if done_bits == done_true {
                break;
            }
            if done_bits != done_false {
                dec_ref_bits(_py, iter_bits);
                dec_ref_bits(_py, set_bits);
                return None;
            }
            set_add_in_place(_py, set_ptr, val_bits, HashContext::SetElement);
            dec_ref_bits(_py, val_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, iter_bits);
                dec_ref_bits(_py, set_bits);
                return None;
            }
        }
        dec_ref_bits(_py, iter_bits);
        Some(set_bits)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref_obj(bits: u64) {
    // Fast path: skip GIL for non-pointer values (ints, floats, bools, none).
    let obj = obj_from_bits(bits);
    let Some(ptr) = obj.as_ptr() else {
        return;
    };
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::object::inc_ref_ptr(_py, ptr) };
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref_obj(bits: u64) {
    // Fast path: skip GIL for non-pointer values (ints, floats, bools, none).
    let obj = obj_from_bits(bits);
    if !obj.is_ptr() {
        return;
    }
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *const MoltHeader;
                let type_id = (*header_ptr).type_id;
                if !crate::object::is_valid_heap_type_id(type_id) {
                    if let Some((file, line, frame, col, end_col)) =
                        crate::builtins::frames::frame_stack_top_info(_py)
                    {
                        eprintln!(
                            "molt fatal: invalid object header before dec_ref \
                             ptr=0x{:x} bits=0x{:x} type_id={} frame={} file={} line={} col={} end_col={}",
                            ptr as usize, bits, type_id, frame, file, line, col, end_col
                        );
                    } else {
                        eprintln!(
                            "molt fatal: invalid object header before dec_ref ptr=0x{:x} bits=0x{:x} type_id={}",
                            ptr as usize, bits, type_id
                        );
                    }
                    if std::env::var("MOLT_TRACE_INVALID_DECREF").as_deref() == Ok("1") {
                        let bt = std::backtrace::Backtrace::force_capture();
                        eprintln!("molt invalid dec_ref backtrace:\n{bt}");
                    }
                    std::process::abort();
                }
                crate::object::dec_ref_ptr_validated(_py, ptr, type_id);
            };
        }
    })
}

/// Batched `inc_ref`: apply one checked typed transition for `count` retained
/// references. Returns the input bits unchanged (convenience for chaining).
#[unsafe(no_mangle)]
pub extern "C" fn molt_inc_ref_n(bits: u64, count: u32) -> u64 {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return bits;
    };
    if count == 0 {
        return bits;
    }
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { crate::object::inc_ref_n_ptr(_py, ptr, count) };
    });
    bits
}

/// Batched `dec_ref`: apply the typed release transition `count` times. Each
/// release is separate because any one of them may cross the terminal edge.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dec_ref_n(bits: u64, count: u32) {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return;
    };
    if count == 0 {
        return;
    }
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let header_ptr = ptr.sub(std::mem::size_of::<MoltHeader>()) as *const MoltHeader;
            let type_id = (*header_ptr).type_id;
            if !crate::object::is_valid_heap_type_id(type_id) {
                eprintln!(
                    "molt fatal: invalid object header before batched dec_ref ptr=0x{:x} bits=0x{:x} type_id={}",
                    ptr as usize, bits, type_id
                );
                std::process::abort();
            }
            for _ in 0..count {
                crate::object::dec_ref_ptr_validated(_py, ptr, type_id);
            }
        }
    })
}

fn unpack_too_many_message(_py: &PyToken<'_>, expected: usize, actual: usize) -> String {
    if crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
        format!(
            "too many values to unpack (expected {}, got {})",
            expected, actual
        )
    } else {
        format!("too many values to unpack (expected {})", expected)
    }
}

fn unpack_non_iterable_message(_py: &PyToken<'_>, seq_bits: u64) -> String {
    let type_name = class_name_for_error(type_of_bits(_py, seq_bits));
    format!("cannot unpack non-iterable {type_name} object")
}

/// Outlined transactional sequence-unpacking authority. It initializes every
/// output to `None`, validates exact arity, and publishes one independent owned
/// reference per output only on success. Every failure rolls back all published
/// references and leaves the entire output range as `None`.
///
/// Returns 0 on success. Errors are raised through the normal exception-pending
/// mechanism and return `MoltObject::none().bits()`.
///
/// # Safety
///
/// A nonzero `output_ptr_bits` must encode writable storage for at least
/// `expected_count` `u64` elements. A null pointer with a nonzero count is the
/// allocation-failure signal used by pointer-width backends and raises
/// `MemoryError` before any source iteration.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_unpack_sequence(
    seq_bits: u64,
    expected_count: u64,
    output_ptr_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Ok(expected) = usize::try_from(expected_count) else {
            return raise_exception::<u64>(
                _py,
                "MemoryError",
                "sequence unpack target count exceeds the active address space",
            );
        };
        let output_ptr = if expected == 0 {
            std::ptr::null_mut()
        } else {
            let Some(output_ptr) = crate::provenance::abi::mut_ptr::<u64>(output_ptr_bits) else {
                return raise_exception::<u64>(
                    _py,
                    "MemoryError",
                    "sequence unpack result address exceeds the active address space",
                );
            };
            output_ptr
        };
        if expected != 0 && output_ptr.is_null() {
            return raise_exception::<u64>(
                _py,
                "MemoryError",
                "sequence unpack result allocation failed",
            );
        }
        let Some(out_slice) =
            (unsafe { crate::provenance::abi::slice_mut(output_ptr, expected_count) })
        else {
            return raise_exception::<u64>(
                _py,
                "MemoryError",
                "sequence unpack result range is invalid for the active target",
            );
        };
        let no_value = MoltObject::none().bits();
        out_slice.fill(no_value);
        if exception_pending(_py) {
            return no_value;
        }

        let obj = obj_from_bits(seq_bits);
        let Some(ptr) = obj.as_ptr() else {
            let msg = unpack_non_iterable_message(_py, seq_bits);
            raise_exception::<u64>(_py, "TypeError", &msg);
            return MoltObject::none().bits();
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_LIST_BOOL {
                let elems = crate::object::layout::list_bool_vec_ref(ptr);
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
                    let msg = unpack_too_many_message(_py, expected, actual);
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    return MoltObject::none().bits();
                }
                for (i, &raw) in elems.iter().enumerate().take(expected) {
                    out_slice[i] = MoltObject::from_bool(raw != 0).bits();
                }
            } else if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                // Exact built-in sequences need no temporary snapshot. The
                // scoped read authority holds the list mutation lock (or a
                // zero-lock immutable tuple view), validates arity, and mints
                // each output owner before releasing that view. This removes
                // the former O(n) allocation and its duplicate retain/release
                // traffic while remaining free-threaded safe.
                let actual = crate::object::seq_access::with_borrowed(ptr, |elems| {
                    let actual = elems.len();
                    if actual == expected {
                        for (slot, &bits) in out_slice.iter_mut().zip(elems.iter()) {
                            inc_ref_bits(_py, bits);
                            *slot = bits;
                        }
                    }
                    actual
                });
                if actual < expected {
                    let msg = format!(
                        "not enough values to unpack (expected {}, got {})",
                        expected, actual
                    );
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    return MoltObject::none().bits();
                }
                if actual > expected {
                    let msg = unpack_too_many_message(_py, expected, actual);
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    return MoltObject::none().bits();
                }
            } else {
                // Generic iterable: materialize via iter/next.
                let iter_bits = molt_iter(seq_bits);
                if obj_from_bits(iter_bits).is_none() {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let msg = unpack_non_iterable_message(_py, seq_bits);
                    raise_exception::<u64>(_py, "TypeError", &msg);
                    return MoltObject::none().bits();
                }
                let mut count = 0usize;
                loop {
                    let mut value_bits = no_value;
                    let done_bits = crate::molt_iter_next_unboxed(
                        iter_bits,
                        crate::provenance::abi::expose_address(&raw mut value_bits),
                    );
                    if exception_pending(_py) {
                        for slot in out_slice.iter_mut().take(count) {
                            dec_ref_bits(_py, *slot);
                            *slot = no_value;
                        }
                        dec_ref_bits(_py, iter_bits);
                        return no_value;
                    }
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        break;
                    }
                    if count == expected {
                        dec_ref_bits(_py, value_bits);
                        for slot in out_slice.iter_mut() {
                            dec_ref_bits(_py, *slot);
                            *slot = no_value;
                        }
                        dec_ref_bits(_py, iter_bits);
                        let msg = format!("too many values to unpack (expected {})", expected);
                        raise_exception::<u64>(_py, "ValueError", &msg);
                        return no_value;
                    }
                    out_slice[count] = value_bits;
                    count += 1;
                }
                dec_ref_bits(_py, iter_bits);
                if count < expected {
                    let msg = format!(
                        "not enough values to unpack (expected {}, got {})",
                        expected, count
                    );
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    for slot in out_slice.iter_mut().take(count) {
                        dec_ref_bits(_py, *slot);
                        *slot = no_value;
                    }
                    return no_value;
                }
            }
        }
        0u64
    })
}

unsafe fn dict_subclass_storage_bits(_py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    unsafe {
        let debug = debug_dict_subclass_enabled();
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
        if !object_replace_class_edge(_py, ptr, 0, ClassEdgeOwnership::Owned) {
            return;
        }
        // Detach scalar metadata before intern/name pools are drained. Class
        // destruction happens much later, after those authorities are gone.
        class_set_name_bits(_py, ptr, none_bits);
        class_set_qualname_bits(_py, ptr, 0u64);
        class_set_annotations_bits(_py, ptr, 0u64);
        class_set_annotate_bits(_py, ptr, 0u64);
        let dict_bits = class_dict_bits(ptr);
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && object_type_id(dict_ptr) == TYPE_ID_DICT
        {
            dict_clear_in_place_shutdown(_py, dict_ptr);
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
    if let Some(f) = as_float_extended(obj) {
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
            if type_id == TYPE_ID_LIST
                || type_id == TYPE_ID_LIST_INT
                || type_id == TYPE_ID_LIST_BOOL
            {
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

fn union_type_display_name(_py: &PyToken<'_>) -> &'static str {
    if runtime_target_minor(_py) >= 14 {
        "types.Union"
    } else {
        "types.UnionType"
    }
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
                TYPE_ID_FLOAT => Cow::Borrowed("float"),
                TYPE_ID_STRING => Cow::Borrowed("str"),
                TYPE_ID_BYTES => Cow::Borrowed("bytes"),
                TYPE_ID_BYTEARRAY => Cow::Borrowed("bytearray"),
                TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL => Cow::Borrowed("list"),
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
                    // First try the dataclass descriptor's name field (always
                    // set at allocation time, unlike class_bits which can be
                    // stale for inline-created instances).
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    if !desc_ptr.is_null() {
                        let name = &(*desc_ptr).name;
                        if !name.is_empty() {
                            return Cow::Owned(name.clone());
                        }
                    }
                    // Fallback to class_bits → class_name_for_error.
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
                TYPE_ID_UNION => Cow::Borrowed(union_type_display_name(_py)),
                TYPE_ID_GENERATOR => Cow::Borrowed("generator"),
                TYPE_ID_ASYNC_GENERATOR => Cow::Borrowed("async_generator"),
                TYPE_ID_ENUMERATE => Cow::Borrowed("enumerate"),
                TYPE_ID_ITER => Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits()))),
                TYPE_ID_CALL_ITER => Cow::Borrowed("callable_iterator"),
                TYPE_ID_REVERSED => Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits()))),
                TYPE_ID_ZIP => Cow::Borrowed("zip"),
                TYPE_ID_MAP => Cow::Borrowed("map"),
                TYPE_ID_FILTER => Cow::Borrowed("filter"),
                TYPE_ID_NATIVE_HANDLE => Cow::Borrowed("native_handle"),
                crate::TYPE_ID_FOREIGN => {
                    // Honest type name: the wrapped C object's `tp_name`.
                    let c_ptr = crate::object::foreign::foreign_ptr_from_obj(ptr);
                    let name_ptr = molt_cpython_abi::bridge::molt_foreign_type_name(c_ptr);
                    if name_ptr.is_null() {
                        Cow::Borrowed("foreign_object")
                    } else {
                        Cow::Owned(
                            std::ffi::CStr::from_ptr(name_ptr)
                                .to_string_lossy()
                                .into_owned(),
                        )
                    }
                }
                TYPE_ID_CLASSMETHOD => Cow::Borrowed("classmethod"),
                TYPE_ID_STATICMETHOD => Cow::Borrowed("staticmethod"),
                TYPE_ID_PROPERTY => Cow::Borrowed("property"),
                TYPE_ID_SUPER => Cow::Borrowed("super"),
                TYPE_ID_OBJECT => Cow::Owned(class_name_for_error(type_of_bits(_py, obj.bits()))),
                _ => {
                    // For unknown type_ids (custom class instances that aren't
                    // TYPE_ID_OBJECT), try to resolve the class name via
                    // type_of_bits. Falls back to "object" if lookup fails.
                    let class_bits = type_of_bits(_py, obj.bits());
                    let name = class_name_for_error(class_bits);
                    if name != "<class>" {
                        Cow::Owned(name)
                    } else {
                        Cow::Borrowed("object")
                    }
                }
            };
        }
    }
    Cow::Borrowed("object")
}

/// Outlined class definition helper.  Replaces the multi-op inline sequence
/// (`class_new` + `class_set_base` + N x `set_attr_generic_obj` +
/// `class_apply_set_name` + `__init_subclass__` dispatch +
/// `class_set_layout_version`) with a single runtime call.
///
/// # Safety
///
/// `bases_ptr_bits` must encode `nbases` entries when `nbases > 0`, and
/// `attrs_ptr_bits` must encode `nattrs * 2` entries when `nattrs > 0`.
unsafe fn install_guarded_class_namespace(
    _py: &PyToken<'_>,
    class_bits: u64,
    name_bits: u64,
    attrs: &[u64],
    layout_size: i64,
) -> bool {
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        return false;
    };
    let dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return false;
    };
    for pair in attrs.chunks_exact(2) {
        unsafe { crate::dict_set_in_place(_py, dict_ptr, pair[0], pair[1]) };
        if exception_pending(_py) {
            return false;
        }
    }
    let size_obj = MoltObject::from_int(layout_size).bits();
    let layout_attr = crate::intern_static_name(
        _py,
        &crate::runtime_state(_py).interned.molt_layout_size,
        b"__molt_layout_size__",
    );
    unsafe { crate::dict_set_in_place(_py, dict_ptr, layout_attr, size_obj) };
    crate::dec_ref_bits(_py, size_obj);
    !exception_pending(_py)
        && unsafe {
            crate::builtins::types::class_finalize_namespace_metadata(_py, class_ptr, name_bits)
        }
}

/// Build and publish a class from raw base and namespace arrays.
///
/// # Safety
///
/// `bases_ptr_bits` must encode `nbases` entries when `nbases > 0`, and
/// `attrs_ptr_bits` must encode `nattrs * 2` entries when `nattrs > 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_guarded_class_def(
    name_bits: u64,
    bases_ptr_bits: u64,
    nbases: u64,
    attrs_ptr_bits: u64,
    nattrs: u64,
    layout_size: i64,
    layout_version: i64,
    flags: u64,
) -> u64 {
    use crate::builtins::types::{
        molt_class_apply_set_name, molt_class_new, molt_class_set_base,
        molt_class_set_layout_version,
    };
    use molt_obj_model::MoltObject;

    let none = MoltObject::none().bits();
    let Ok(nb) = usize::try_from(nbases) else {
        return crate::with_gil_entry_nopanic!(_py, {
            raise_exception::<u64>(_py, "MemoryError", "class base count is too large")
        });
    };
    let Ok(na) = usize::try_from(nattrs) else {
        return crate::with_gil_entry_nopanic!(_py, {
            raise_exception::<u64>(_py, "MemoryError", "class attribute count is too large")
        });
    };
    let Some(attr_slots) = na.checked_mul(2) else {
        return crate::with_gil_entry_nopanic!(_py, {
            raise_exception::<u64>(_py, "MemoryError", "class attribute storage is too large")
        });
    };
    let bases_ptr = if nb == 0 {
        std::ptr::null()
    } else {
        let Some(ptr) = crate::provenance::abi::const_ptr::<u64>(bases_ptr_bits) else {
            return crate::with_gil_entry_nopanic!(_py, {
                raise_exception::<u64>(
                    _py,
                    "MemoryError",
                    "class base address exceeds the active address space",
                )
            });
        };
        if ptr.is_null() {
            return crate::with_gil_entry_nopanic!(_py, {
                raise_exception::<u64>(_py, "RuntimeError", "class base pointer is null")
            });
        }
        ptr
    };
    let attrs_ptr = if attr_slots == 0 {
        std::ptr::null()
    } else {
        let Some(ptr) = crate::provenance::abi::const_ptr::<u64>(attrs_ptr_bits) else {
            return crate::with_gil_entry_nopanic!(_py, {
                raise_exception::<u64>(
                    _py,
                    "MemoryError",
                    "class attribute address exceeds the active address space",
                )
            });
        };
        if ptr.is_null() {
            return crate::with_gil_entry_nopanic!(_py, {
                raise_exception::<u64>(_py, "RuntimeError", "class attribute pointer is null")
            });
        }
        ptr
    };
    let Some(bases) = (unsafe { crate::provenance::abi::slice(bases_ptr, nbases) }) else {
        return crate::with_gil_entry_nopanic!(_py, {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "class base range is invalid for the active target",
            )
        });
    };
    let attr_slots_bits =
        u64::try_from(attr_slots).expect("usize is no wider than Molt's u64 runtime ABI word");
    let Some(attrs) = (unsafe { crate::provenance::abi::slice(attrs_ptr, attr_slots_bits) }) else {
        return crate::with_gil_entry_nopanic!(_py, {
            raise_exception::<u64>(
                _py,
                "RuntimeError",
                "class attribute range is invalid for the active target",
            )
        });
    };
    let bases_vec = bases.to_vec();
    let attrs_vec = attrs.to_vec();
    let debug_class_def = std::env::var("MOLT_DEBUG_CLASS_DEF").as_deref() == Ok("1");
    let class_bits = molt_class_new(name_bits);
    if class_bits == none {
        return class_bits;
    }
    if (flags & 1) != 0 && nb > 0 {
        if nb == 1 {
            molt_class_set_base(class_bits, bases_vec[0]);
        } else {
            crate::with_gil_entry_nopanic!(_py, {
                let tuple_ptr = crate::object::builders::alloc_tuple(_py, &bases_vec);
                if !tuple_ptr.is_null() {
                    let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    molt_class_set_base(class_bits, tuple_bits);
                    crate::dec_ref_bits(_py, tuple_bits);
                }
            });
        }
    }

    let namespace_ok = crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            install_guarded_class_namespace(_py, class_bits, name_bits, &attrs_vec, layout_size)
        }
    });
    if !namespace_ok {
        crate::with_gil_entry_nopanic!(_py, { crate::dec_ref_bits(_py, class_bits) });
        return MoltObject::none().bits();
    }

    if debug_class_def {
        eprintln!("molt class_def before apply_set_name");
    }
    molt_class_apply_set_name(class_bits);
    if debug_class_def {
        eprintln!("molt class_def after apply_set_name");
    }
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
            unsafe {
                crate::object::class_finish_definition(_py, class_ptr);
            }
        }
    });

    if nb > 0 {
        let init_subclass_ok = crate::with_gil_entry_nopanic!(_py, {
            if debug_class_def {
                let class_name = string_obj_to_owned(obj_from_bits(name_bits))
                    .unwrap_or_else(|| "<class>".to_string());
                eprintln!(
                    "molt class_def init_subclass name={} nbases={} nattrs={} flags={}",
                    class_name, nb, na, flags
                );
                for pair in attrs_vec.chunks_exact(2) {
                    eprintln!(
                        "molt class_def attr key_type={} val_type={} val_bits=0x{:x}",
                        type_name(_py, obj_from_bits(pair[0])),
                        type_name(_py, obj_from_bits(pair[1])),
                        pair[1]
                    );
                }
            }
            if debug_class_def {
                eprintln!("molt class_def before init_subclass dispatch");
            }
            let ok = unsafe {
                crate::call::bind::dispatch_init_subclass_hooks(
                    _py,
                    &bases_vec,
                    class_bits,
                    &[],
                    &[],
                )
            };
            if debug_class_def {
                eprintln!("molt class_def after init_subclass dispatch");
            }
            ok
        });
        if !init_subclass_ok {
            return MoltObject::none().bits();
        }
    }

    let version_obj = MoltObject::from_int(layout_version).bits();
    molt_class_set_layout_version(class_bits, version_obj);
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let Some(n) = crate::provenance::abi::address(n_parts) else {
            return raise_exception::<_>(
                _py,
                "MemoryError",
                "f-string part count exceeds the active address space",
            );
        };
        if n == 0 {
            let ptr = alloc_string(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        }
        if parts_ptr.is_null() {
            return raise_exception::<_>(_py, "RuntimeError", "f-string parts pointer is null");
        }
        let Some(pair_slots) = n.checked_mul(2) else {
            return raise_exception::<_>(_py, "MemoryError", "f-string part storage overflow");
        };
        let pair_slots_bits =
            u64::try_from(pair_slots).expect("usize is no wider than Molt's u64 runtime ABI word");
        let Some(raw_parts) =
            (unsafe { crate::provenance::abi::slice(parts_ptr, pair_slots_bits) })
        else {
            return raise_exception::<_>(_py, "RuntimeError", "f-string parts pointer is invalid");
        };

        // Collect string parts — resolve values via str() as needed.
        let mut parts: Vec<(u64, bool)> = Vec::new(); // (string_bits, owned)
        if parts.try_reserve_exact(n).is_err() {
            return raise_exception::<_>(
                _py,
                "MemoryError",
                "f-string part storage allocation failed",
            );
        }
        let mut total_len: usize = 0;

        for pair in raw_parts.chunks_exact(2) {
            let is_literal = pair[0];
            let value_bits = pair[1];

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
                        let Some(next_len) = total_len.checked_add(string_len(ptr)) else {
                            for &(bits, owned) in &parts {
                                if owned {
                                    dec_ref_bits(_py, bits);
                                }
                            }
                            if string_bits.1 {
                                dec_ref_bits(_py, string_bits.0);
                            }
                            return raise_exception::<_>(
                                _py,
                                "MemoryError",
                                "f-string result length overflow",
                            );
                        };
                        total_len = next_len;
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
                if let Some(ptr) = obj_from_bits(bits).as_ptr()
                    && object_type_id(ptr) == TYPE_ID_STRING
                {
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
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(list_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_LIST {
                return 0;
            }
            let Some(idx) = index_i64_integral_bits(index_bits) else {
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
            // Borrowed: do NOT inc_ref. The runtime GIL keeps the list and
            // element alive after the scoped backing read returns.
            crate::object::seq_access::item(ptr, i as usize).unwrap_or(0)
        }
    })
}

/// Returns a tuple element WITHOUT incrementing the refcount.
/// The tuple holds the element alive. This mirrors CPython's
/// `PyTuple_GetItem()` borrowed-reference semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_getitem_borrowed(tuple_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(tuple_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_TUPLE {
                return 0;
            }
            let Some(idx) = index_i64_integral_bits(index_bits) else {
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
            // Borrowed: do NOT inc_ref.
            crate::object::seq_access::with_immutable_tuple_slice(ptr, |items| {
                items.get(i as usize).copied().unwrap_or(0)
            })
            .unwrap_or(0)
        }
    })
}
