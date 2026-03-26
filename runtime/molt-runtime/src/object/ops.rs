// Re-export iter impl functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_iter::{
    enumerate_new_impl, filter_new_impl, map_new_impl, reversed_new_impl, zip_new_impl,
};

// Re-export compare functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_compare::{
    CompareBoolOutcome, CompareOp, CompareOutcome, compare_builtin_bool, compare_objects,
    compare_type_error, rich_compare_bool,
};

// Re-export format functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_format::{
    FormatError, FormatSpec, decode_string_list, decode_value_list, format_float_with_spec,
    format_obj, format_obj_str, format_with_spec, parse_format_spec, string_obj_to_owned,
};

// Re-export hash functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_hash::{
    HashSecret, ensure_hashable, fatal_hash_seed, hash_bits, hash_bits_signed, hash_int,
    hash_pointer, hash_slice_bits, hash_string_bytes,
};

// Re-export encoding functions for backward compatibility with crate::object::ops::* paths
pub(crate) use crate::object::ops_encoding::{
    DecodeTextError, EncodeError, EncodingKind, decode_bytes_text, decode_error_byte,
    decode_error_range, encode_error_reason, encode_string_with_errors, encoding_kind_name,
    is_surrogate, normalize_encoding, unicode_escape,
};

use crate::object::accessors::object_field_init_ptr_raw;
use crate::object::layout::{range_start_bits, range_step_bits, range_stop_bits};
use crate::object::ops_bytes::{
    BytesCtorKind, bytes_ascii_space, bytes_hex_from_bits, bytes_item_to_u8,
    collect_bytearray_assign_bytes,
};
use crate::randomness::{fill_os_random, os_random_supported};
use crate::state::runtime_state::PythonVersionInfo;
use crate::*;
use memchr::{memchr, memmem};
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
#[cfg(not(target_arch = "wasm32"))]
use std::ffi::CString;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Mutex, OnceLock};

use super::ops_string::{
    push_wtf8_codepoint, utf8_char_to_byte_index_cached, wtf8_codepoint_at, wtf8_from_bytes,
    wtf8_has_surrogates,
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

pub(crate) fn slice_match(slice: &[u8], needle: &[u8], start_raw: i64, total: i64, suffix: bool) -> bool {
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

pub(super) fn alloc_range_from_bigints(_py: &PyToken<'_>, start: BigInt, stop: BigInt, step: BigInt) -> u64 {
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


#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = alloc_slice_obj(_py, start_bits, stop_bits, step_bits);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn slice_indices_adjust(mut idx: BigInt, len: &BigInt, lower: &BigInt, upper: &BigInt) -> BigInt {
    if idx.is_negative() {
        idx += len;
    }
    if idx < *lower {
        return lower.clone();
    }
    if idx > *upper {
        return upper.clone();
    }
    idx
}

fn slice_reduce_tuple(_py: &PyToken<'_>, slice_ptr: *mut u8) -> u64 {
    unsafe {
        let start_bits = slice_start_bits(slice_ptr);
        let stop_bits = slice_stop_bits(slice_ptr);
        let step_bits = slice_step_bits(slice_ptr);
        let args_ptr = alloc_tuple(_py, &[start_bits, stop_bits, step_bits]);
        if args_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let class_bits = builtin_classes(_py).slice;
        let res_ptr = alloc_tuple(_py, &[class_bits, args_bits]);
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(res_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_indices(slice_bits: u64, length_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            let msg = "slice indices must be integers or None or have an __index__ method";
            let Some(len) = index_bigint_from_obj(_py, length_bits, msg) else {
                return MoltObject::none().bits();
            };
            if len.is_negative() {
                return raise_exception::<_>(_py, "ValueError", "length should not be negative");
            }
            let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
            let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
            let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
            let step = if step_obj.is_none() {
                BigInt::from(1)
            } else {
                let Some(step_val) = index_bigint_from_obj(_py, step_obj.bits(), msg) else {
                    return MoltObject::none().bits();
                };
                step_val
            };
            if step.is_zero() {
                return raise_exception::<_>(_py, "ValueError", "slice step cannot be zero");
            }
            let step_neg = step.is_negative();
            let lower = if step_neg {
                BigInt::from(-1)
            } else {
                BigInt::from(0)
            };
            let upper = if step_neg { &len - 1 } else { len.clone() };
            let start = if start_obj.is_none() {
                if step_neg {
                    upper.clone()
                } else {
                    lower.clone()
                }
            } else {
                let Some(idx) = index_bigint_from_obj(_py, start_obj.bits(), msg) else {
                    return MoltObject::none().bits();
                };
                slice_indices_adjust(idx, &len, &lower, &upper)
            };
            let stop = if stop_obj.is_none() {
                if step_neg {
                    lower.clone()
                } else {
                    upper.clone()
                }
            } else {
                let Some(idx) = index_bigint_from_obj(_py, stop_obj.bits(), msg) else {
                    return MoltObject::none().bits();
                };
                slice_indices_adjust(idx, &len, &lower, &upper)
            };
            let start_bits = int_bits_from_bigint(_py, start);
            let stop_bits = int_bits_from_bigint(_py, stop);
            let step_bits = int_bits_from_bigint(_py, step);
            let tuple_ptr = alloc_tuple(_py, &[start_bits, stop_bits, step_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_hash(slice_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            let start_bits = slice_start_bits(slice_ptr);
            let stop_bits = slice_stop_bits(slice_ptr);
            let step_bits = slice_step_bits(slice_ptr);
            let Some(hash) = hash_slice_bits(_py, start_bits, stop_bits, step_bits) else {
                return MoltObject::none().bits();
            };
            int_bits_from_i64(_py, hash)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_eq(slice_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return not_implemented_bits(_py);
        };
        let Some(other_ptr) = obj_from_bits(other_bits).as_ptr() else {
            return not_implemented_bits(_py);
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return not_implemented_bits(_py);
            }
            if object_type_id(other_ptr) != TYPE_ID_SLICE {
                return not_implemented_bits(_py);
            }
            let start_eq = molt_eq(slice_start_bits(slice_ptr), slice_start_bits(other_ptr));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !is_truthy(_py, obj_from_bits(start_eq)) {
                return MoltObject::from_bool(false).bits();
            }
            let stop_eq = molt_eq(slice_stop_bits(slice_ptr), slice_stop_bits(other_ptr));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !is_truthy(_py, obj_from_bits(stop_eq)) {
                return MoltObject::from_bool(false).bits();
            }
            let step_eq = molt_eq(slice_step_bits(slice_ptr), slice_step_bits(other_ptr));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !is_truthy(_py, obj_from_bits(step_eq)) {
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(true).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_reduce(slice_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(slice_ptr) = obj_from_bits(slice_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(slice_ptr) != TYPE_ID_SLICE {
                return MoltObject::none().bits();
            }
            slice_reduce_tuple(_py, slice_ptr)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_reduce_ex(slice_bits: u64, _protocol_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_slice_reduce(slice_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_new(
    name_bits: u64,
    field_names_bits: u64,
    values_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let name = match string_obj_to_owned(name_obj) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "dataclass name must be a str"),
        };
        let field_names_obj = obj_from_bits(field_names_bits);
        let field_names = match decode_string_list(field_names_obj) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "dataclass field names must be a list/tuple of str",
                );
            }
        };
        let values_obj = obj_from_bits(values_bits);
        let values = match decode_value_list(values_obj) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "dataclass values must be a list/tuple",
                );
            }
        };
        if field_names.len() != values.len() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "dataclass constructor argument mismatch",
            );
        }
        let flags = to_i64(obj_from_bits(flags_bits)).unwrap_or(0) as u64;
        let frozen = (flags & 0x1) != 0;
        let eq = (flags & 0x2) != 0;
        let repr = (flags & 0x4) != 0;
        let slots = (flags & 0x8) != 0;
        let mut field_name_to_index = HashMap::with_capacity(field_names.len());
        for (idx, field_name) in field_names.iter().enumerate() {
            field_name_to_index.insert(field_name.clone(), idx);
        }
        let desc = Box::new(DataclassDesc {
            name,
            field_names,
            field_name_to_index,
            frozen,
            eq,
            repr,
            slots,
            class_bits: 0,
            field_flags: Vec::new(),
            hash_mode: 0,
        });
        let desc_ptr = Box::into_raw(desc);

        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<*mut DataclassDesc>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<u64>();
        let ptr = alloc_object(_py, total, TYPE_ID_DATACLASS);
        if ptr.is_null() {
            unsafe { drop(Box::from_raw(desc_ptr)) };
            return MoltObject::none().bits();
        }
        unsafe {
            let mut vec = Vec::with_capacity(values.len());
            vec.extend_from_slice(&values);
            for &val in values.iter() {
                inc_ref_bits(_py, val);
            }
            let vec_ptr = Box::into_raw(Box::new(vec));
            *(ptr as *mut *mut DataclassDesc) = desc_ptr;
            *(ptr.add(std::mem::size_of::<*mut DataclassDesc>()) as *mut *mut Vec<u64>) = vec_ptr;
            dataclass_set_dict_bits(_py, ptr, 0);
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_get(obj_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let idx = match obj_from_bits(index_bits).as_int() {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "dataclass field index must be int");
            }
        };
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_DATACLASS {
                    return MoltObject::none().bits();
                }
                let fields = dataclass_fields_ref(ptr);
                if idx < 0 || idx as usize >= fields.len() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dataclass field index out of range",
                    );
                }
                let val = fields[idx as usize];
                if is_missing_bits(_py, val) {
                    let desc_ptr = dataclass_desc_ptr(ptr);
                    let name = if !desc_ptr.is_null() {
                        let names = &(*desc_ptr).field_names;
                        names
                            .get(idx as usize)
                            .map(|s| s.as_str())
                            .unwrap_or("field")
                    } else {
                        "field"
                    };
                    return attr_error(_py, "dataclass", name) as u64;
                }
                inc_ref_bits(_py, val);
                return val;
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_set(obj_bits: u64, index_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let idx = match obj_from_bits(index_bits).as_int() {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "dataclass field index must be int");
            }
        };
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_DATACLASS {
                    return MoltObject::none().bits();
                }
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() && (*desc_ptr).frozen {
                    let field_names = &(*desc_ptr).field_names;
                    let field_name = if idx >= 0 {
                        field_names
                            .get(idx as usize)
                            .map(|name| name.as_str())
                            .unwrap_or("<field>")
                    } else {
                        "<field>"
                    };
                    return raise_frozen_instance_error(
                        _py,
                        &format!("cannot assign to field '{field_name}'"),
                    );
                }
                let fields = dataclass_fields_mut(ptr);
                if idx < 0 || idx as usize >= fields.len() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dataclass field index out of range",
                    );
                }
                let old_bits = fields[idx as usize];
                if old_bits != val_bits {
                    dec_ref_bits(_py, old_bits);
                    inc_ref_bits(_py, val_bits);
                    fields[idx as usize] = val_bits;
                }
                return obj_bits;
            }
        }
        MoltObject::none().bits()
    })
}

fn raise_frozen_instance_error(_py: &PyToken<'_>, message: &str) -> u64 {
    let module_name_ptr = alloc_string(_py, b"dataclasses");
    if module_name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let module_name_bits = MoltObject::from_ptr(module_name_ptr).bits();
    let module_bits = crate::molt_module_import(module_name_bits);
    dec_ref_bits(_py, module_name_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, b"FrozenInstanceError") else {
        dec_ref_bits(_py, module_bits);
        return MoltObject::none().bits();
    };
    let missing = missing_bits(_py);
    let class_bits = molt_getattr_builtin(module_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, module_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    if class_bits == missing {
        return raise_exception::<u64>(_py, "RuntimeError", "FrozenInstanceError unavailable");
    }
    let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
        dec_ref_bits(_py, class_bits);
        return raise_exception::<u64>(_py, "TypeError", "FrozenInstanceError class is invalid");
    };
    let message_ptr = alloc_string(_py, message.as_bytes());
    if message_ptr.is_null() {
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let message_bits = MoltObject::from_ptr(message_ptr).bits();
    let exc_bits = unsafe { call_class_init_with_args(_py, class_ptr, &[message_bits]) };
    dec_ref_bits(_py, message_bits);
    dec_ref_bits(_py, class_bits);
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    crate::molt_raise(exc_bits)
}

pub(crate) unsafe fn dataclass_set_class_raw(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    class_bits: u64,
) -> u64 {
    unsafe {
        if object_type_id(ptr) != TYPE_ID_DATACLASS {
            return raise_exception::<_>(_py, "TypeError", "dataclass expects object");
        }
        if class_bits != 0 {
            let class_obj = obj_from_bits(class_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "class must be a type object");
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "class must be a type object");
            }
        }
        let desc_ptr = dataclass_desc_ptr(ptr);
        if !desc_ptr.is_null() {
            let old_bits = (*desc_ptr).class_bits;
            if old_bits != 0 {
                dec_ref_bits(_py, old_bits);
            }
            (*desc_ptr).class_bits = class_bits;
            if class_bits != 0 {
                inc_ref_bits(_py, class_bits);
            }
            object_set_class_bits(_py, ptr, class_bits);
            if class_bits != 0 {
                let class_obj = obj_from_bits(class_bits);
                if let Some(class_ptr) = class_obj.as_ptr()
                    && object_type_id(class_ptr) == TYPE_ID_TYPE
                {
                    let flags_name =
                        attr_name_bits_from_bytes(_py, b"__molt_dataclass_field_flags__");
                    if let Some(flags_name) = flags_name {
                        if let Some(flags_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, flags_name)
                        {
                            let flags_obj = obj_from_bits(flags_bits);
                            let flags_ptr = flags_obj.as_ptr();
                            if let Some(flags_ptr) = flags_ptr {
                                let type_id = object_type_id(flags_ptr);
                                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                                    let elems = seq_vec_ref(flags_ptr);
                                    let mut out = Vec::with_capacity(elems.len());
                                    for &elem_bits in elems.iter() {
                                        let elem_obj = obj_from_bits(elem_bits);
                                        let Some(val) = to_i64(elem_obj) else {
                                            out.clear();
                                            break;
                                        };
                                        if val < 0 || val > u8::MAX as i64 {
                                            out.clear();
                                            break;
                                        }
                                        out.push(val as u8);
                                    }
                                    if !out.is_empty() {
                                        (*desc_ptr).field_flags = out;
                                    }
                                }
                            }
                        }
                        dec_ref_bits(_py, flags_name);
                    }
                    let hash_name = attr_name_bits_from_bytes(_py, b"__molt_dataclass_hash__");
                    if let Some(hash_name) = hash_name {
                        if let Some(hash_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, hash_name)
                        {
                            let hash_obj = obj_from_bits(hash_bits);
                            if let Some(val) = to_i64(hash_obj)
                                && val >= 0
                                && val <= u8::MAX as i64
                            {
                                (*desc_ptr).hash_mode = val as u8;
                            }
                        }
                        dec_ref_bits(_py, hash_name);
                    }
                }
            }
        }
        MoltObject::none().bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dataclass_set_class(obj_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dataclass expects object");
        };
        unsafe { dataclass_set_class_raw(_py, ptr, class_bits) }
    })
}

// --- NaN-boxed ops ---

fn is_number_for_concat(obj: MoltObject) -> bool {
    if obj.as_float().is_some() {
        return true;
    }
    if to_i64(obj).is_some() {
        return true;
    }
    if bigint_ptr_from_bits(obj.bits()).is_some() {
        return true;
    }
    if complex_ptr_from_bits(obj.bits()).is_some() {
        return true;
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_add(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Note: exception_pending check removed — backends guarantee molt_add
        // is only called on non-exception paths, so the TLS + atomic overhead
        // of checking every arithmetic op is unnecessary.
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Guard: skip int fast path if either operand is a float, because
        // to_i64 coerces exact-integer floats (e.g. 2.0 -> 2).
        if !lhs.is_float() && !rhs.is_float() && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            let res = li as i128 + ri as i128;
            return int_bits_from_i128(_py, res);
        }
        // Float fast path — second most common after int, moved before
        // as_ptr / bigint checks to avoid unnecessary pointer dereferences.
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return MoltObject::from_float(lf + rf).bits();
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if ltype == TYPE_ID_STRING && rtype == TYPE_ID_STRING {
                    let l_len = string_len(lp);
                    let r_len = string_len(rp);
                    let l_bytes = std::slice::from_raw_parts(string_bytes(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(string_bytes(rp), r_len);
                    if let Some(bits) = concat_bytes_like(_py, l_bytes, r_bytes, TYPE_ID_STRING) {
                        return bits;
                    }
                    return MoltObject::none().bits();
                }
                if ltype == TYPE_ID_BYTES && rtype == TYPE_ID_BYTES {
                    let l_len = bytes_len(lp);
                    let r_len = bytes_len(rp);
                    let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                    if let Some(bits) = concat_bytes_like(_py, l_bytes, r_bytes, TYPE_ID_BYTES) {
                        return bits;
                    }
                    return MoltObject::none().bits();
                }
                if ltype == TYPE_ID_BYTEARRAY && rtype == TYPE_ID_BYTEARRAY {
                    let l_len = bytes_len(lp);
                    let r_len = bytes_len(rp);
                    let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                    if let Some(bits) = concat_bytes_like(_py, l_bytes, r_bytes, TYPE_ID_BYTEARRAY)
                    {
                        return bits;
                    }
                    return MoltObject::none().bits();
                }
                if ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST {
                    let l_len = list_len(lp);
                    let r_len = list_len(rp);
                    let l_elems = seq_vec_ref(lp);
                    let r_elems = seq_vec_ref(rp);
                    let mut combined = Vec::with_capacity(l_len + r_len);
                    combined.extend_from_slice(l_elems);
                    combined.extend_from_slice(r_elems);
                    let ptr = alloc_list(_py, &combined);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
                if ltype == TYPE_ID_TUPLE && rtype == TYPE_ID_TUPLE {
                    let l_len = tuple_len(lp);
                    let r_len = tuple_len(rp);
                    let l_elems = seq_vec_ref(lp);
                    let r_elems = seq_vec_ref(rp);
                    let mut combined = Vec::with_capacity(l_len + r_len);
                    combined.extend_from_slice(l_elems);
                    combined.extend_from_slice(r_elems);
                    let ptr = alloc_tuple(_py, &combined);
                    if ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(ptr).bits();
                }
            }
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big + r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(lc)), Ok(Some(rc))) => {
                    return complex_bits(_py, lc.re + rc.re, lc.im + rc.im);
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        unsafe {
            let add_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.add_name, b"__add__");
            let radd_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.radd_name, b"__radd__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, add_name_bits, radd_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "+")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_concat(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if is_number_for_concat(lhs) && is_number_for_concat(rhs) {
            return binary_type_error(_py, lhs, rhs, "+");
        }
        molt_add(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_add(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                let ltype = object_type_id(ptr);
                if ltype == TYPE_ID_LIST {
                    let _ = molt_list_extend(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
                if ltype == TYPE_ID_STRING {
                    // In-place string concat: O(n) amortised when refcount == 1.
                    let header = &mut *header_from_obj_ptr(ptr);
                    if header.ref_count.load(std::sync::atomic::Ordering::Relaxed) == 1
                        && (header.flags & crate::object::HEADER_FLAG_IMMORTAL) == 0
                    {
                        let rhs_obj = obj_from_bits(b);
                        if let Some(r_ptr) = rhs_obj.as_ptr() {
                            if object_type_id(r_ptr) == TYPE_ID_STRING {
                                let l_len = string_len(ptr);
                                let r_len = string_len(r_ptr);
                                if let Some(content_len) = l_len.checked_add(r_len) {
                                let needed = std::mem::size_of::<MoltHeader>()
                                    + std::mem::size_of::<usize>()
                                    + content_len;
                                let total_sz = super::total_size_from_header(header, ptr);
                                if total_sz >= needed {
                                    // Fast: spare capacity — append in place, zero alloc
                                    let l_data = string_bytes(ptr) as *mut u8;
                                    let r_data = string_bytes(r_ptr);
                                    std::ptr::copy_nonoverlapping(r_data, l_data.add(l_len), r_len);
                                    *(ptr as *mut usize) = l_len + r_len;
                                    super::object_set_state(ptr, 0); // invalidate hash
                                    inc_ref_bits(_py, a);
                                    return a;
                                }
                                // Slow: allocate 2x, amortised growth
                                let new_cap = std::cmp::max(total_sz * 2, needed + 64);
                                let new_ptr = alloc_object(_py, new_cap, TYPE_ID_STRING);
                                if !new_ptr.is_null() {
                                    let l_data = string_bytes(ptr);
                                    let r_data = string_bytes(r_ptr);
                                    let n_data = string_bytes(new_ptr) as *mut u8;
                                    std::ptr::copy_nonoverlapping(l_data, n_data, l_len);
                                    std::ptr::copy_nonoverlapping(r_data, n_data.add(l_len), r_len);
                                    *(new_ptr as *mut usize) = l_len + r_len;
                                    // Caller dec-refs old LHS after storing result.
                                    return MoltObject::from_ptr(new_ptr).bits();
                                }
                                } // if let Some(content_len) — overflow falls through
                            }
                        }
                    }
                    // Fall through to regular add (concat)
                }
                if ltype == TYPE_ID_BYTEARRAY {
                    if bytearray_concat_in_place(_py, ptr, b) {
                        inc_ref_bits(_py, a);
                        return a;
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        unsafe {
            let iadd_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.iadd_name, b"__iadd__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, iadd_name_bits) {
                return res_bits;
            }
        }
        molt_add(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_concat(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if is_number_for_concat(lhs) && is_number_for_concat(rhs) {
            return binary_type_error(_py, lhs, rhs, "+");
        }
        molt_inplace_add(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sub(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if !lhs.is_float() && !rhs.is_float() && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            let res = li as i128 - ri as i128;
            return int_bits_from_i128(_py, res);
        }
        // Float fast path — moved before bigint/as_ptr checks.
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return MoltObject::from_float(lf - rf).bits();
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big - r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(lc)), Ok(Some(rc))) => {
                    return complex_bits(_py, lc.re - rc.re, lc.im - rc.im);
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    return set_like_difference(_py, lp, rp, ltype);
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
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
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let res = set_like_difference(_py, lhs_ptr, rhs_ptr, TYPE_ID_SET);
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return res;
                }
            }
        }
        unsafe {
            let sub_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.sub_name, b"__sub__");
            let rsub_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rsub_name, b"__rsub__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, sub_name_bits, rsub_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "-")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_sub(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Int/float fast paths — avoid dunder dispatch overhead for numeric types.
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            return int_bits_from_i128(_py, li as i128 - ri as i128);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return MoltObject::from_float(lf - rf).bits();
        }
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let rhs = obj_from_bits(b);
                    let ok = rhs
                        .as_ptr()
                        .is_some_and(|rhs_ptr| is_set_inplace_rhs_type(object_type_id(rhs_ptr)));
                    if !ok {
                        return raise_unsupported_inplace(_py, "-=", a, b);
                    }
                    let _ = molt_set_difference_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        unsafe {
            let isub_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.isub_name, b"__isub__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, isub_name_bits) {
                return res_bits;
            }
        }
        molt_sub(a, b)
    })
}

pub(crate) fn repeat_sequence(_py: &PyToken<'_>, ptr: *mut u8, count: i64) -> Option<u64> {
    unsafe {
        let type_id = object_type_id(ptr);
        if count <= 0 {
            let out_ptr = match type_id {
                TYPE_ID_LIST => alloc_list(_py, &[]),
                TYPE_ID_TUPLE => alloc_tuple(_py, &[]),
                TYPE_ID_STRING => alloc_string(_py, &[]),
                TYPE_ID_BYTES => alloc_bytes(_py, &[]),
                TYPE_ID_BYTEARRAY => alloc_bytearray(_py, &[]),
                _ => return None,
            };
            if out_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return Some(MoltObject::from_ptr(out_ptr).bits());
        }
        if count == 1 && type_id == TYPE_ID_TUPLE {
            let bits = MoltObject::from_ptr(ptr).bits();
            inc_ref_bits(_py, bits);
            return Some(bits);
        }

        let times = count as usize;
        match type_id {
            TYPE_ID_LIST => {
                let elems = seq_vec_ref(ptr);
                let total = match elems.len().checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                let mut combined = Vec::with_capacity(total);
                for _ in 0..times {
                    combined.extend_from_slice(elems);
                }
                let out_ptr = alloc_list(_py, &combined);
                if out_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(ptr);
                let total = match elems.len().checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                let mut combined = Vec::with_capacity(total);
                for _ in 0..times {
                    combined.extend_from_slice(elems);
                }
                let out_ptr = alloc_tuple(_py, &combined);
                if out_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_STRING => {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let total = match len.checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                let out_ptr = alloc_bytes_like_with_len(_py, total, TYPE_ID_STRING);
                if out_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                let data_ptr = out_ptr.add(std::mem::size_of::<usize>());
                let out_slice = std::slice::from_raw_parts_mut(data_ptr, total);
                fill_repeated_bytes(out_slice, bytes);
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_BYTES => {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let total = match len.checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                let out_ptr = alloc_bytes_like_with_len(_py, total, TYPE_ID_BYTES);
                if out_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                let data_ptr = out_ptr.add(std::mem::size_of::<usize>());
                let out_slice = std::slice::from_raw_parts_mut(data_ptr, total);
                fill_repeated_bytes(out_slice, bytes);
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            TYPE_ID_BYTEARRAY => {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                let total = match len.checked_mul(times) {
                    Some(total) => total,
                    None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
                };
                let mut out = Vec::with_capacity(total);
                for _ in 0..times {
                    out.extend_from_slice(bytes);
                }
                let out_ptr = alloc_bytearray(_py, &out);
                if out_ptr.is_null() {
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
                Some(MoltObject::from_ptr(out_ptr).bits())
            }
            _ => None,
        }
    }
}

unsafe fn list_repeat_in_place(_py: &PyToken<'_>, ptr: *mut u8, count: i64) -> bool {
    unsafe {
        let elems = seq_vec(ptr);
        if count <= 0 {
            for &item in elems.iter() {
                dec_ref_bits(_py, item);
            }
            elems.clear();
            return true;
        }
        let count = match usize::try_from(count) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        if count == 1 {
            return true;
        }
        let snapshot = elems.clone();
        if snapshot.is_empty() {
            return true;
        }
        let total = match snapshot.len().checked_mul(count) {
            Some(total) => total,
            None => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        elems.reserve(total.saturating_sub(snapshot.len()));
        for _ in 1..count {
            for &item in snapshot.iter() {
                elems.push(item);
                inc_ref_bits(_py, item);
            }
        }
        true
    }
}

unsafe fn bytearray_repeat_in_place(_py: &PyToken<'_>, ptr: *mut u8, count: i64) -> bool {
    unsafe {
        let elems = bytearray_vec(ptr);
        if count <= 0 {
            elems.clear();
            return true;
        }
        let count = match usize::try_from(count) {
            Ok(val) => val,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        if count == 1 {
            return true;
        }
        let snapshot = elems.clone();
        if snapshot.is_empty() {
            return true;
        }
        let total = match snapshot.len().checked_mul(count) {
            Some(total) => total,
            None => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        elems.reserve(total.saturating_sub(snapshot.len()));
        for _ in 1..count {
            elems.extend_from_slice(&snapshot);
        }
        true
    }
}

unsafe fn bytearray_concat_in_place(_py: &PyToken<'_>, ptr: *mut u8, other_bits: u64) -> bool {
    unsafe {
        let other = obj_from_bits(other_bits);
        let Some(other_ptr) = other.as_ptr() else {
            let msg = format!("can't concat {} to bytearray", type_name(_py, other));
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let other_type = object_type_id(other_ptr);
        let payload = if other_type == TYPE_ID_MEMORYVIEW {
            if let Some(slice) = memoryview_bytes_slice(other_ptr) {
                slice.to_vec()
            } else if let Some(buf) = memoryview_collect_bytes(other_ptr) {
                buf
            } else {
                let msg = format!("can't concat {} to bytearray", type_name(_py, other));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        } else if other_type == TYPE_ID_BYTES || other_type == TYPE_ID_BYTEARRAY {
            if other_ptr == ptr {
                bytearray_vec_ref(ptr).clone()
            } else {
                bytes_like_slice_raw(other_ptr).unwrap_or(&[]).to_vec()
            }
        } else {
            let msg = format!("can't concat {} to bytearray", type_name(_py, other));
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        bytearray_vec(ptr).extend_from_slice(&payload);
        true
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_mul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Int/float fast paths — avoid dunder dispatch overhead for numeric types.
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            return int_bits_from_i128(_py, li as i128 * ri as i128);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return MoltObject::from_float(lf * rf).bits();
        }
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                let ltype = object_type_id(ptr);
                if ltype == TYPE_ID_LIST || ltype == TYPE_ID_BYTEARRAY {
                    let rhs_type = type_name(_py, obj_from_bits(b));
                    let msg = format!("can't multiply sequence by non-int of type '{rhs_type}'");
                    let count = index_i64_from_obj(_py, b, &msg);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let ok = if ltype == TYPE_ID_LIST {
                        list_repeat_in_place(_py, ptr, count)
                    } else {
                        bytearray_repeat_in_place(_py, ptr, count)
                    };
                    if !ok || exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        molt_mul(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if !lhs.is_float() && !rhs.is_float() && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            let res = li as i128 * ri as i128;
            return int_bits_from_i128(_py, res);
        }
        // Float fast path — moved before repeat_sequence/bigint checks.
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return MoltObject::from_float(lf * rf).bits();
        }
        if let Some(count) = to_i64(lhs)
            && let Some(ptr) = rhs.as_ptr()
            && let Some(bits) = repeat_sequence(_py, ptr, count)
        {
            return bits;
        }
        if let Some(count) = to_i64(rhs)
            && let Some(ptr) = lhs.as_ptr()
            && let Some(bits) = repeat_sequence(_py, ptr, count)
        {
            return bits;
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big * r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(lc)), Ok(Some(rc))) => {
                    let re = lc.re * rc.re - lc.im * rc.im;
                    let im = lc.im * rc.re + lc.re * rc.im;
                    return complex_bits(_py, re, im);
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        unsafe {
            let mul_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.mul_name, b"__mul__");
            let rmul_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rmul_name, b"__rmul__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, mul_name_bits, rmul_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "*")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_div(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Python true division: int / int always returns float
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "division by zero");
            }
            return MoltObject::from_float(li as f64 / ri as f64).bits();
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "division by zero");
            }
            return MoltObject::from_float(lf / rf).bits();
        }
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(lc)), Ok(Some(rc))) => {
                    let denom = rc.re * rc.re + rc.im * rc.im;
                    if denom == 0.0 {
                        return raise_exception::<_>(_py, "ZeroDivisionError", "division by zero");
                    }
                    let re = (lc.re * rc.re + lc.im * rc.im) / denom;
                    let im = (lc.im * rc.re - lc.re * rc.im) / denom;
                    return complex_bits(_py, re, im);
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        if bigint_ptr_from_bits(a).is_some() || bigint_ptr_from_bits(b).is_some() {
            return raise_exception::<_>(_py, "OverflowError", "int too large to convert to float");
        }
        unsafe {
            let div_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.truediv_name,
                b"__truediv__",
            );
            let rdiv_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.rtruediv_name,
                b"__rtruediv__",
            );
            if let Some(res_bits) = call_binary_dunder(_py, a, b, div_name_bits, rdiv_name_bits) {
                return res_bits;
            }
        }
        raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for /")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_div(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let idiv_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.itruediv_name,
                b"__itruediv__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, idiv_name_bits) {
                return res_bits;
            }
        }
        molt_div(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_floordiv(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let either_float = lhs.is_float() || rhs.is_float();
        if !either_float && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            if li == i64::MIN && ri == -1 {
                // overflow — fall through to bigint
            } else {
                let q = li / ri;
                let r = li % ri;
                let res = if r != 0 && (r < 0) != (ri < 0) { q - 1 } else { q };
                return MoltObject::from_int(res).bits();
            }
        }
        if !either_float && let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if r_big.is_zero() {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let res = l_big.div_floor(&r_big);
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "float floor division by zero",
                );
            }
            return MoltObject::from_float((lf / rf).floor()).bits();
        }
        unsafe {
            let div_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.floordiv_name,
                b"__floordiv__",
            );
            let rdiv_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.rfloordiv_name,
                b"__rfloordiv__",
            );
            if let Some(res_bits) = call_binary_dunder(_py, a, b, div_name_bits, rdiv_name_bits) {
                return res_bits;
            }
        }
        raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for //")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_floordiv(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let idiv_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.ifloordiv_name,
                b"__ifloordiv__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, idiv_name_bits) {
                return res_bits;
            }
        }
        molt_floordiv(a, b)
    })
}

#[derive(Clone, Copy, Default)]
struct PercentFormatFlags {
    left_adjust: bool,
    sign_plus: bool,
    sign_space: bool,
    zero_pad: bool,
    alternate: bool,
}

fn percent_object_has_getitem(_py: &PyToken<'_>, ptr: *mut u8) -> bool {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") else {
        return false;
    };
    let call_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, name_bits) };
    dec_ref_bits(_py, name_bits);
    if let Some(call_bits) = call_bits {
        dec_ref_bits(_py, call_bits);
        return true;
    }
    false
}

fn percent_rhs_allows_unused_non_tuple(_py: &PyToken<'_>, rhs: MoltObject) -> bool {
    let Some(ptr) = rhs.as_ptr() else {
        return false;
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_STRING || type_id == TYPE_ID_TUPLE {
            return false;
        }
    }
    percent_object_has_getitem(_py, ptr)
}

fn percent_parse_usize(
    _py: &PyToken<'_>,
    bytes: &[u8],
    idx: &mut usize,
    field_name: &str,
) -> Option<usize> {
    let start = *idx;
    let mut out: usize = 0;
    while *idx < bytes.len() && bytes[*idx].is_ascii_digit() {
        let digit = (bytes[*idx] - b'0') as usize;
        out = match out.checked_mul(10).and_then(|v| v.checked_add(digit)) {
            Some(v) => v,
            None => {
                let msg = format!("{field_name} too large in format string");
                return raise_exception::<Option<usize>>(_py, "ValueError", &msg);
            }
        };
        *idx += 1;
    }
    if *idx == start { None } else { Some(out) }
}

fn percent_unsupported_char(_py: &PyToken<'_>, ch: u8, idx: usize) -> Option<String> {
    let ch_display = ch as char;
    let msg = format!("unsupported format character '{ch_display}' (0x{ch:02x}) at index {idx}");
    raise_exception::<Option<String>>(_py, "ValueError", &msg)
}

fn percent_apply_width(
    text: String,
    width: Option<usize>,
    left_adjust: bool,
    pad_char: char,
) -> String {
    let Some(width) = width else {
        return text;
    };
    let text_len = text.chars().count();
    if text_len >= width {
        return text;
    }
    let pad_len = width - text_len;
    let padding = pad_char.to_string().repeat(pad_len);
    if left_adjust {
        format!("{text}{padding}")
    } else {
        format!("{padding}{text}")
    }
}

fn percent_apply_numeric_width(
    prefix: &str,
    body: String,
    width: Option<usize>,
    left_adjust: bool,
    zero_pad: bool,
) -> String {
    let prefix_len = prefix.chars().count();
    let body_len = body.chars().count();
    if zero_pad
        && !left_adjust
        && let Some(width) = width
        && width > prefix_len + body_len
    {
        let mut out = String::with_capacity(width);
        out.push_str(prefix);
        out.push_str(&"0".repeat(width - prefix_len - body_len));
        out.push_str(&body);
        return out;
    }
    let mut text = String::with_capacity(prefix.len() + body.len());
    text.push_str(prefix);
    text.push_str(&body);
    percent_apply_width(text, width, left_adjust, ' ')
}

fn percent_raise_real_type_error_decimal(
    _py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
) -> Option<BigInt> {
    let conv_ch = conv as char;
    let msg = format!(
        "%{conv_ch} format: a real number is required, not {}",
        type_name(_py, obj)
    );
    raise_exception::<Option<BigInt>>(_py, "TypeError", &msg)
}

fn percent_raise_integer_type_error(
    _py: &PyToken<'_>,
    obj: MoltObject,
    conv: u8,
) -> Option<BigInt> {
    let conv_ch = conv as char;
    let msg = format!(
        "%{conv_ch} format: an integer is required, not {}",
        type_name(_py, obj)
    );
    raise_exception::<Option<BigInt>>(_py, "TypeError", &msg)
}

fn percent_raise_real_type_error_f(_py: &PyToken<'_>, obj: MoltObject) -> Option<f64> {
    let msg = format!("must be real number, not {}", type_name(_py, obj));
    raise_exception::<Option<f64>>(_py, "TypeError", &msg)
}

fn percent_raise_char_type_error(_py: &PyToken<'_>, obj: MoltObject) -> Option<char> {
    let _ = obj;
    raise_exception::<Option<char>>(_py, "TypeError", "%c requires int or char")
}

fn percent_char_from_bigint(_py: &PyToken<'_>, value: BigInt) -> Option<char> {
    let max_code = BigInt::from(0x110000u32);
    if value.sign() == Sign::Minus || value >= max_code {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    }
    let Some(code) = value.to_u32() else {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    };
    let Some(ch) = char::from_u32(code) else {
        return raise_exception::<Option<char>>(
            _py,
            "OverflowError",
            "%c arg not in range(0x110000)",
        );
    };
    Some(ch)
}

fn percent_decimal_from_obj(_py: &PyToken<'_>, value_bits: u64, conv: u8) -> Option<BigInt> {
    let obj = obj_from_bits(value_bits);
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return Some(unsafe { bigint_ref(big_ptr) }.clone());
    }
    if let Some(f) = obj.as_float() {
        if f.is_nan() {
            return raise_exception::<Option<BigInt>>(
                _py,
                "ValueError",
                "cannot convert float NaN to integer",
            );
        }
        if f.is_infinite() {
            return raise_exception::<Option<BigInt>>(
                _py,
                "OverflowError",
                "cannot convert float infinity to integer",
            );
        }
        return Some(bigint_from_f64_trunc(f));
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_real_type_error_decimal(_py, obj, conv);
            }
            let int_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.int_name, b"__int__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, int_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__int__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
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
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_real_type_error_decimal(_py, obj, conv)
}

fn percent_integer_from_obj(_py: &PyToken<'_>, value_bits: u64, conv: u8) -> Option<BigInt> {
    let obj = obj_from_bits(value_bits);
    if let Some(i) = to_i64(obj) {
        return Some(BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return Some(unsafe { bigint_ref(big_ptr) }.clone());
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_integer_type_error(_py, obj, conv);
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return Some(out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<BigInt>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_integer_type_error(_py, obj, conv)
}

fn percent_char_from_obj(_py: &PyToken<'_>, value_bits: u64) -> Option<char> {
    let obj = obj_from_bits(value_bits);
    if let Some(text) = string_obj_to_owned(obj) {
        let mut chars = text.chars();
        return match chars.next() {
            Some(ch) if chars.next().is_none() => Some(ch),
            _ => percent_raise_char_type_error(_py, obj),
        };
    }
    if let Some(i) = to_i64(obj) {
        return percent_char_from_bigint(_py, BigInt::from(i));
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return percent_char_from_bigint(_py, unsafe { bigint_ref(big_ptr) }.clone());
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return percent_char_from_bigint(_py, BigInt::from(i));
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).clone();
                    dec_ref_bits(_py, res_bits);
                    return percent_char_from_bigint(_py, out);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<char>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_char_type_error(_py, obj)
}

fn percent_float_from_obj(_py: &PyToken<'_>, value_bits: u64) -> Option<f64> {
    let obj = obj_from_bits(value_bits);
    if let Some(f) = obj.as_float() {
        return Some(f);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(big_ptr) = bigint_ptr_from_bits(value_bits) {
        return match unsafe { bigint_ref(big_ptr) }.to_f64() {
            Some(v) => Some(v),
            None => raise_exception::<Option<f64>>(
                _py,
                "OverflowError",
                "int too large to convert to float",
            ),
        };
    }
    if let Some(ptr) = maybe_ptr_from_bits(value_bits) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_COMPLEX
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
            {
                return percent_raise_real_type_error_f(_py, obj);
            }
            let float_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.float_name, b"__float__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits) {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(f) = res_obj.as_float() {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(f);
                }
                let owner = class_name_for_error(type_of_bits(_py, value_bits));
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                return raise_exception::<Option<f64>>(_py, "TypeError", &msg);
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
                    if obj_from_bits(res_bits).as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return None;
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    return Some(i as f64);
                }
                if let Some(res_big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let out = bigint_ref(res_big_ptr).to_f64();
                    dec_ref_bits(_py, res_bits);
                    return match out {
                        Some(v) => Some(v),
                        None => raise_exception::<Option<f64>>(
                            _py,
                            "OverflowError",
                            "int too large to convert to float",
                        ),
                    };
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<Option<f64>>(_py, "TypeError", &msg);
            }
            if exception_pending(_py) {
                return None;
            }
        }
    }
    percent_raise_real_type_error_f(_py, obj)
}

fn percent_numeric_prefix(is_negative: bool, flags: PercentFormatFlags) -> Option<char> {
    if is_negative {
        Some('-')
    } else if flags.sign_plus {
        Some('+')
    } else if flags.sign_space {
        Some(' ')
    } else {
        None
    }
}

fn percent_format_text(
    text: String,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> String {
    let rendered = if let Some(precision) = precision {
        text.chars().take(precision).collect::<String>()
    } else {
        text
    };
    percent_apply_width(rendered, width, flags.left_adjust, ' ')
}

fn percent_format_decimal(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_decimal_from_obj(_py, value_bits, conv)?;
    let negative = value.is_negative();
    let mut body = value.abs().to_string();
    if let Some(precision) = precision
        && body.len() < precision
    {
        body = format!("{}{}", "0".repeat(precision - body.len()), body);
    }
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    let zero_pad = flags.zero_pad && !flags.left_adjust;
    Some(percent_apply_numeric_width(
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        zero_pad,
    ))
}

fn percent_format_radix(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_integer_from_obj(_py, value_bits, conv)?;
    let negative = value.is_negative();
    let mut body = match conv {
        b'o' => value.abs().to_str_radix(8),
        b'x' | b'X' => value.abs().to_str_radix(16),
        _ => value.abs().to_string(),
    };
    if conv == b'X' {
        body = body.to_uppercase();
    }
    if let Some(precision) = precision
        && body.len() < precision
    {
        body = format!("{}{}", "0".repeat(precision - body.len()), body);
    }
    let mut prefix = String::new();
    if let Some(sign) = percent_numeric_prefix(negative, flags) {
        prefix.push(sign);
    }
    if flags.alternate {
        match conv {
            b'o' => prefix.push_str("0o"),
            b'x' => prefix.push_str("0x"),
            b'X' => prefix.push_str("0X"),
            _ => {}
        }
    }
    Some(percent_apply_numeric_width(
        prefix.as_str(),
        body,
        width,
        flags.left_adjust,
        flags.zero_pad && !flags.left_adjust,
    ))
}

fn percent_format_float(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
    conv: u8,
) -> Option<String> {
    let value = percent_float_from_obj(_py, value_bits)?;
    let sign = if flags.sign_plus {
        Some('+')
    } else if flags.sign_space {
        Some(' ')
    } else {
        None
    };
    let align = if flags.left_adjust {
        Some('<')
    } else if flags.zero_pad {
        Some('=')
    } else {
        None
    };
    let spec = FormatSpec {
        fill: if flags.zero_pad && !flags.left_adjust {
            '0'
        } else {
            ' '
        },
        align,
        sign,
        alternate: flags.alternate,
        width,
        grouping: None,
        precision,
        ty: Some(conv as char),
    };
    match format_float_with_spec(MoltObject::from_float(value), &spec) {
        Ok(text) => Some(text),
        Err((kind, msg)) => raise_exception::<Option<String>>(_py, kind, msg.as_ref()),
    }
}

fn percent_format_ascii(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    precision: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<String> {
    let rendered_bits = molt_ascii_from_obj(value_bits);
    if exception_pending(_py) {
        if obj_from_bits(rendered_bits).as_ptr().is_some() {
            dec_ref_bits(_py, rendered_bits);
        }
        return None;
    }
    let rendered = string_obj_to_owned(obj_from_bits(rendered_bits));
    if obj_from_bits(rendered_bits).as_ptr().is_some() {
        dec_ref_bits(_py, rendered_bits);
    }
    let rendered = rendered.unwrap_or_default();
    Some(percent_format_text(rendered, width, precision, flags))
}

fn percent_format_char(
    _py: &PyToken<'_>,
    value_bits: u64,
    width: Option<usize>,
    flags: PercentFormatFlags,
) -> Option<String> {
    let ch = percent_char_from_obj(_py, value_bits)?;
    Some(percent_apply_width(
        ch.to_string(),
        width,
        flags.left_adjust,
        ' ',
    ))
}

fn percent_lookup_mapping_arg(_py: &PyToken<'_>, rhs_bits: u64, key: &str) -> Option<(u64, bool)> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let Some(rhs_ptr) = rhs_obj.as_ptr() else {
        return raise_exception::<Option<(u64, bool)>>(
            _py,
            "TypeError",
            "format requires a mapping",
        );
    };
    unsafe {
        let rhs_type = object_type_id(rhs_ptr);
        if rhs_type == TYPE_ID_TUPLE {
            return raise_exception::<Option<(u64, bool)>>(
                _py,
                "TypeError",
                "format requires a mapping",
            );
        }
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        if rhs_type == TYPE_ID_DICT {
            if let Some(bits) = dict_get_in_place(_py, rhs_ptr, key_bits) {
                dec_ref_bits(_py, key_bits);
                return Some((bits, false));
            }
            if exception_pending(_py) {
                dec_ref_bits(_py, key_bits);
                return None;
            }
            raise_key_error_with_key::<()>(_py, key_bits);
            dec_ref_bits(_py, key_bits);
            return None;
        }
        if !percent_object_has_getitem(_py, rhs_ptr) {
            dec_ref_bits(_py, key_bits);
            return raise_exception::<Option<(u64, bool)>>(
                _py,
                "TypeError",
                "format requires a mapping",
            );
        }
        let bits = molt_index(rhs_bits, key_bits);
        dec_ref_bits(_py, key_bits);
        if exception_pending(_py) {
            return None;
        }
        Some((bits, true))
    }
}

fn percent_consume_next_arg(
    _py: &PyToken<'_>,
    rhs_bits: u64,
    tuple_ptr: Option<*mut u8>,
    tuple_idx: &mut usize,
    single_consumed: &mut bool,
) -> Option<u64> {
    if let Some(ptr) = tuple_ptr {
        let elems = unsafe { seq_vec_ref(ptr) };
        if *tuple_idx >= elems.len() {
            return raise_exception::<Option<u64>>(
                _py,
                "TypeError",
                "not enough arguments for format string",
            );
        }
        let bits = elems[*tuple_idx];
        *tuple_idx += 1;
        return Some(bits);
    }
    if *single_consumed {
        return raise_exception::<Option<u64>>(
            _py,
            "TypeError",
            "not enough arguments for format string",
        );
    }
    *single_consumed = true;
    Some(rhs_bits)
}

fn string_percent_format_impl(_py: &PyToken<'_>, text: &str, rhs_bits: u64) -> Option<String> {
    let rhs_obj = obj_from_bits(rhs_bits);
    let tuple_ptr = rhs_obj
        .as_ptr()
        .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_TUPLE });
    let mut tuple_idx = 0usize;
    let mut single_consumed = false;
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len() + 16);
    let mut literal_start = 0usize;
    let mut idx = 0usize;
    while idx < bytes.len() {
        if bytes[idx] != b'%' {
            idx += 1;
            continue;
        }
        out.push_str(&text[literal_start..idx]);
        idx += 1;
        if idx >= bytes.len() {
            return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
        }
        if bytes[idx] == b'%' {
            out.push('%');
            idx += 1;
            literal_start = idx;
            continue;
        }
        let mut key: Option<&str> = None;
        if bytes[idx] == b'(' {
            let key_start = idx + 1;
            let mut key_end = key_start;
            while key_end < bytes.len() && bytes[key_end] != b')' {
                key_end += 1;
            }
            if key_end >= bytes.len() {
                return raise_exception::<Option<String>>(
                    _py,
                    "ValueError",
                    "incomplete format key",
                );
            }
            key = Some(&text[key_start..key_end]);
            idx = key_end + 1;
        }
        let mut flags = PercentFormatFlags::default();
        loop {
            if idx >= bytes.len() {
                return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
            }
            match bytes[idx] {
                b'-' => flags.left_adjust = true,
                b'+' => flags.sign_plus = true,
                b' ' => flags.sign_space = true,
                b'0' => flags.zero_pad = true,
                b'#' => flags.alternate = true,
                _ => break,
            }
            idx += 1;
        }
        let mut width = if idx < bytes.len() && bytes[idx].is_ascii_digit() {
            percent_parse_usize(_py, bytes, &mut idx, "width")
        } else {
            None
        };
        if idx < bytes.len() && bytes[idx] == b'*' {
            idx += 1;
            let width_bits = percent_consume_next_arg(
                _py,
                rhs_bits,
                tuple_ptr,
                &mut tuple_idx,
                &mut single_consumed,
            )?;
            let width_val = index_i64_from_obj(_py, width_bits, "* wants int");
            if exception_pending(_py) {
                return None;
            }
            if width_val < 0 {
                flags.left_adjust = true;
                let abs = width_val.checked_abs().unwrap_or(i64::MAX);
                let Ok(width_usize) = usize::try_from(abs) else {
                    return raise_exception::<Option<String>>(
                        _py,
                        "OverflowError",
                        "width too big",
                    );
                };
                width = Some(width_usize);
            } else {
                let Ok(width_usize) = usize::try_from(width_val) else {
                    return raise_exception::<Option<String>>(
                        _py,
                        "OverflowError",
                        "width too big",
                    );
                };
                width = Some(width_usize);
            }
        }
        let mut precision: Option<usize> = None;
        if idx < bytes.len() && bytes[idx] == b'.' {
            idx += 1;
            if idx < bytes.len() && bytes[idx] == b'*' {
                idx += 1;
                let prec_bits = percent_consume_next_arg(
                    _py,
                    rhs_bits,
                    tuple_ptr,
                    &mut tuple_idx,
                    &mut single_consumed,
                )?;
                let prec_val = index_i64_from_obj(_py, prec_bits, "* wants int");
                if exception_pending(_py) {
                    return None;
                }
                if prec_val <= 0 {
                    precision = Some(0);
                } else {
                    let Ok(prec_usize) = usize::try_from(prec_val) else {
                        return raise_exception::<Option<String>>(
                            _py,
                            "OverflowError",
                            "precision too big",
                        );
                    };
                    precision = Some(prec_usize);
                }
            } else {
                precision =
                    Some(percent_parse_usize(_py, bytes, &mut idx, "precision").unwrap_or(0));
            }
        }
        if idx < bytes.len() && (bytes[idx] == b'h' || bytes[idx] == b'l' || bytes[idx] == b'L') {
            let first = bytes[idx];
            idx += 1;
            if idx < bytes.len() && (first == b'h' || first == b'l') && bytes[idx] == first {
                idx += 1;
            }
        }
        if idx >= bytes.len() {
            return raise_exception::<Option<String>>(_py, "ValueError", "incomplete format");
        }
        let conv_idx = idx;
        let conv = bytes[idx];
        idx += 1;
        let (value_bits, drop_value) = if let Some(key) = key {
            percent_lookup_mapping_arg(_py, rhs_bits, key)?
        } else {
            (
                percent_consume_next_arg(
                    _py,
                    rhs_bits,
                    tuple_ptr,
                    &mut tuple_idx,
                    &mut single_consumed,
                )?,
                false,
            )
        };
        let rendered = match conv {
            b's' => Some(percent_format_text(
                format_obj_str(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            )),
            b'r' => Some(percent_format_text(
                format_obj(_py, obj_from_bits(value_bits)),
                width,
                precision,
                flags,
            )),
            b'a' => percent_format_ascii(_py, value_bits, width, precision, flags),
            b'c' => percent_format_char(_py, value_bits, width, flags),
            b'd' | b'i' | b'u' => {
                percent_format_decimal(_py, value_bits, width, precision, flags, conv)
            }
            b'o' | b'x' | b'X' => {
                percent_format_radix(_py, value_bits, width, precision, flags, conv)
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => {
                percent_format_float(_py, value_bits, width, precision, flags, conv)
            }
            _ => percent_unsupported_char(_py, conv, conv_idx),
        };
        if drop_value {
            dec_ref_bits(_py, value_bits);
        }
        let rendered = rendered?;
        out.push_str(&rendered);
        literal_start = idx;
    }
    out.push_str(&text[literal_start..]);
    if let Some(ptr) = tuple_ptr {
        let elems = unsafe { seq_vec_ref(ptr) };
        if tuple_idx < elems.len() {
            return raise_exception::<Option<String>>(
                _py,
                "TypeError",
                "not all arguments converted during string formatting",
            );
        }
    } else if !single_consumed && !percent_rhs_allows_unused_non_tuple(_py, rhs_obj) {
        return raise_exception::<Option<String>>(
            _py,
            "TypeError",
            "not all arguments converted during string formatting",
        );
    }
    Some(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_mod(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        // Int fast path first — much more common than string % formatting.
        // Skip if either operand is a float so that e.g. 7 % 2.0 returns 1.0 (float).
        let either_float = lhs.is_float() || rhs.is_float();
        if !either_float && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "integer division or modulo by zero");
            }
            let mut rem = li % ri;
            if rem != 0 && (rem > 0) != (ri > 0) {
                rem += ri;
            }
            return MoltObject::from_int(rem).bits();
        }
        // String % formatting — moved after int fast path.
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    let text = string_obj_to_owned(lhs).unwrap_or_default();
                    let Some(rendered) = string_percent_format_impl(_py, &text, b) else {
                        return MoltObject::none().bits();
                    };
                    let out_ptr = alloc_string(_py, rendered.as_bytes());
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
        }
        if !either_float && let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if r_big.is_zero() {
                return raise_exception::<_>(_py, "ZeroDivisionError", "integer division or modulo by zero");
            }
            let res = l_big.mod_floor(&r_big);
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "float modulo");
            }
            let mut rem = lf % rf;
            if rem != 0.0 && (rem > 0.0) != (rf > 0.0) {
                rem += rf;
            }
            return MoltObject::from_float(rem).bits();
        }
        raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for %")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_mod(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let imod_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.imod_name, b"__imod__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, imod_name_bits) {
                return res_bits;
            }
        }
        molt_mod(a, b)
    })
}

fn complex_pow(base: ComplexParts, exp: ComplexParts) -> Result<ComplexParts, ()> {
    if base.re == 0.0 && base.im == 0.0 {
        if exp.re == 0.0 && exp.im == 0.0 {
            return Ok(ComplexParts { re: 1.0, im: 0.0 });
        }
        if exp.im != 0.0 || exp.re < 0.0 {
            return Err(());
        }
        return Ok(ComplexParts { re: 0.0, im: 0.0 });
    }
    let r = (base.re * base.re + base.im * base.im).sqrt();
    let theta = base.im.atan2(base.re);
    let log_r = r.ln();
    let u = exp.re * log_r - exp.im * theta;
    let v = exp.im * log_r + exp.re * theta;
    let exp_u = u.exp();
    Ok(ComplexParts {
        re: exp_u * v.cos(),
        im: exp_u * v.sin(),
    })
}

fn pow_i64_checked(base: i64, exp: i64) -> Option<i64> {
    if exp < 0 {
        return None;
    }
    let mut result: i128 = 1;
    let mut base_val: i128 = base as i128;
    let mut exp_val = exp as u64;
    let max = (1i128 << 46) - 1;
    let min = -(1i128 << 46);
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = result.saturating_mul(base_val);
            if result > max || result < min {
                return None;
            }
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base_val = base_val.saturating_mul(base_val);
            if base_val > max || base_val < min {
                return None;
            }
        }
    }
    Some(result as i64)
}

fn mod_py_i128(value: i128, modulus: i128) -> i128 {
    let mut rem = value % modulus;
    if rem != 0 && (rem > 0) != (modulus > 0) {
        rem += modulus;
    }
    rem
}

fn mod_pow_i128(_py: &PyToken<'_>, mut base: i128, exp: i64, modulus: i128) -> i128 {
    let mut result: i128 = 1;
    base = mod_py_i128(base, modulus);
    let mut exp_val = exp as u64;
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = mod_py_i128(result * base, modulus);
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base = mod_py_i128(base * base, modulus);
        }
    }
    mod_py_i128(result, modulus)
}

fn egcd_i128(a: i128, b: i128) -> (i128, i128, i128) {
    if b == 0 {
        return (a, 1, 0);
    }
    let (g, x, y) = egcd_i128(b, a % b);
    (g, y, x - (a / b) * y)
}

fn mod_inverse_i128(_py: &PyToken<'_>, value: i128, modulus: i128) -> Option<i128> {
    let (g, x, _) = egcd_i128(value, modulus);
    if g == 1 || g == -1 {
        Some(mod_py_i128(x, modulus))
    } else {
        None
    }
}

fn mod_pow_bigint(base: &BigInt, exp: u64, modulus: &BigInt) -> BigInt {
    let mut result = BigInt::from(1);
    let mut base_val = base.mod_floor(modulus);
    let mut exp_val = exp;
    while exp_val > 0 {
        if (exp_val & 1) != 0 {
            result = (result * &base_val).mod_floor(modulus);
        }
        exp_val >>= 1;
        if exp_val > 0 {
            base_val = (&base_val * &base_val).mod_floor(modulus);
        }
    }
    result
}

fn egcd_bigint(a: BigInt, b: BigInt) -> (BigInt, BigInt, BigInt) {
    if b.is_zero() {
        return (a, BigInt::from(1), BigInt::from(0));
    }
    let (q, r) = a.div_mod_floor(&b);
    let (g, x, y) = egcd_bigint(b, r);
    (g, y.clone(), x - q * y)
}

fn mod_inverse_bigint(value: BigInt, modulus: &BigInt) -> Option<BigInt> {
    let (g, x, _) = egcd_bigint(value, modulus.clone());
    if g == BigInt::from(1) || g == BigInt::from(-1) {
        Some(x.mod_floor(modulus))
    } else {
        None
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pow(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if complex_ptr_from_bits(a).is_some() || complex_ptr_from_bits(b).is_some() {
            match (
                complex_from_obj_strict(_py, lhs),
                complex_from_obj_strict(_py, rhs),
            ) {
                (Ok(Some(base)), Ok(Some(exp))) => {
                    return match complex_pow(base, exp) {
                        Ok(out) => complex_bits(_py, out.re, out.im),
                        Err(()) => raise_exception::<_>(
                            _py,
                            "ZeroDivisionError",
                            "zero to a negative or complex power",
                        ),
                    };
                }
                (Err(_), _) | (_, Err(_)) => {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "int too large to convert to float",
                    );
                }
                _ => {}
            }
        }
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri >= 0 {
                if let Some(res) = pow_i64_checked(li, ri) {
                    return MoltObject::from_int(res).bits();
                }
                let res = BigInt::from(li).pow(ri as u32);
                if let Some(i) = bigint_to_inline(&res) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, res);
            }
            let lf = li as f64;
            let rf = ri as f64;
            if lf == 0.0 && rf < 0.0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "0.0 cannot be raised to a negative power",
                );
            }
            let out = lf.powf(rf);
            if out.is_infinite() && lf.is_finite() && rf.is_finite() {
                return raise_exception::<_>(_py, "OverflowError", "math range error");
            }
            return MoltObject::from_float(out).bits();
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if let Some(exp) = r_big.to_u64() {
                let res = l_big.pow(exp as u32);
                if let Some(i) = bigint_to_inline(&res) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, res);
            }
            if r_big.is_negative()
                && let Some(lf) = l_big.to_f64()
            {
                let rf = r_big.to_f64().unwrap_or(f64::NEG_INFINITY);
                if lf == 0.0 && rf < 0.0 {
                    return raise_exception::<_>(
                        _py,
                        "ZeroDivisionError",
                        "0.0 cannot be raised to a negative power",
                    );
                }
                return MoltObject::from_float(lf.powf(rf)).bits();
            }
            return raise_exception::<_>(_py, "OverflowError", "exponent too large");
        }
        if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
            if lf == 0.0 && rf < 0.0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "0.0 cannot be raised to a negative power",
                );
            }
            if lf < 0.0 && rf.is_finite() && rf.fract() != 0.0 {
                let base = ComplexParts { re: lf, im: 0.0 };
                let exp = ComplexParts { re: rf, im: 0.0 };
                if let Ok(out) = complex_pow(base, exp) {
                    return complex_bits(_py, out.re, out.im);
                }
            }
            let out = lf.powf(rf);
            if out.is_infinite() && lf.is_finite() && rf.is_finite() {
                return raise_exception::<_>(_py, "OverflowError", "math range error");
            }
            return MoltObject::from_float(out).bits();
        }
        raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for **")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_pow(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let ipow_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ipow_name, b"__ipow__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ipow_name_bits) {
                return res_bits;
            }
        }
        molt_pow(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_pow_mod(a: u64, b: u64, m: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let mod_obj = obj_from_bits(m);
        if let (Some(li), Some(ri), Some(mi)) = (to_i64(lhs), to_i64(rhs), to_i64(mod_obj)) {
            let (base, exp, modulus) = (li as i128, ri, mi as i128);
            if modulus == 0 {
                return raise_exception::<_>(_py, "ValueError", "pow() 3rd argument cannot be 0");
            }
            let result = if exp < 0 {
                let mod_abs = modulus.abs();
                let base_mod = mod_py_i128(base, mod_abs);
                let Some(inv) = mod_inverse_i128(_py, base_mod, mod_abs) else {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "base is not invertible for the given modulus",
                    );
                };
                let inv_mod = mod_py_i128(inv, modulus);
                mod_pow_i128(_py, inv_mod, -exp, modulus)
            } else {
                mod_pow_i128(_py, base, exp, modulus)
            };
            return MoltObject::from_int(result as i64).bits();
        }
        if let (Some(base), Some(exp), Some(modulus)) =
            (to_bigint(lhs), to_bigint(rhs), to_bigint(mod_obj))
        {
            if modulus.is_zero() {
                return raise_exception::<_>(_py, "ValueError", "pow() 3rd argument cannot be 0");
            }
            let result = if exp.is_negative() {
                let mod_abs = modulus.abs();
                let base_mod = base.mod_floor(&mod_abs);
                let Some(inv) = mod_inverse_bigint(base_mod, &mod_abs) else {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "base is not invertible for the given modulus",
                    );
                };
                let inv_mod = inv.mod_floor(&modulus);
                let neg_exp = -exp;
                if neg_exp.to_u64().is_none() {
                    return raise_exception::<_>(_py, "OverflowError", "exponent too large");
                }
                let exp_u64 = neg_exp.to_u64().unwrap();
                mod_pow_bigint(&inv_mod, exp_u64, &modulus)
            } else {
                if exp.to_u64().is_none() {
                    return raise_exception::<_>(_py, "OverflowError", "exponent too large");
                }
                let exp_u64 = exp.to_u64().unwrap();
                mod_pow_bigint(&base, exp_u64, &modulus)
            };
            if let Some(i) = bigint_to_inline(&result) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, result);
        }
        raise_exception::<_>(
            _py,
            "TypeError",
            "pow() 3rd argument not allowed unless all arguments are integers",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_round(val_bits: u64, ndigits_bits: u64, has_ndigits_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let val = obj_from_bits(val_bits);
        let has_ndigits = to_i64(obj_from_bits(has_ndigits_bits)).unwrap_or(0) != 0;
        if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
            if !has_ndigits {
                return val_bits;
            }
            let ndigits_obj = obj_from_bits(ndigits_bits);
            if ndigits_obj.is_none() {
                return val_bits;
            }
            let ndigits = index_i64_from_obj(_py, ndigits_bits, "round() ndigits must be int");
            if ndigits >= 0 {
                return val_bits;
            }
            let exp = (-ndigits) as u32;
            let value = unsafe { bigint_ref(ptr).clone() };
            let pow = BigInt::from(10).pow(exp);
            if pow.is_zero() {
                return val_bits;
            }
            let div = value.div_floor(&pow);
            let rem = value.mod_floor(&pow);
            let twice = &rem * 2;
            let mut rounded = div;
            if twice > pow || (twice == pow && !rounded.is_even()) {
                if value.is_negative() {
                    rounded -= 1;
                } else {
                    rounded += 1;
                }
            }
            let result = rounded * pow;
            if let Some(i) = bigint_to_inline(&result) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, result);
        }
        if !val.is_int()
            && !val.is_bool()
            && !val.is_float()
            && let Some(ptr) = maybe_ptr_from_bits(val_bits)
        {
            unsafe {
                let round_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.round_name, b"__round__");
                if let Some(call_bits) = attr_lookup_ptr(_py, ptr, round_name_bits) {
                    let ndigits_obj = obj_from_bits(ndigits_bits);
                    let want_arg = has_ndigits && !ndigits_obj.is_none();
                    let arity = callable_arity(_py, call_bits).unwrap_or(0);
                    let res_bits = if arity <= 1 {
                        if want_arg {
                            call_callable1(_py, call_bits, ndigits_bits)
                        } else {
                            call_callable0(_py, call_bits)
                        }
                    } else {
                        let arg_bits = if want_arg {
                            ndigits_bits
                        } else {
                            MoltObject::none().bits()
                        };
                        call_callable1(_py, call_bits, arg_bits)
                    };
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        if !val.is_float() && let Some(i) = to_i64(val) {
            if !has_ndigits {
                return MoltObject::from_int(i).bits();
            }
            let ndigits_obj = obj_from_bits(ndigits_bits);
            if ndigits_obj.is_none() {
                return MoltObject::from_int(i).bits();
            }
            let Some(ndigits) = to_i64(ndigits_obj) else {
                return raise_exception::<_>(_py, "TypeError", "round() ndigits must be int");
            };
            if ndigits >= 0 {
                return MoltObject::from_int(i).bits();
            }
            let exp = (-ndigits) as u32;
            if exp > 38 {
                return MoltObject::from_int(0).bits();
            }
            let pow = 10_i128.pow(exp);
            let value = i as i128;
            if pow == 0 {
                return MoltObject::from_int(i).bits();
            }
            let div = value / pow;
            let rem = value % pow;
            let abs_rem = rem.abs();
            let twice = abs_rem.saturating_mul(2);
            let mut rounded = div;
            if twice > pow || (twice == pow && (div & 1) != 0) {
                rounded += if value >= 0 { 1 } else { -1 };
            }
            let result = rounded.saturating_mul(pow);
            return MoltObject::from_int(result as i64).bits();
        }
        if let Some(f) = to_f64(val) {
            if !has_ndigits {
                if f.is_nan() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "cannot convert float NaN to integer",
                    );
                }
                if f.is_infinite() {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "cannot convert float infinity to integer",
                    );
                }
                let rounded = round_half_even(f);
                let big = bigint_from_f64_trunc(rounded);
                if let Some(i) = bigint_to_inline(&big) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, big);
            }
            let ndigits_obj = obj_from_bits(ndigits_bits);
            if ndigits_obj.is_none() {
                if f.is_nan() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "cannot convert float NaN to integer",
                    );
                }
                if f.is_infinite() {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "cannot convert float infinity to integer",
                    );
                }
                let rounded = round_half_even(f);
                let big = bigint_from_f64_trunc(rounded);
                if let Some(i) = bigint_to_inline(&big) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, big);
            }
            let Some(ndigits) = to_i64(ndigits_obj) else {
                return raise_exception::<_>(_py, "TypeError", "round() ndigits must be int");
            };
            let rounded = round_float_ndigits(f, ndigits);
            return MoltObject::from_float(rounded).bits();
        }
        raise_exception::<_>(_py, "TypeError", "round() expects a real number")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trunc(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let val = obj_from_bits(val_bits);
        if let Some(i) = to_i64(val) {
            return MoltObject::from_int(i).bits();
        }
        if bigint_ptr_from_bits(val_bits).is_some() {
            return val_bits;
        }
        if let Some(f) = to_f64(val) {
            if f.is_nan() {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "cannot convert float NaN to integer",
                );
            }
            if f.is_infinite() {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot convert float infinity to integer",
                );
            }
            let big = bigint_from_f64_trunc(f);
            if let Some(i) = bigint_to_inline(&big) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, big);
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let trunc_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.trunc_name, b"__trunc__");
                if let Some(call_bits) = attr_lookup_ptr(_py, ptr, trunc_name_bits) {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        raise_exception::<_>(_py, "TypeError", "trunc() expects a real number")
    })
}

pub(super) fn set_like_result_type_id(type_id: u32) -> u32 {
    if type_id == TYPE_ID_FROZENSET {
        TYPE_ID_FROZENSET
    } else {
        TYPE_ID_SET
    }
}

pub(super) unsafe fn set_like_new_bits(type_id: u32, capacity: usize) -> u64 {
    if type_id == TYPE_ID_FROZENSET {
        molt_frozenset_new(capacity as u64)
    } else {
        molt_set_new(capacity as u64)
    }
}

pub(super) unsafe fn set_like_union(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let res_bits = set_like_new_bits(result_type_id, l_elems.len() + r_elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in l_elems.iter() {
            set_add_in_place(_py, res_ptr, entry);
        }
        for &entry in r_elems.iter() {
            set_add_in_place(_py, res_ptr, entry);
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_intersection(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let (probe_elems, probe_table, output) = if l_elems.len() <= r_elems.len() {
            (r_elems, set_table(rhs_ptr), l_elems)
        } else {
            (l_elems, set_table(lhs_ptr), r_elems)
        };
        let res_bits = set_like_new_bits(result_type_id, output.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in output.iter() {
            let found = set_find_entry(_py, probe_elems, probe_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_some() {
                set_add_in_place(_py, res_ptr, entry);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_difference(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let r_table = set_table(rhs_ptr);
        let res_bits = set_like_new_bits(result_type_id, l_elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in l_elems.iter() {
            let found = set_find_entry(_py, r_elems, r_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_symdiff(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let l_table = set_table(lhs_ptr);
        let r_table = set_table(rhs_ptr);
        let res_bits = set_like_new_bits(result_type_id, l_elems.len() + r_elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in l_elems.iter() {
            let found = set_find_entry(_py, r_elems, r_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        for &entry in r_elems.iter() {
            let found = set_find_entry(_py, l_elems, l_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_copy_bits(_py: &PyToken<'_>, ptr: *mut u8, result_type_id: u32) -> u64 {
    unsafe {
        let elems = set_order(ptr);
        let res_bits = set_like_new_bits(result_type_id, elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in elems.iter() {
            set_add_in_place(_py, res_ptr, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
        }
        res_bits
    }
}

pub(super) unsafe fn set_like_ptr_from_bits(
    _py: &PyToken<'_>,
    other_bits: u64,
) -> Option<(*mut u8, Option<u64>)> {
    unsafe {
        let obj = obj_from_bits(other_bits);
        if let Some(ptr) = obj.as_ptr() {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_SET || type_id == TYPE_ID_FROZENSET {
                return Some((ptr, None));
            }
        }
        let set_bits = set_from_iter_bits(_py, other_bits)?;
        let ptr = obj_from_bits(set_bits).as_ptr()?;
        Some((ptr, Some(set_bits)))
    }
}

pub(super) unsafe fn set_from_iter_bits(_py: &PyToken<'_>, other_bits: u64) -> Option<u64> {
    unsafe {
        let iter_bits = molt_iter(other_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, other_bits);
        }
        let set_bits = molt_set_new(0);
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

fn binary_type_error(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject, op: &str) -> u64 {
    let msg = format!(
        "unsupported operand type(s) for {op}: '{}' and '{}'",
        type_name(_py, lhs),
        type_name(_py, rhs)
    );
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn is_union_operand(_py: &PyToken<'_>, obj: MoltObject) -> bool {
    if obj.is_none() {
        return true;
    }
    let Some(ptr) = obj.as_ptr() else {
        return false;
    };
    unsafe {
        matches!(
            object_type_id(ptr),
            TYPE_ID_TYPE | TYPE_ID_GENERIC_ALIAS | TYPE_ID_UNION
        )
    }
}

fn append_union_arg(_py: &PyToken<'_>, args: &mut Vec<u64>, candidate: u64) {
    for &existing in args.iter() {
        if obj_eq(_py, obj_from_bits(existing), obj_from_bits(candidate)) {
            return;
        }
    }
    args.push(candidate);
}

fn collect_union_args(_py: &PyToken<'_>, bits: u64, args: &mut Vec<u64>) {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        append_union_arg(_py, args, builtin_classes(_py).none_type);
        return;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_UNION {
                let args_bits = union_type_args_bits(ptr);
                let args_obj = obj_from_bits(args_bits);
                if let Some(args_ptr) = args_obj.as_ptr()
                    && object_type_id(args_ptr) == TYPE_ID_TUPLE
                {
                    let elems = seq_vec_ref(args_ptr);
                    for &elem_bits in elems.iter() {
                        append_union_arg(_py, args, elem_bits);
                    }
                    return;
                }
                append_union_arg(_py, args, args_bits);
                return;
            }
        }
    }
    append_union_arg(_py, args, bits);
}

fn build_union_type(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> u64 {
    let mut args = Vec::new();
    collect_union_args(_py, lhs.bits(), &mut args);
    collect_union_args(_py, rhs.bits(), &mut args);
    if args.len() == 1 {
        let bits = args[0];
        inc_ref_bits(_py, bits);
        return bits;
    }
    let tuple_ptr = alloc_tuple(_py, args.as_slice());
    if tuple_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let args_bits = MoltObject::from_ptr(tuple_ptr).bits();
    let union_ptr = alloc_union_type(_py, args_bits);
    if union_ptr.is_null() {
        dec_ref_bits(_py, args_bits);
        return MoltObject::none().bits();
    }
    dec_ref_bits(_py, args_bits);
    MoltObject::from_ptr(union_ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bit_or(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if lhs.is_bool() && rhs.is_bool() {
                return MoltObject::from_bool((li != 0) | (ri != 0)).bits();
            }
            let res = li | ri;
            if inline_int_from_i128(res as i128).is_some() {
                return MoltObject::from_int(res).bits();
            }
            return bigint_bits(_py, BigInt::from(li) | BigInt::from(ri));
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big | r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if is_union_operand(_py, lhs) && is_union_operand(_py, rhs) {
            return build_union_type(_py, lhs, rhs);
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    return set_like_union(_py, lp, rp, set_like_result_type_id(ltype));
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
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
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let res = set_like_union(_py, lhs_ptr, rhs_ptr, TYPE_ID_SET);
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return res;
                }
                if ltype == TYPE_ID_DICT && rtype == TYPE_ID_DICT {
                    let builtins = builtin_classes(_py);
                    let lhs_class = object_class_bits(lp);
                    let rhs_class = object_class_bits(rp);
                    let lhs_exact = lhs_class == 0 || lhs_class == builtins.dict;
                    let rhs_exact = rhs_class == 0 || rhs_class == builtins.dict;
                    if !lhs_exact || !rhs_exact {
                        // Dict subclasses must dispatch through dunder resolution.
                        // Skip the dict fast-path so __or__/__ror__ can run.
                        // (Exact dict stays on the optimized union path.)
                    } else if let (Some(lhs_bits), Some(rhs_bits)) = (
                        dict_like_bits_from_ptr(_py, lp),
                        dict_like_bits_from_ptr(_py, rp),
                    ) {
                        let out_bits = molt_dict_copy(lhs_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        let _ = molt_dict_update(out_bits, rhs_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, out_bits);
                            return MoltObject::none().bits();
                        }
                        return out_bits;
                    }
                }
            }
        }
        unsafe {
            let or_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.or_name, b"__or__");
            let ror_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ror_name, b"__ror__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, or_name_bits, ror_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "|")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_bit_or(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let rhs = obj_from_bits(b);
                    let ok = rhs
                        .as_ptr()
                        .is_some_and(|rhs_ptr| is_set_inplace_rhs_type(object_type_id(rhs_ptr)));
                    if !ok {
                        return raise_unsupported_inplace(_py, "|=", a, b);
                    }
                    let _ = molt_set_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_DICT {
                    let builtins = builtin_classes(_py);
                    let class_bits = object_class_bits(ptr);
                    let exact_dict = class_bits == 0 || class_bits == builtins.dict;
                    if exact_dict {
                        if let Some(rhs_ptr) = obj_from_bits(b).as_ptr()
                            && dict_like_bits_from_ptr(_py, rhs_ptr).is_some()
                        {
                            let _ = molt_dict_update(a, b);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            inc_ref_bits(_py, a);
                            return a;
                        }
                        return raise_unsupported_inplace(_py, "|=", a, b);
                    }
                }
            }
        }
        unsafe {
            let ior_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ior_name, b"__ior__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ior_name_bits) {
                return res_bits;
            }
        }
        molt_bit_or(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bit_and(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if lhs.is_bool() && rhs.is_bool() {
                return MoltObject::from_bool((li != 0) & (ri != 0)).bits();
            }
            let res = li & ri;
            if inline_int_from_i128(res as i128).is_some() {
                return MoltObject::from_int(res).bits();
            }
            return bigint_bits(_py, BigInt::from(li) & BigInt::from(ri));
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big & r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    return set_like_intersection(_py, lp, rp, set_like_result_type_id(ltype));
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
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
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let res = set_like_intersection(_py, lhs_ptr, rhs_ptr, TYPE_ID_SET);
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return res;
                }
            }
        }
        unsafe {
            let and_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.and_name, b"__and__");
            let rand_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rand_name, b"__rand__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, and_name_bits, rand_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "&")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_bit_and(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let rhs = obj_from_bits(b);
                    let ok = rhs
                        .as_ptr()
                        .is_some_and(|rhs_ptr| is_set_inplace_rhs_type(object_type_id(rhs_ptr)));
                    if !ok {
                        return raise_unsupported_inplace(_py, "&=", a, b);
                    }
                    let _ = molt_set_intersection_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        unsafe {
            let iand_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.iand_name, b"__iand__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, iand_name_bits) {
                return res_bits;
            }
        }
        molt_bit_and(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bit_xor(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if lhs.is_bool() && rhs.is_bool() {
                return MoltObject::from_bool((li != 0) ^ (ri != 0)).bits();
            }
            let res = li ^ ri;
            if inline_int_from_i128(res as i128).is_some() {
                return MoltObject::from_int(res).bits();
            }
            return bigint_bits(_py, BigInt::from(li) ^ BigInt::from(ri));
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big ^ r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                let ltype = object_type_id(lp);
                let rtype = object_type_id(rp);
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    return set_like_symdiff(_py, lp, rp, set_like_result_type_id(ltype));
                }
                if (is_set_like_type(ltype) || is_set_view_type(ltype))
                    && (is_set_like_type(rtype) || is_set_view_type(rtype))
                {
                    let (lhs_ptr, lhs_bits) = if is_set_like_type(ltype) {
                        (lp, None)
                    } else {
                        let Some(bits) = dict_view_as_set_bits(_py, lp, ltype) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
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
                            return MoltObject::none().bits();
                        };
                        let Some(ptr) = obj_from_bits(bits).as_ptr() else {
                            if let Some(bits) = lhs_bits {
                                dec_ref_bits(_py, bits);
                            }
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        (ptr, Some(bits))
                    };
                    let res = set_like_symdiff(_py, lhs_ptr, rhs_ptr, TYPE_ID_SET);
                    if let Some(bits) = lhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = rhs_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return res;
                }
            }
        }
        unsafe {
            let xor_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.xor_name, b"__xor__");
            let rxor_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.rxor_name, b"__rxor__");
            if let Some(res_bits) = call_binary_dunder(_py, a, b, xor_name_bits, rxor_name_bits) {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "^")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_invert(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            let res = -(i as i128) - 1;
            return int_bits_from_i128(_py, res);
        }
        if let Some(big) = to_bigint(obj) {
            let res = -big - BigInt::from(1);
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        let msg = format!("bad operand type for unary ~: '{}'", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_neg(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(i) = to_i64(obj) {
            let res = -(i as i128);
            return int_bits_from_i128(_py, res);
        }
        if let Some(big) = to_bigint(obj) {
            let res = -big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some(f) = to_f64(obj) {
            return MoltObject::from_float(-f).bits();
        }
        if let Some(ptr) = complex_ptr_from_bits(val) {
            let value = unsafe { *complex_ref(ptr) };
            return complex_bits(_py, -value.re, -value.im);
        }
        if let Some(ptr) = maybe_ptr_from_bits(val)
            && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__neg__")
        {
            unsafe {
                let call_bits = attr_lookup_ptr(_py, ptr, name_bits);
                dec_ref_bits(_py, name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val));
        let msg = format!("bad operand type for unary -: '{type_name}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_bit_xor(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        if let Some(ptr) = lhs.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let rhs = obj_from_bits(b);
                    let ok = rhs
                        .as_ptr()
                        .is_some_and(|rhs_ptr| is_set_inplace_rhs_type(object_type_id(rhs_ptr)));
                    if !ok {
                        return raise_unsupported_inplace(_py, "^=", a, b);
                    }
                    let _ = molt_set_symdiff_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, a);
                    return a;
                }
            }
        }
        unsafe {
            let ixor_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.ixor_name, b"__ixor__");
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ixor_name_bits) {
                return res_bits;
            }
        }
        molt_bit_xor(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let shift = index_i64_from_obj(_py, b, "shift count must be int");
        if shift < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative shift count");
        }
        let shift_u = shift as u32;
        if let Some(value) = to_i64(lhs) {
            if shift_u >= 63 {
                return bigint_bits(_py, BigInt::from(value) << shift_u);
            }
            let res = value << shift_u;
            if inline_int_from_i128(res as i128).is_some() {
                return MoltObject::from_int(res).bits();
            }
            return bigint_bits(_py, BigInt::from(value) << shift_u);
        }
        if let Some(value) = to_bigint(lhs) {
            let res = value << shift_u;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        binary_type_error(_py, lhs, rhs, "<<")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_lshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let ilshift_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.ilshift_name,
                b"__ilshift__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, ilshift_name_bits) {
                return res_bits;
            }
        }
        molt_lshift(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_rshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let shift = index_i64_from_obj(_py, b, "shift count must be int");
        if shift < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative shift count");
        }
        let shift_u = shift as u32;
        if let Some(value) = to_i64(lhs) {
            let res = if shift_u >= 63 {
                if value >= 0 { 0 } else { -1 }
            } else {
                value >> shift_u
            };
            return MoltObject::from_int(res).bits();
        }
        if let Some(value) = to_bigint(lhs) {
            let res = value >> shift_u;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        binary_type_error(_py, lhs, rhs, ">>")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_rshift(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let irshift_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.irshift_name,
                b"__irshift__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, irshift_name_bits) {
                return res_bits;
            }
        }
        molt_rshift(a, b)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_matmul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(lp), Some(rp)) = (lhs.as_ptr(), rhs.as_ptr()) {
            unsafe {
                if object_type_id(lp) == TYPE_ID_BUFFER2D && object_type_id(rp) == TYPE_ID_BUFFER2D
                {
                    return molt_buffer2d_matmul(a, b);
                }
            }
        }
        unsafe {
            let matmul_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.matmul_name, b"__matmul__");
            let rmatmul_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.rmatmul_name,
                b"__rmatmul__",
            );
            if let Some(res_bits) =
                call_binary_dunder(_py, a, b, matmul_name_bits, rmatmul_name_bits)
            {
                return res_bits;
            }
        }
        binary_type_error(_py, lhs, rhs, "@")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_inplace_matmul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let imatmul_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.imatmul_name,
                b"__imatmul__",
            );
            if let Some(res_bits) = call_inplace_dunder(_py, a, b, imatmul_name_bits) {
                return res_bits;
            }
        }
        molt_matmul(a, b)
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_str_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    molt_inc_ref(ptr);
                    return val_bits;
                }
            }
        }
        let rendered = format_obj_str(_py, obj);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let ptr = alloc_string(_py, rendered.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_repr_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        let rendered = format_obj(_py, obj);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let ptr = alloc_string(_py, rendered.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn ascii_escape(text: &str) -> String {
    let bytes = text.as_bytes();
    // SIMD fast path: if entire string is ASCII, return as-is (common case)
    if bytes.is_ascii() {
        return text.to_string();
    }
    // Find the first non-ASCII byte using SIMD scan, copy the safe prefix in bulk
    let mut first_non_ascii = 0usize;

    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            use std::arch::aarch64::*;
            let high_bit = vdupq_n_u8(0x80);
            while first_non_ascii + 16 <= bytes.len() {
                let chunk = vld1q_u8(bytes.as_ptr().add(first_non_ascii));
                let is_non_ascii = vandq_u8(chunk, high_bit);
                if vmaxvq_u8(is_non_ascii) != 0 {
                    break;
                }
                first_non_ascii += 16;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            use std::arch::x86_64::*;
            while first_non_ascii + 16 <= bytes.len() {
                let chunk = _mm_loadu_si128(bytes.as_ptr().add(first_non_ascii) as *const __m128i);
                let mask = _mm_movemask_epi8(chunk) as u32; // high bit of each byte
                if mask != 0 {
                    break;
                }
                first_non_ascii += 16;
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            use std::arch::wasm32::*;
            let high_bit = u8x16_splat(0x80);
            while first_non_ascii + 16 <= bytes.len() {
                let chunk = v128_load(bytes.as_ptr().add(first_non_ascii) as *const v128);
                let has_high = v128_and(chunk, high_bit);
                if u8x16_bitmask(has_high) != 0 {
                    break;
                }
                first_non_ascii += 16;
            }
        }
    }

    while first_non_ascii < bytes.len() && bytes[first_non_ascii].is_ascii() {
        first_non_ascii += 1;
    }

    let mut out = String::with_capacity(text.len());
    // Copy the all-ASCII prefix in bulk
    out.push_str(&text[..first_non_ascii]);
    // Process remaining characters
    for ch in text[first_non_ascii..].chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let code = ch as u32;
            if code <= 0xff {
                out.push_str(&format!("\\x{:02x}", code));
            } else if code <= 0xffff {
                out.push_str(&format!("\\u{:04x}", code));
            } else {
                out.push_str(&format!("\\U{:08x}", code));
            }
        }
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ascii_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        let rendered = format_obj(_py, obj);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let escaped = ascii_escape(&rendered);
        let ptr = alloc_string(_py, escaped.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn format_int_base(value: &BigInt, base: u32, prefix: &str, upper: bool) -> String {
    let negative = value.is_negative();
    let mut abs_val = if negative { -value } else { value.clone() };
    if abs_val.is_zero() {
        abs_val = BigInt::from(0);
    }
    let mut digits = abs_val.to_str_radix(base);
    if upper {
        digits = digits.to_uppercase();
    }
    if negative {
        format!("-{prefix}{digits}")
    } else {
        format!("{prefix}{digits}")
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bin_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, val_bits, &msg) else {
            return MoltObject::none().bits();
        };
        let text = format_int_base(&value, 2, "0b", false);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_oct_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, val_bits, &msg) else {
            return MoltObject::none().bits();
        };
        let text = format_int_base(&value, 8, "0o", false);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hex_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("'{type_name}' object cannot be interpreted as an integer");
        let Some(value) = index_bigint_from_obj(_py, val_bits, &msg) else {
            return MoltObject::none().bits();
        };
        let text = format_int_base(&value, 16, "0x", false);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn parse_float_from_bytes(bytes: &[u8]) -> Result<f64, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let trimmed = text.trim();
    trimmed.parse::<f64>().map_err(|_| ())
}

fn parse_complex_from_str(text: &str) -> Result<ComplexParts, ()> {
    let mut trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    if trimmed.starts_with('(') && trimmed.ends_with(')') && trimmed.len() >= 2 {
        trimmed = trimmed[1..trimmed.len() - 1].trim();
        if trimmed.is_empty() {
            return Err(());
        }
    }
    if trimmed.chars().any(|ch| ch.is_whitespace()) {
        return Err(());
    }
    let bytes = trimmed.as_bytes();
    let ends_with_j = matches!(bytes.last(), Some(b'j') | Some(b'J'));
    if ends_with_j {
        let core = &trimmed[..trimmed.len() - 1];
        if core.is_empty() {
            return Ok(ComplexParts { re: 0.0, im: 1.0 });
        }
        if core == "+" {
            return Ok(ComplexParts { re: 0.0, im: 1.0 });
        }
        if core == "-" {
            return Ok(ComplexParts { re: 0.0, im: -1.0 });
        }
        let mut sep_idx = None;
        let core_bytes = core.as_bytes();
        for idx in 1..core_bytes.len() {
            let ch = core_bytes[idx] as char;
            if ch == '+' || ch == '-' {
                let prev = core_bytes[idx - 1] as char;
                if prev == 'e' || prev == 'E' {
                    continue;
                }
                sep_idx = Some(idx);
            }
        }
        if let Some(idx) = sep_idx {
            let real_part = &core[..idx];
            let imag_part = &core[idx..];
            let real = parse_float_from_bytes(real_part.as_bytes())?;
            let imag = if imag_part == "+" {
                1.0
            } else if imag_part == "-" {
                -1.0
            } else {
                parse_float_from_bytes(imag_part.as_bytes())?
            };
            return Ok(ComplexParts { re: real, im: imag });
        }
        let imag = parse_float_from_bytes(core.as_bytes())?;
        return Ok(ComplexParts { re: 0.0, im: imag });
    }
    let real = parse_float_from_bytes(trimmed.as_bytes())?;
    Ok(ComplexParts { re: real, im: 0.0 })
}

fn parse_int_from_str(text: &str, base: i64) -> Result<(BigInt, i64), ()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    let mut sign = 1i32;
    let mut digits = trimmed;
    if let Some(rest) = digits.strip_prefix('+') {
        digits = rest;
    } else if let Some(rest) = digits.strip_prefix('-') {
        digits = rest;
        sign = -1;
    }
    let mut base_val = base;
    if base_val == 0 {
        if let Some(rest) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            base_val = 16;
            digits = rest;
        } else if let Some(rest) = digits
            .strip_prefix("0o")
            .or_else(|| digits.strip_prefix("0O"))
        {
            base_val = 8;
            digits = rest;
        } else if let Some(rest) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
        {
            base_val = 2;
            digits = rest;
        } else {
            base_val = 10;
        }
    } else if base_val == 16 {
        if let Some(rest) = digits
            .strip_prefix("0x")
            .or_else(|| digits.strip_prefix("0X"))
        {
            digits = rest;
        }
    } else if base_val == 8 {
        if let Some(rest) = digits
            .strip_prefix("0o")
            .or_else(|| digits.strip_prefix("0O"))
        {
            digits = rest;
        }
    } else if base_val == 2
        && let Some(rest) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
    {
        digits = rest;
    }
    let digits = digits.replace('_', "");
    if digits.is_empty() {
        return Err(());
    }
    let parsed = BigInt::parse_bytes(digits.as_bytes(), base_val as u32).ok_or(())?;
    let parsed = if sign < 0 { -parsed } else { parsed };
    Ok((parsed, base_val))
}

/// # Safety
/// - `ptr` must be null or valid for `len_bits` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bigint_from_str(ptr: *const u8, len_bits: u64) -> u64 {
    unsafe {
        crate::with_gil_entry!(_py, {
            let len = usize_from_bits(len_bits);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            let bytes = std::slice::from_raw_parts(ptr, len);
            let text = match std::str::from_utf8(bytes) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<_>(_py, "ValueError", "invalid literal for int()");
                }
            };
            let (parsed, _base_used) = match parse_int_from_str(text, 10) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<_>(_py, "ValueError", "invalid literal for int()");
                }
            };
            if let Some(i) = bigint_to_inline(&parsed) {
                return MoltObject::from_int(i).bits();
            }
            bigint_bits(_py, parsed)
        })
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        if obj.is_float() {
            return val_bits;
        }
        if complex_ptr_from_bits(val_bits).is_some() {
            let type_label = type_name(_py, obj);
            let msg =
                format!("float() argument must be a string or a real number, not '{type_label}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if let Some(i) = to_i64(obj) {
            return MoltObject::from_float(i as f64).bits();
        }
        if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
            let big = unsafe { bigint_ref(ptr) };
            if let Some(val) = big.to_f64() {
                return MoltObject::from_float(val).bits();
            }
            return raise_exception::<_>(_py, "OverflowError", "int too large to convert to float");
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let len = string_len(ptr);
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                    if let Ok(parsed) = parse_float_from_bytes(bytes) {
                        return MoltObject::from_float(parsed).bits();
                    }
                    let rendered = String::from_utf8_lossy(bytes);
                    let msg = format!("could not convert string to float: '{rendered}'");
                    return raise_exception::<_>(_py, "ValueError", &msg);
                }
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    if let Ok(parsed) = parse_float_from_bytes(bytes) {
                        return MoltObject::from_float(parsed).bits();
                    }
                    let rendered = String::from_utf8_lossy(bytes);
                    let msg = format!("could not convert string to float: '{rendered}'");
                    return raise_exception::<_>(_py, "ValueError", &msg);
                }
                let float_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.float_name, b"__float__");
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits) {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    let res_obj = obj_from_bits(res_bits);
                    if res_obj.is_float() {
                        return res_bits;
                    }
                    let owner = class_name_for_error(type_of_bits(_py, val_bits));
                    let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let index_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits) {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    let res_obj = obj_from_bits(res_bits);
                    if let Some(i) = to_i64(res_obj) {
                        return MoltObject::from_float(i as f64).bits();
                    }
                    let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                    if res_obj.as_ptr().is_some() {
                        dec_ref_bits(_py, res_bits);
                    }
                    let msg = format!("__index__ returned non-int (type {res_type})");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        raise_exception::<_>(
            _py,
            "TypeError",
            "float() argument must be a string or a number",
        )
    })
}

fn parse_float_fromhex_text(text: &str) -> Result<f64, ()> {
    let mut src = text.trim();
    if src.is_empty() {
        return Err(());
    }
    let mut sign = 1.0f64;
    if let Some(rest) = src.strip_prefix('+') {
        src = rest;
    } else if let Some(rest) = src.strip_prefix('-') {
        src = rest;
        sign = -1.0;
    }
    if src.eq_ignore_ascii_case("inf") || src.eq_ignore_ascii_case("infinity") {
        return Ok(sign * f64::INFINITY);
    }
    if src.eq_ignore_ascii_case("nan") {
        return Ok(f64::NAN);
    }
    let Some(hex_src) = src.strip_prefix("0x").or_else(|| src.strip_prefix("0X")) else {
        return Err(());
    };
    let mut split = hex_src.split(['p', 'P']);
    let significand = split.next().ok_or(())?;
    let exponent_text = split.next().ok_or(())?;
    if split.next().is_some() {
        return Err(());
    }
    let exponent = exponent_text.parse::<i32>().map_err(|_| ())?;
    let (int_part, frac_part) = if let Some((left, right)) = significand.split_once('.') {
        (left, right)
    } else {
        (significand, "")
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return Err(());
    }
    let mut mantissa = 0.0f64;
    let mut digits = 0usize;
    for ch in int_part.bytes() {
        let Some(d) = (ch as char).to_digit(16) else {
            return Err(());
        };
        mantissa = mantissa * 16.0 + d as f64;
        digits += 1;
    }
    let mut frac_digits = 0usize;
    for ch in frac_part.bytes() {
        let Some(d) = (ch as char).to_digit(16) else {
            return Err(());
        };
        mantissa = mantissa * 16.0 + d as f64;
        digits += 1;
        frac_digits += 1;
    }
    if digits == 0 {
        return Err(());
    }
    let exp2 = exponent
        .checked_sub((frac_digits.saturating_mul(4)) as i32)
        .ok_or(())?;
    let mut out = mantissa * 2f64.powi(exp2);
    if sign.is_sign_negative() {
        out = -out;
    }
    Ok(out)
}

fn float_hex_string(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        if value.is_sign_negative() {
            return "-inf".to_string();
        }
        return "inf".to_string();
    }
    if value == 0.0 {
        if value.is_sign_negative() {
            return "-0x0.0p+0".to_string();
        }
        return "0x0.0p+0".to_string();
    }
    let bits = value.to_bits();
    let sign = if (bits >> 63) != 0 { "-" } else { "" };
    let exp_bits = ((bits >> 52) & 0x7ff) as i32;
    let frac_bits = bits & ((1u64 << 52) - 1);
    let (lead, exponent) = if exp_bits == 0 {
        (0u8, -1022)
    } else {
        (1u8, exp_bits - 1023)
    };
    format!("{sign}0x{lead:x}.{frac_bits:013x}p{exponent:+}")
}

fn float_value_or_descriptor_error(_py: &PyToken<'_>, self_bits: u64, method: &str) -> Option<f64> {
    let obj = obj_from_bits(self_bits);
    if let Some(value) = obj.as_float() {
        return Some(value);
    }
    let type_label = class_name_for_error(type_of_bits(_py, self_bits));
    let msg = format!(
        "descriptor '{method}' for 'float' objects doesn't apply to a '{type_label}' object"
    );
    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_conjugate(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "conjugate") else {
            return MoltObject::none().bits();
        };
        MoltObject::from_float(value).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_is_integer(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "is_integer") else {
            return MoltObject::none().bits();
        };
        let out = value.is_finite() && value.fract() == 0.0;
        MoltObject::from_bool(out).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_as_integer_ratio(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "as_integer_ratio")
        else {
            return MoltObject::none().bits();
        };
        if value.is_nan() {
            return raise_exception::<_>(_py, "ValueError", "cannot convert NaN to integer ratio");
        }
        if value.is_infinite() {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "cannot convert Infinity to integer ratio",
            );
        }
        if value == 0.0 {
            let zero = MoltObject::from_int(0).bits();
            let one = MoltObject::from_int(1).bits();
            let tuple_ptr = alloc_tuple(_py, &[zero, one]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let bits = value.to_bits();
        let negative = (bits >> 63) != 0;
        let exp_bits = ((bits >> 52) & 0x7ff) as i32;
        let mut mantissa = bits & ((1u64 << 52) - 1);
        let exponent = if exp_bits == 0 {
            -1022 - 52
        } else {
            mantissa |= 1u64 << 52;
            exp_bits - 1023 - 52
        };
        let mut numerator = BigInt::from(mantissa);
        if negative {
            numerator = -numerator;
        }
        let mut denominator = BigInt::from(1u8);
        if exponent >= 0 {
            numerator <<= exponent as usize;
        } else {
            denominator <<= (-exponent) as usize;
        }
        let gcd = numerator.abs().gcd(&denominator);
        if !gcd.is_zero() {
            numerator /= &gcd;
            denominator /= &gcd;
        }
        let num_bits = int_bits_from_bigint(_py, numerator);
        let den_bits = int_bits_from_bigint(_py, denominator);
        let tuple_ptr = alloc_tuple(_py, &[num_bits, den_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_hex(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value) = float_value_or_descriptor_error(_py, self_bits, "hex") else {
            return MoltObject::none().bits();
        };
        let text = float_hex_string(value);
        let ptr = alloc_string(_py, text.as_bytes());
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_fromhex(cls_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let text_obj = obj_from_bits(text_bits);
        let Some(text_ptr) = text_obj.as_ptr() else {
            let msg = format!(
                "fromhex() argument must be str, not {}",
                type_name(_py, text_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(text_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "fromhex() argument must be str, not {}",
                    type_name(_py, text_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let bytes = std::slice::from_raw_parts(string_bytes(text_ptr), string_len(text_ptr));
            let text = match std::str::from_utf8(bytes) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "invalid hexadecimal floating-point string",
                    );
                }
            };
            let value = match parse_float_fromhex_text(text) {
                Ok(val) => val,
                Err(()) => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "invalid hexadecimal floating-point string",
                    );
                }
            };
            let out_bits = MoltObject::from_float(value).bits();
            let builtins = builtin_classes(_py);
            if cls_bits == builtins.float {
                return out_bits;
            }
            if !issubclass_bits(cls_bits, builtins.float) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "fromhex() requires a float subclass",
                );
            }
            let res_bits = call_callable1(_py, cls_bits, out_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            res_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_from_number(cls_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    let msg = format!(
                        "must be real number, not {}",
                        type_name(_py, obj_from_bits(val_bits))
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if complex_ptr_from_bits(val_bits).is_some() {
            let msg = format!(
                "must be real number, not {}",
                type_name(_py, obj_from_bits(val_bits))
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let out_bits = molt_float_from_obj(val_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let builtins = builtin_classes(_py);
        if cls_bits == builtins.float {
            return out_bits;
        }
        if !issubclass_bits(cls_bits, builtins.float) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "from_number() requires a float subclass",
            );
        }
        let res_bits = unsafe { call_callable1(_py, cls_bits, out_bits) };
        dec_ref_bits(_py, out_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        res_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_from_obj(val_bits: u64, imag_bits: u64, has_imag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let has_imag = to_i64(obj_from_bits(has_imag_bits)).unwrap_or(0) != 0;
        let val_obj = obj_from_bits(val_bits);
        if !has_imag {
            if complex_ptr_from_bits(val_bits).is_some() {
                inc_ref_bits(_py, val_bits);
                return val_bits;
            }
            if let Some(f) = val_obj.as_float() {
                return complex_bits(_py, f, 0.0);
            }
            if let Some(i) = to_i64(val_obj) {
                return complex_bits(_py, i as f64, 0.0);
            }
            if let Some(ptr) = bigint_ptr_from_bits(val_bits) {
                if let Some(val) = unsafe { bigint_ref(ptr) }.to_f64() {
                    return complex_bits(_py, val, 0.0);
                }
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "int too large to convert to float",
                );
            }
            if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
                unsafe {
                    let type_id = object_type_id(ptr);
                    if type_id == TYPE_ID_STRING {
                        let len = string_len(ptr);
                        let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                        let text = match std::str::from_utf8(bytes) {
                            Ok(val) => val,
                            Err(_) => {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "complex() arg is a malformed string",
                                );
                            }
                        };
                        match parse_complex_from_str(text) {
                            Ok(parts) => {
                                return complex_bits(_py, parts.re, parts.im);
                            }
                            Err(()) => {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "complex() arg is a malformed string",
                                );
                            }
                        }
                    }
                    if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                        let type_label = type_name(_py, val_obj);
                        let msg = format!(
                            "complex() argument must be a string or a number, not {type_label}"
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__complex__") {
                        if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits)
                        {
                            let res_bits = call_callable0(_py, call_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            if complex_ptr_from_bits(res_bits).is_some() {
                                return res_bits;
                            }
                            let owner = class_name_for_error(type_of_bits(_py, val_bits));
                            let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                            if obj_from_bits(res_bits).as_ptr().is_some() {
                                dec_ref_bits(_py, res_bits);
                            }
                            let msg = format!(
                                "{owner}.__complex__ returned non-complex (type {res_type})"
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        dec_ref_bits(_py, name_bits);
                    }
                    let float_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.float_name,
                        b"__float__",
                    );
                    if let Some(call_bits) =
                        attr_lookup_ptr_allow_missing(_py, ptr, float_name_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(f) = res_obj.as_float() {
                            return complex_bits(_py, f, 0.0);
                        }
                        let owner = class_name_for_error(type_of_bits(_py, val_bits));
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        let msg = format!("{owner}.__float__ returned non-float (type {res_type})");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let index_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.index_name,
                        b"__index__",
                    );
                    if let Some(call_bits) =
                        attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(i) = to_i64(res_obj) {
                            return complex_bits(_py, i as f64, 0.0);
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        let msg = format!("__index__ returned non-int (type {res_type})");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
            }
            return raise_exception::<_>(
                _py,
                "TypeError",
                "complex() argument must be a string or a number",
            );
        }
        let imag_obj = obj_from_bits(imag_bits);
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    let type_label = type_name(_py, val_obj);
                    let msg = format!(
                        "complex() argument 'real' must be a real number, not {type_label}"
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        if let Some(ptr) = maybe_ptr_from_bits(imag_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    let type_label = type_name(_py, imag_obj);
                    let msg = format!(
                        "complex() argument 'imag' must be a real number, not {type_label}"
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        let real = match complex_from_obj_strict(_py, val_obj) {
            Ok(Some(val)) => val,
            Ok(None) => {
                let type_label = type_name(_py, val_obj);
                let msg =
                    format!("complex() argument 'real' must be a real number, not {type_label}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            Err(()) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "int too large to convert to float",
                );
            }
        };
        let imag = match complex_from_obj_strict(_py, imag_obj) {
            Ok(Some(val)) => val,
            Ok(None) => {
                let type_label = type_name(_py, imag_obj);
                let msg =
                    format!("complex() argument 'imag' must be a real number, not {type_label}");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            Err(()) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "int too large to convert to float",
                );
            }
        };
        let re = real.re - imag.im;
        let im = real.im + imag.re;
        complex_bits(_py, re, im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_conjugate(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = complex_ptr_from_bits(val_bits) else {
            return raise_exception::<_>(_py, "TypeError", "complex.conjugate expects complex");
        };
        let value = unsafe { *complex_ref(ptr) };
        complex_bits(_py, value.re, -value.im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_from_number(cls_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    let msg = format!(
                        "must be real number, not {}",
                        type_name(_py, obj_from_bits(val_bits))
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        }
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        let out_bits = molt_complex_from_obj(val_bits, none_bits, false_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let builtins = builtin_classes(_py);
        if cls_bits == builtins.complex {
            return out_bits;
        }
        if !issubclass_bits(cls_bits, builtins.complex) {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "from_number() requires a complex subclass",
            );
        }
        let res_bits = unsafe { call_callable1(_py, cls_bits, out_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        res_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_new(cls_bits: u64, val_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "int.__new__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "int.__new__ expects type");
            }
        }
        let has_base = base_bits != missing_bits(_py);
        let has_base_bits = MoltObject::from_int(if has_base { 1 } else { 0 }).bits();
        let int_bits = molt_int_from_obj(val_bits, base_bits, has_base_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let builtins = builtin_classes(_py);
        if cls_bits == builtins.int {
            return int_bits;
        }
        if !issubclass_bits(cls_bits, builtins.int) {
            let type_label = class_name_for_error(cls_bits);
            let msg = format!("int.__new__ expects type, got {}", type_label);
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let inst_bits = unsafe { alloc_instance_for_class(_py, cls_ptr) };
        let Some(inst_ptr) = obj_from_bits(inst_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let Some(slot_name_bits) = attr_name_bits_from_bytes(_py, b"__molt_int_value__") else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "int subclass layout missing value slot",
            );
        };
        let Some(offset) = (unsafe { class_field_offset(_py, cls_ptr, slot_name_bits) }) else {
            dec_ref_bits(_py, slot_name_bits);
            return raise_exception::<_>(
                _py,
                "TypeError",
                "int subclass layout missing value slot",
            );
        };
        dec_ref_bits(_py, slot_name_bits);
        unsafe {
            let _ = object_field_init_ptr_raw(_py, inst_ptr, offset, int_bits);
        }
        inst_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_int(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(self_bits);
        if obj.is_int() {
            return self_bits;
        }
        if obj.is_bool() {
            return MoltObject::from_int(if obj.as_bool().unwrap_or(false) { 1 } else { 0 }).bits();
        }
        if bigint_ptr_from_bits(self_bits).is_some() {
            inc_ref_bits(_py, self_bits);
            return self_bits;
        }
        if let Some(bits) = int_subclass_value_bits_raw(self_bits) {
            if obj_from_bits(bits).as_ptr().is_some() {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }
        let type_label = class_name_for_error(type_of_bits(_py, self_bits));
        let msg = format!(
            "descriptor '__int__' requires a 'int' object but received '{}'",
            type_label
        );
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_index(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(self_bits);
        if obj.is_int() {
            return self_bits;
        }
        if obj.is_bool() {
            return MoltObject::from_int(if obj.as_bool().unwrap_or(false) { 1 } else { 0 }).bits();
        }
        if bigint_ptr_from_bits(self_bits).is_some() {
            inc_ref_bits(_py, self_bits);
            return self_bits;
        }
        if let Some(bits) = int_subclass_value_bits_raw(self_bits) {
            if obj_from_bits(bits).as_ptr().is_some() {
                inc_ref_bits(_py, bits);
            }
            return bits;
        }
        let type_label = class_name_for_error(type_of_bits(_py, self_bits));
        let msg = format!(
            "descriptor '__index__' requires a 'int' object but received '{}'",
            type_label
        );
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_bit_length(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(value) = to_bigint(obj) else {
            let type_label = class_name_for_error(type_of_bits(_py, self_bits));
            let msg = format!(
                "descriptor 'bit_length' requires a 'int' object but received '{}'",
                type_label
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        let (_sign, bytes) = value.to_bytes_be();
        if bytes.is_empty() {
            return MoltObject::from_int(0).bits();
        }
        let lead = bytes[0];
        let lead_bits = 8usize.saturating_sub(lead.leading_zeros() as usize);
        let total_bits = (bytes.len().saturating_sub(1) * 8) + lead_bits;
        MoltObject::from_int(total_bits as i64).bits()
    })
}

fn int_method_value_bits_or_error(_py: &PyToken<'_>, self_bits: u64, method: &str) -> Option<u64> {
    let obj = obj_from_bits(self_bits);
    if obj.is_int() {
        return Some(self_bits);
    }
    if obj.is_bool() {
        return Some(
            MoltObject::from_int(if obj.as_bool().unwrap_or(false) { 1 } else { 0 }).bits(),
        );
    }
    if bigint_ptr_from_bits(self_bits).is_some() {
        return Some(self_bits);
    }
    if let Some(bits) = int_subclass_value_bits_raw(self_bits) {
        return Some(bits);
    }
    let type_label = class_name_for_error(type_of_bits(_py, self_bits));
    let msg = format!(
        "descriptor '{method}' requires a 'int' object but received '{}'",
        type_label
    );
    let _ = raise_exception::<u64>(_py, "TypeError", &msg);
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_bit_count(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(value_bits) = int_method_value_bits_or_error(_py, self_bits, "bit_count") else {
            return MoltObject::none().bits();
        };
        let value_obj = obj_from_bits(value_bits);
        if let Some(i) = to_i64(value_obj) {
            let count = i.unsigned_abs().count_ones() as i64;
            return MoltObject::from_int(count).bits();
        }
        if let Some(ptr) = bigint_ptr_from_bits(value_bits) {
            let abs = unsafe { bigint_ref(ptr) }.abs();
            let (_sign, bytes) = abs.to_bytes_le();
            let mut count = 0i64;
            for byte in bytes {
                count += byte.count_ones() as i64;
            }
            return MoltObject::from_int(count).bits();
        }
        // int subclasses should always lower to int/bigint storage.
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_as_integer_ratio(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(num_bits) = int_method_value_bits_or_error(_py, self_bits, "as_integer_ratio")
        else {
            return MoltObject::none().bits();
        };
        let one_bits = MoltObject::from_int(1).bits();
        let tuple_ptr = alloc_tuple(_py, &[num_bits, one_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_conjugate(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(out_bits) = int_method_value_bits_or_error(_py, self_bits, "conjugate") else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(out_bits).as_ptr().is_some() {
            inc_ref_bits(_py, out_bits);
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_is_integer(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if int_method_value_bits_or_error(_py, self_bits, "is_integer").is_none() {
            return MoltObject::none().bits();
        }
        MoltObject::from_bool(true).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_from_obj(val_bits: u64, base_bits: u64, has_base_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        let has_base = to_i64(obj_from_bits(has_base_bits)).unwrap_or(0) != 0;
        let base_val = if has_base {
            let base = index_i64_from_obj(_py, base_bits, "int() base must be int");
            if base != 0 && !(2..=36).contains(&base) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "base must be 0 or between 2 and 36",
                );
            }
            base
        } else {
            10
        };
        let invalid_literal = |base: i64, literal: &str| -> u64 {
            let msg = format!("invalid literal for int() with base {base}: '{literal}'");
            raise_exception::<_>(_py, "ValueError", &msg)
        };
        if has_base {
            let Some(ptr) = maybe_ptr_from_bits(val_bits) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "int() can't convert non-string with explicit base",
                );
            };
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id != TYPE_ID_STRING
                    && type_id != TYPE_ID_BYTES
                    && type_id != TYPE_ID_BYTEARRAY
                {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "int() can't convert non-string with explicit base",
                    );
                }
            }
        }
        if !has_base {
            if complex_ptr_from_bits(val_bits).is_some() {
                let type_label = type_name(_py, obj);
                let msg = format!(
                    "int() argument must be a string, a bytes-like object or a real number, not '{type_label}'"
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            if let Some(i) = to_i64(obj) {
                return MoltObject::from_int(i).bits();
            }
            if bigint_ptr_from_bits(val_bits).is_some() {
                return val_bits;
            }
            if let Some(f) = to_f64(obj) {
                if f.is_nan() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "cannot convert float NaN to integer",
                    );
                }
                if f.is_infinite() {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "cannot convert float infinity to integer",
                    );
                }
                let big = bigint_from_f64_trunc(f);
                if let Some(i) = bigint_to_inline(&big) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, big);
            }
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let len = string_len(ptr);
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                    let text = match std::str::from_utf8(bytes) {
                        Ok(val) => val,
                        Err(_) => return invalid_literal(base_val, "<bytes>"),
                    };
                    let base = if has_base { base_val } else { 10 };
                    let (parsed, _base_used) = match parse_int_from_str(text, base) {
                        Ok(val) => val,
                        Err(_) => return invalid_literal(base, text),
                    };
                    if let Some(i) = bigint_to_inline(&parsed) {
                        return MoltObject::from_int(i).bits();
                    }
                    return bigint_bits(_py, parsed);
                }
                if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    let text = String::from_utf8_lossy(bytes);
                    let base = if has_base { base_val } else { 10 };
                    let (parsed, _base_used) = match parse_int_from_str(&text, base) {
                        Ok(val) => val,
                        Err(_) => return invalid_literal(base, &format!("b'{text}'")),
                    };
                    if let Some(i) = bigint_to_inline(&parsed) {
                        return MoltObject::from_int(i).bits();
                    }
                    return bigint_bits(_py, parsed);
                }
                if !has_base {
                    let int_name_bits =
                        intern_static_name(_py, &runtime_state(_py).interned.int_name, b"__int__");
                    if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, int_name_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(i) = to_i64(res_obj) {
                            return MoltObject::from_int(i).bits();
                        }
                        if bigint_ptr_from_bits(res_bits).is_some() {
                            return res_bits;
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        let msg = format!("__int__ returned non-int (type {res_type})");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let index_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.index_name,
                        b"__index__",
                    );
                    if let Some(call_bits) =
                        attr_lookup_ptr_allow_missing(_py, ptr, index_name_bits)
                    {
                        let res_bits = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        let res_obj = obj_from_bits(res_bits);
                        if let Some(i) = to_i64(res_obj) {
                            return MoltObject::from_int(i).bits();
                        }
                        if bigint_ptr_from_bits(res_bits).is_some() {
                            return res_bits;
                        }
                        let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                        if res_obj.as_ptr().is_some() {
                            dec_ref_bits(_py, res_bits);
                        }
                        let msg = format!("__index__ returned non-int (type {res_type})");
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
            }
        }
        if has_base {
            return raise_exception::<_>(_py, "ValueError", "invalid literal for int()");
        }
        raise_exception::<_>(
            _py,
            "TypeError",
            "int() argument must be a string or a number",
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_guard_type(val_bits: u64, expected_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let expected = match to_i64(obj_from_bits(expected_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "guard type tag must be int"),
        };
        if expected == TYPE_TAG_ANY {
            return val_bits;
        }
        let obj = obj_from_bits(val_bits);
        let matches = match expected {
            TYPE_TAG_INT => obj.is_int() || bigint_ptr_from_bits(val_bits).is_some(),
            TYPE_TAG_FLOAT => obj.is_float(),
            TYPE_TAG_COMPLEX => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_COMPLEX }),
            TYPE_TAG_BOOL => obj.is_bool(),
            TYPE_TAG_NONE => obj.is_none(),
            TYPE_TAG_STR => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING }),
            TYPE_TAG_BYTES => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTES }),
            TYPE_TAG_BYTEARRAY => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTEARRAY }),
            TYPE_TAG_LIST => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_LIST }),
            TYPE_TAG_TUPLE => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TUPLE }),
            TYPE_TAG_INTARRAY => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_INTARRAY }),
            TYPE_TAG_DICT => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DICT }),
            TYPE_TAG_SET => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_SET }),
            TYPE_TAG_FROZENSET => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_FROZENSET }),
            TYPE_TAG_RANGE => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_RANGE }),
            TYPE_TAG_SLICE => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_SLICE }),
            TYPE_TAG_DATACLASS => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_DATACLASS }),
            TYPE_TAG_BUFFER2D => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_BUFFER2D }),
            TYPE_TAG_MEMORYVIEW => obj
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_MEMORYVIEW }),
            _ => false,
        };
        if !matches {
            profile_hit_unchecked(&GUARD_TAG_TYPE_MISMATCH_DEOPT_COUNT);
            return raise_exception::<_>(_py, "TypeError", "type guard mismatch");
        }
        val_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is_truthy(val: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        if is_truthy(_py, obj_from_bits(val)) {
            1
        } else {
            0
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_not(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(!is_truthy(_py, obj_from_bits(val))).bits()
    })
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name)
        .map(|val| !val.is_empty() && val != "0")
        .unwrap_or(false)
}

fn maybe_emit_runtime_feedback_file(payload: &serde_json::Value) {
    if !env_flag_enabled("MOLT_RUNTIME_FEEDBACK") {
        return;
    }
    let out_path = std::env::var("MOLT_RUNTIME_FEEDBACK_FILE")
        .ok()
        .filter(|val| !val.is_empty())
        .unwrap_or_else(|| "molt_runtime_feedback.json".to_string());
    let path = std::path::Path::new(&out_path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        eprintln!(
            "molt_runtime_feedback_error stage=create_dir path={} err={}",
            path.display(),
            err
        );
        return;
    }
    let encoded = match serde_json::to_string_pretty(payload) {
        Ok(value) => value,
        Err(err) => {
            eprintln!(
                "molt_runtime_feedback_error stage=encode path={} err={}",
                path.display(),
                err
            );
            return;
        }
    };
    if let Err(err) = std::fs::write(path, encoded) {
        eprintln!(
            "molt_runtime_feedback_error stage=write path={} err={}",
            path.display(),
            err
        );
        return;
    }
    eprintln!("molt_runtime_feedback_file {}", path.display());
}

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

enum SliceError {
    Type,
    Value,
}

fn slice_error(_py: &PyToken<'_>, err: SliceError) -> u64 {
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

fn decode_slice_bound(
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

fn normalize_slice_indices(
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

fn collect_slice_indices(start: isize, stop: isize, step: isize) -> Vec<usize> {
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

fn collect_iterable_values(_py: &PyToken<'_>, bits: u64, err_msg: &str) -> Option<Vec<u64>> {
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
                    return MoltObject::from_int(code.to_u32() as i64).bits();
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_slots_init(count: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if runtime_state(_py).code_slots.get().is_some() {
            return MoltObject::none().bits();
        }
        let Some(count) = usize::try_from(count).ok() else {
            return raise_exception::<_>(_py, "MemoryError", "code slot count too large");
        };
        let slots = (0..count).map(|_| AtomicU64::new(0)).collect();
        let _ = runtime_state(_py).code_slots.set(slots);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_code_slot_set(code_id: u64, code_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(slots) = runtime_state(_py).code_slots.get() else {
            return raise_exception::<_>(_py, "RuntimeError", "code slots not initialized");
        };
        let Some(idx) = usize::try_from(code_id).ok() else {
            return raise_exception::<_>(_py, "IndexError", "code slot out of range");
        };
        if idx >= slots.len() {
            return raise_exception::<_>(_py, "IndexError", "code slot out of range");
        }
        if let Some(ptr) = obj_from_bits(code_bits).as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_CODE {
                    return raise_exception::<_>(_py, "TypeError", "code slot expects code object");
                }
            }
        } else {
            return raise_exception::<_>(_py, "TypeError", "code slot expects code object");
        }
        if code_bits != 0 {
            inc_ref_bits(_py, code_bits);
        }
        let old_bits = slots[idx].swap(code_bits, AtomicOrdering::AcqRel);
        if old_bits != 0 {
            dec_ref_bits(_py, old_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_enter(func_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut code_bits = MoltObject::none().bits();
        let func_obj = obj_from_bits(func_bits);
        if let Some(func_ptr) = func_obj.as_ptr() {
            unsafe {
                match object_type_id(func_ptr) {
                    TYPE_ID_FUNCTION => {
                        code_bits = ensure_function_code_bits(_py, func_ptr);
                    }
                    TYPE_ID_BOUND_METHOD => {
                        let bound_func_bits = bound_method_func_bits(func_ptr);
                        if let Some(bound_ptr) = obj_from_bits(bound_func_bits).as_ptr()
                            && object_type_id(bound_ptr) == TYPE_ID_FUNCTION
                        {
                            code_bits = ensure_function_code_bits(_py, bound_ptr);
                        }
                    }
                    _ => {}
                }
            }
        }
        frame_stack_push(_py, code_bits);
        code_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_enter_slot(code_id: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(slots) = runtime_state(_py).code_slots.get() else {
            return MoltObject::none().bits();
        };
        let Some(idx) = usize::try_from(code_id).ok() else {
            return MoltObject::none().bits();
        };
        let code_bits = if idx < slots.len() {
            slots[idx].load(AtomicOrdering::Acquire)
        } else {
            MoltObject::none().bits()
        };
        frame_stack_push(_py, code_bits);
        code_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_exit() -> u64 {
    crate::with_gil_entry!(_py, {
        frame_stack_pop(_py);
        MoltObject::none().bits()
    })
}

/// Outlined guarded-call helper: performs recursion guard enter/exit, optional
/// trace enter/exit, and the actual function call via function pointer dispatch.
/// Replaces the multi-block inline sequence previously generated for every
/// `call` op, eliminating ~3 Cranelift blocks and ~12 function-declaration/import
/// operations per call site.
#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn molt_guarded_call(
    fn_ptr: u64,
    args_ptr: *const u64,
    nargs: u64,
    code_id: i64,
) -> u64 {
    if !recursion_guard_enter() {
        crate::with_gil_entry!(_py, {
            return raise_exception::<u64>(
                _py, "RecursionError", "maximum recursion depth exceeded",
            );
        });
    }
    if code_id >= 0 {
        crate::with_gil_entry!(_py, {
            if let Some(slots) = runtime_state(_py).code_slots.get() {
                let idx = code_id as usize;
                let code_bits = if idx < slots.len() {
                    slots[idx].load(AtomicOrdering::Acquire)
                } else { MoltObject::none().bits() };
                frame_stack_push(_py, code_bits);
            } else {
                frame_stack_push(_py, MoltObject::none().bits());
            }
        });
    }
    let result: u64 = unsafe {
        let n = nargs as usize;
        molt_guarded_call_dispatch(fn_ptr, args_ptr, n)
    };
    if code_id >= 0 {
        crate::with_gil_entry!(_py, { frame_stack_pop(_py); });
    }
    recursion_guard_exit();
    result
}

/// Outlined guarded-call helper for dynamic dispatch paths where the callee
/// is identified by its object bits rather than a code slot id.
#[unsafe(no_mangle)]
pub extern "C" fn molt_guarded_call_obj(
    fn_ptr: u64,
    args_ptr: *const u64,
    nargs: u64,
    callee_bits: u64,
) -> u64 {
    if !recursion_guard_enter() {
        crate::with_gil_entry!(_py, {
            return raise_exception::<u64>(
                _py, "RecursionError", "maximum recursion depth exceeded",
            );
        });
    }
    if callee_bits != 0 {
        crate::with_gil_entry!(_py, {
            let mut code_bits = MoltObject::none().bits();
            let func_obj = obj_from_bits(callee_bits);
            if let Some(func_ptr) = func_obj.as_ptr() {
                unsafe {
                    match object_type_id(func_ptr) {
                        TYPE_ID_FUNCTION => {
                            code_bits = ensure_function_code_bits(_py, func_ptr);
                        }
                        TYPE_ID_BOUND_METHOD => {
                            let bound_func_bits = bound_method_func_bits(func_ptr);
                            if let Some(bound_ptr) = obj_from_bits(bound_func_bits).as_ptr()
                                && object_type_id(bound_ptr) == TYPE_ID_FUNCTION
                            {
                                code_bits = ensure_function_code_bits(_py, bound_ptr);
                            }
                        }
                        _ => {}
                    }
                }
            }
            frame_stack_push(_py, code_bits);
        });
    }
    let result: u64 = unsafe {
        let n = nargs as usize;
        molt_guarded_call_dispatch(fn_ptr, args_ptr, n)
    };
    if callee_bits != 0 {
        crate::with_gil_entry!(_py, { frame_stack_pop(_py); });
    }
    recursion_guard_exit();
    result
}

/// Shared dispatch table: call fn_ptr with n arguments read from args_ptr.
#[inline(never)]
unsafe fn molt_guarded_call_dispatch(fn_ptr: u64, args_ptr: *const u64, n: usize) -> u64 {
    unsafe {
        match n {
            0 => {
                let f: extern "C" fn() -> u64 = std::mem::transmute(fn_ptr as usize);
                f()
            }
            1 => {
                let f: extern "C" fn(u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr)
            }
            2 => {
                let f: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1))
            }
            3 => {
                let f: extern "C" fn(u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2))
            }
            4 => {
                let f: extern "C" fn(u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3))
            }
            5 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4))
            }
            6 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5))
            }
            7 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6))
            }
            8 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7))
            }
            9 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7), *args_ptr.add(8))
            }
            10 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7), *args_ptr.add(8), *args_ptr.add(9))
            }
            11 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7), *args_ptr.add(8), *args_ptr.add(9), *args_ptr.add(10))
            }
            12 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7), *args_ptr.add(8), *args_ptr.add(9), *args_ptr.add(10), *args_ptr.add(11))
            }
            13 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7), *args_ptr.add(8), *args_ptr.add(9), *args_ptr.add(10), *args_ptr.add(11), *args_ptr.add(12))
            }
            14 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7), *args_ptr.add(8), *args_ptr.add(9), *args_ptr.add(10), *args_ptr.add(11), *args_ptr.add(12), *args_ptr.add(13))
            }
            15 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7), *args_ptr.add(8), *args_ptr.add(9), *args_ptr.add(10), *args_ptr.add(11), *args_ptr.add(12), *args_ptr.add(13), *args_ptr.add(14))
            }
            16 => {
                let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
                f(*args_ptr, *args_ptr.add(1), *args_ptr.add(2), *args_ptr.add(3), *args_ptr.add(4), *args_ptr.add(5), *args_ptr.add(6), *args_ptr.add(7), *args_ptr.add(8), *args_ptr.add(9), *args_ptr.add(10), *args_ptr.add(11), *args_ptr.add(12), *args_ptr.add(13), *args_ptr.add(14), *args_ptr.add(15))
            }
            _ => {
                // Arity > 16: raise a clear error instead of silently failing.
                // This path is only reachable if a function genuinely has 17+
                // parameters AND is called via the direct fn_ptr dispatch table.
                // molt_call_func_dispatch handles arbitrary arities via callargs,
                // so this should never be reached in practice.
                crate::with_gil_entry!(_py, {
                    return raise_exception::<u64>(
                        _py,
                        "RuntimeError",
                        &format!(
                            "direct dispatch does not support {} arguments; \
                             use callargs dispatch for functions with >16 parameters",
                            n
                        ),
                    );
                })
            }
        }
    }
}

/// Outlined dynamic function call dispatch for the `call_func` op.
///
/// Handles the full Python call protocol:
/// - Handle resolution (promises/futures)
/// - Bound method unwrapping (extracts self + func)
/// - Function object detection and direct fn_ptr dispatch
/// - Closure detection (delegates to callargs for closures)
/// - Arity matching with default arg handling
/// - Recursion guard and tracing
/// - Fallback to `molt_call_bind` for non-function callables
///
/// Arguments:
///   func_bits: the callable (could be function, bound method, or any callable)
///   args_ptr: pointer to array of argument bits (spilled to stack by caller)
///   nargs: number of arguments
///   code_id: unique code ID for this call site (tracing); 0 means no tracing
///
/// Returns: the call result bits
#[unsafe(no_mangle)]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "C" fn molt_call_func_dispatch(
    func_bits: u64,
    args_ptr_bits: u64,  // u64 to match WASM all-i64 ABI; cast to *const u64 below
    nargs: u64,
    code_id: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let n = nargs as usize;
        let args_ptr = args_ptr_bits as usize as *const u64;

        // Read arguments into an inline stack buffer to avoid heap allocation
        // on every function call.  Falls back to Vec only for >16 args (very rare).
        let mut inline_buf = [0u64; 16];
        let heap_args: Vec<u64>;
        let args_slice: &[u64] = if n <= 16 {
            for i in 0..n { unsafe { inline_buf[i] = *args_ptr.add(i); } }
            &inline_buf[..n]
        } else {
            heap_args = unsafe { (0..n).map(|i| *args_ptr.add(i)).collect() };
            &heap_args
        };

        // --- Step 1: Bound method unwrap ---
        // Use a [u64; 17] inline buffer for bound methods (self + up to 16 args).
        let mut bound_buf = [0u64; 17];
        let heap_bound: Vec<u64>;
        let (effective_func, effective_args): (u64, &[u64]) = unsafe {
            if let Some(ptr) = maybe_ptr_from_bits(func_bits) {
                if object_type_id(ptr) == TYPE_ID_BOUND_METHOD {
                    let inner = bound_method_func_bits(ptr);
                    let self_bits = bound_method_self_bits(ptr);
                    let combined_len = n + 1;
                    if combined_len <= 17 {
                        bound_buf[0] = self_bits;
                        for i in 0..n { bound_buf[i + 1] = args_slice[i]; }
                        (inner, &bound_buf[..combined_len])
                    } else {
                        let mut v = Vec::with_capacity(combined_len);
                        v.push(self_bits);
                        v.extend_from_slice(args_slice);
                        heap_bound = v;
                        (inner, &heap_bound)
                    }
                } else {
                    (func_bits, args_slice)
                }
            } else {
                (func_bits, args_slice)
            }
        };

        // --- Step 2: Check if it's a plain function object ---
        let func_ptr = match maybe_ptr_from_bits(effective_func) {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_FUNCTION } => ptr,
            _ => {
                // Not a function — use the generic callargs dispatch.
                return molt_call_func_via_callargs(func_bits, effective_args);
            }
        };

        // --- Step 3: Check for closure ---
        // Closures need the full callargs path for env capture setup.
        let has_closure = unsafe { function_closure_bits(func_ptr) } != 0;
        if has_closure {
            return molt_call_func_via_callargs(func_bits, effective_args);
        }

        // --- Step 4: Direct call fast path ---
        let fn_ptr_val = unsafe { function_fn_ptr(func_ptr) };
        let func_arity = unsafe { function_arity(func_ptr) } as usize;
        let eff_nargs = effective_args.len();

        if func_arity == eff_nargs {
            // Exact arity match — fast path.
            return molt_call_func_direct(
                _py, fn_ptr_val, effective_args, code_id, func_bits,
            );
        }

        // --- Step 5: Handle missing args with defaults ---
        // Use an inline [u64; 18] buffer for padded args (up to 16 effective + 2 defaults).
        // This same buffer is reused for the generic __defaults__ fallback below,
        // eliminating a second heap allocation.
        if eff_nargs < func_arity {
            let missing = func_arity - eff_nargs;
            let mut padded_buf = [0u64; 18];
            if missing <= 2 {
                let default_kind = molt_function_default_kind(effective_func);
                padded_buf[..eff_nargs].copy_from_slice(effective_args);
                let mut padded_len = eff_nargs;

                let filled = match (missing, default_kind) {
                    (1, FUNC_DEFAULT_NONE) => {
                        padded_buf[padded_len] = MoltObject::none().bits();
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_DICT_POP) => {
                        padded_buf[padded_len] = MoltObject::from_int(1).bits();
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_DICT_UPDATE) => {
                        padded_buf[padded_len] = missing_bits(_py);
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_ZERO) => {
                        padded_buf[padded_len] = MoltObject::from_int(0).bits();
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_NEG_ONE) => {
                        padded_buf[padded_len] = MoltObject::from_int(-1).bits();
                        padded_len += 1;
                        true
                    }
                    (1, FUNC_DEFAULT_MISSING) => {
                        padded_buf[padded_len] = missing_bits(_py);
                        padded_len += 1;
                        true
                    }
                    (2, FUNC_DEFAULT_NONE2) => {
                        padded_buf[padded_len] = MoltObject::none().bits();
                        padded_buf[padded_len + 1] = MoltObject::none().bits();
                        padded_len += 2;
                        true
                    }
                    (2, FUNC_DEFAULT_DICT_POP) => {
                        padded_buf[padded_len] = MoltObject::none().bits();
                        padded_buf[padded_len + 1] = MoltObject::from_int(0).bits();
                        padded_len += 2;
                        true
                    }
                    _ => false,
                };

                if filled {
                    return molt_call_func_direct(
                        _py, fn_ptr_val, &padded_buf[..padded_len], code_id, func_bits,
                    );
                }
            }

            // Generic fallback: consult __defaults__ tuple on the function.
            // This handles user-defined functions with keyword default
            // arguments (e.g. `def f(a, b, lo=0, hi=100)`) that the compact
            // default_kind encoding cannot represent.
            // Reuses padded_buf from above to avoid a second heap allocation.
            unsafe {
                let defaults_bits = function_attr_bits(
                    _py,
                    func_ptr,
                    intern_static_name(
                        _py,
                        &runtime_state(_py).interned.defaults_name,
                        b"__defaults__",
                    ),
                );
                if let Some(dbits) = defaults_bits {
                    if !obj_from_bits(dbits).is_none() {
                        if let Some(def_ptr) = obj_from_bits(dbits).as_ptr() {
                            if object_type_id(def_ptr) == TYPE_ID_TUPLE {
                                let defaults = seq_vec_ref(def_ptr);
                                let n_defaults = defaults.len();
                                if missing <= n_defaults {
                                    let total = eff_nargs + missing;
                                    if total <= 18 {
                                        // Reuse the stack-allocated padded_buf.
                                        padded_buf[..eff_nargs].copy_from_slice(effective_args);
                                        let start = n_defaults - missing;
                                        for i in 0..missing {
                                            padded_buf[eff_nargs + i] = defaults[start + i];
                                        }
                                        return molt_call_func_direct(
                                            _py, fn_ptr_val, &padded_buf[..total], code_id, func_bits,
                                        );
                                    } else {
                                        // >18 padded args: fall back to Vec (extremely rare).
                                        let mut padded = Vec::with_capacity(total);
                                        padded.extend_from_slice(effective_args);
                                        let start = n_defaults - missing;
                                        for i in start..n_defaults {
                                            padded.push(defaults[i]);
                                        }
                                        return molt_call_func_direct(
                                            _py, fn_ptr_val, &padded, code_id, func_bits,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Arity mismatch we can't handle inline — fallback.
        molt_call_func_via_callargs(func_bits, effective_args)
    })
}

/// Direct function call through fn_ptr with recursion guard and optional tracing.
fn molt_call_func_direct(
    _py: &crate::concurrency::PyToken<'_>,
    fn_ptr: u64,
    args: &[u64],
    code_id: u64,
    callable_bits: u64,
) -> u64 {
    if !recursion_guard_enter() {
        return raise_exception::<u64>(
            _py, "RecursionError", "maximum recursion depth exceeded",
        );
    }
    if code_id != 0 {
        if let Some(func_ptr) = obj_from_bits(callable_bits).as_ptr() {
            unsafe {
                let code_bits = match object_type_id(func_ptr) {
                    TYPE_ID_FUNCTION => ensure_function_code_bits(_py, func_ptr),
                    TYPE_ID_BOUND_METHOD => {
                        let bf = bound_method_func_bits(func_ptr);
                        if let Some(bp) = obj_from_bits(bf).as_ptr() {
                            if object_type_id(bp) == TYPE_ID_FUNCTION {
                                ensure_function_code_bits(_py, bp)
                            } else {
                                MoltObject::none().bits()
                            }
                        } else {
                            MoltObject::none().bits()
                        }
                    }
                    _ => MoltObject::none().bits(),
                };
                frame_stack_push(_py, code_bits);
            }
        }
    }
    let result = unsafe { molt_guarded_call_dispatch(fn_ptr, args.as_ptr(), args.len()) };
    if code_id != 0 {
        frame_stack_pop(_py);
    }
    recursion_guard_exit();
    result
}

/// Ultra-fast inline dispatch for `call_func` with known small arities.
///
/// These functions receive args as register values (no stack spill/reload),
/// skip GIL re-acquisition (caller already holds it in the compiled code
/// context), and do a minimal type check + direct fn_ptr call.
///
/// Fast path: func_bits is a non-closure TYPE_ID_FUNCTION with exact arity.
/// Slow path: falls back to the full `molt_call_func_dispatch`.

/// Direct fn_ptr call for exactly 0 args — fully inlined, no match dispatch.
#[inline(always)]
unsafe fn direct_call_0(fn_ptr: u64) -> u64 {
    unsafe {
        let f: extern "C" fn() -> u64 = std::mem::transmute(fn_ptr as usize);
        f()
    }
}

/// Direct fn_ptr call for exactly 1 arg — fully inlined, no match dispatch.
#[inline(always)]
unsafe fn direct_call_1(fn_ptr: u64, a0: u64) -> u64 {
    unsafe {
        let f: extern "C" fn(u64) -> u64 = std::mem::transmute(fn_ptr as usize);
        f(a0)
    }
}

/// Direct fn_ptr call for exactly 2 args — fully inlined, no match dispatch.
#[inline(always)]
unsafe fn direct_call_2(fn_ptr: u64, a0: u64, a1: u64) -> u64 {
    unsafe {
        let f: extern "C" fn(u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
        f(a0, a1)
    }
}

/// Direct fn_ptr call for exactly 3 args — fully inlined, no match dispatch.
#[inline(always)]
unsafe fn direct_call_3(fn_ptr: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    unsafe {
        let f: extern "C" fn(u64, u64, u64) -> u64 = std::mem::transmute(fn_ptr as usize);
        f(a0, a1, a2)
    }
}

/// Probe the callable: if it's a non-closure function with matching arity,
/// return Some(fn_ptr). Otherwise None.
#[inline(always)]
unsafe fn probe_simple_func(func_bits: u64, expected_arity: usize) -> Option<u64> {
    unsafe {
        let obj = obj_from_bits(func_bits);
        let ptr = obj.as_ptr()?;
        if object_type_id(ptr) != TYPE_ID_FUNCTION {
            return None;
        }
        if function_closure_bits(ptr) != 0 {
            return None;
        }
        if (function_arity(ptr) as usize) != expected_arity {
            return None;
        }
        Some(function_fn_ptr(ptr))
    }
}

/// Fast 0-argument function call. No args — minimal dispatch.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_func_fast0(func_bits: u64) -> u64 {
    unsafe {
        if let Some(fn_ptr) = probe_simple_func(func_bits, 0) {
            if !recursion_guard_enter() {
                return crate::with_gil_entry!(_py, {
                    raise_exception::<u64>(_py, "RecursionError", "maximum recursion depth exceeded")
                });
            }
            let result = direct_call_0(fn_ptr);
            recursion_guard_exit();
            return result;
        }
    }
    // Slow path
    let args: [u64; 0] = [];
    molt_call_func_dispatch(func_bits, args.as_ptr() as u64, 0, 0)
}

/// Fast 1-argument function call. Args passed in registers — no stack spill.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_func_fast1(func_bits: u64, a0: u64) -> u64 {
    unsafe {
        if let Some(fn_ptr) = probe_simple_func(func_bits, 1) {
            if !recursion_guard_enter() {
                return crate::with_gil_entry!(_py, {
                    raise_exception::<u64>(_py, "RecursionError", "maximum recursion depth exceeded")
                });
            }
            let result = direct_call_1(fn_ptr, a0);
            recursion_guard_exit();
            return result;
        }
    }
    // Slow path
    let args = [a0];
    molt_call_func_dispatch(func_bits, args.as_ptr() as u64, 1, 0)
}

/// Fast 2-argument function call. Args passed in registers — no stack spill.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_func_fast2(func_bits: u64, a0: u64, a1: u64) -> u64 {
    unsafe {
        if let Some(fn_ptr) = probe_simple_func(func_bits, 2) {
            if !recursion_guard_enter() {
                return crate::with_gil_entry!(_py, {
                    raise_exception::<u64>(_py, "RecursionError", "maximum recursion depth exceeded")
                });
            }
            let result = direct_call_2(fn_ptr, a0, a1);
            recursion_guard_exit();
            return result;
        }
    }
    // Slow path
    let args = [a0, a1];
    molt_call_func_dispatch(func_bits, args.as_ptr() as u64, 2, 0)
}

/// Fast 3-argument function call. Args passed in registers — no stack spill.
#[unsafe(no_mangle)]
pub extern "C" fn molt_call_func_fast3(func_bits: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    unsafe {
        if let Some(fn_ptr) = probe_simple_func(func_bits, 3) {
            if !recursion_guard_enter() {
                return crate::with_gil_entry!(_py, {
                    raise_exception::<u64>(_py, "RecursionError", "maximum recursion depth exceeded")
                });
            }
            let result = direct_call_3(fn_ptr, a0, a1, a2);
            recursion_guard_exit();
            return result;
        }
    }
    // Slow path
    let args = [a0, a1, a2];
    molt_call_func_dispatch(func_bits, args.as_ptr() as u64, 3, 0)
}

/// Fallback: build a CallArgs and dispatch through `molt_call_bind`.
fn molt_call_func_via_callargs(callable_bits: u64, args: &[u64]) -> u64 {
    let nargs = args.len() as u64;
    let pos_cap = MoltObject::from_int(nargs as i64).bits();
    let kw_cap = MoltObject::from_int(0).bits();
    let callargs_bits = molt_callargs_new(pos_cap, kw_cap);
    for &arg in args {
        unsafe { molt_callargs_push_pos(callargs_bits, arg) };
    }
    molt_call_bind(callable_bits, callargs_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_trace_set_line(line_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let line_obj = obj_from_bits(line_bits);
        let line = if line_obj.is_int() || line_obj.is_bool() {
            to_i64(line_obj).unwrap_or(0)
        } else {
            line_bits as i64
        };
        frame_stack_set_line(line);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_repr_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_repr_from_obj(val_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_format_builtin(val_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        let spec_obj = obj_from_bits(spec_bits);
        let Some(spec_ptr) = spec_obj.as_ptr() else {
            let msg = format!(
                "format() argument 2 must be str, not {}",
                type_name(_py, spec_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(spec_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "format() argument 2 must be str, not {}",
                    type_name(_py, spec_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let spec_text = string_obj_to_owned(spec_obj).unwrap_or_default();
        if let Some(obj_ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_OBJECT || type_id == TYPE_ID_DATACLASS {
                    let class_bits = object_class_bits(obj_ptr);
                    if class_bits != 0
                        && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                        && object_type_id(class_ptr) == TYPE_ID_TYPE
                    {
                        let format_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.format_name,
                            b"__format__",
                        );
                        if let Some(call_bits) =
                            class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), format_bits)
                        {
                            return call_callable1(_py, call_bits, spec_bits);
                        }
                    }
                }
            }
        }
        let supports_format = obj.as_int().is_some()
            || obj.as_bool().is_some()
            || obj.as_float().is_some()
            || bigint_ptr_from_bits(obj.bits()).is_some()
            || obj
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
                .unwrap_or(false)
            || obj
                .as_ptr()
                .map(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_COMPLEX })
                .unwrap_or(false);
        if supports_format {
            return molt_string_format(val_bits, spec_bits);
        }
        if spec_text.is_empty() {
            return molt_str_from_obj(val_bits);
        }
        let type_label = type_name(_py, obj);
        let msg = format!("unsupported format string passed to {type_label}.__format__");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_callable_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_is_callable(val_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_round_builtin(val_bits: u64, ndigits_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let has_ndigits = ndigits_bits != missing;
        let has_ndigits_bits = MoltObject::from_bool(has_ndigits).bits();
        let ndigits = if has_ndigits {
            ndigits_bits
        } else {
            MoltObject::none().bits()
        };
        molt_round(val_bits, ndigits, has_ndigits_bits)
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_any_builtin(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    return MoltObject::from_bool(false).bits();
                }
                if is_truthy(_py, obj_from_bits(val_bits)) {
                    return MoltObject::from_bool(true).bits();
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_all_builtin(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    return MoltObject::from_bool(true).bits();
                }
                if !is_truthy(_py, obj_from_bits(val_bits)) {
                    return MoltObject::from_bool(false).bits();
                }
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_abs_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        if let Some(i) = to_i64(obj) {
            return int_bits_from_i128(_py, (i as i128).abs());
        }
        if let Some(big) = to_bigint(obj) {
            let abs_val = big.abs();
            if let Some(i) = bigint_to_inline(&abs_val) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, abs_val);
        }
        if let Some(f) = to_f64(obj) {
            return MoltObject::from_float(f.abs()).bits();
        }
        if let Some(ptr) = complex_ptr_from_bits(val_bits) {
            let value = unsafe { *complex_ref(ptr) };
            return MoltObject::from_float(value.re.hypot(value.im)).bits();
        }
        if let Some(ptr) = maybe_ptr_from_bits(val_bits)
            && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__abs__")
        {
            unsafe {
                let call_bits = attr_lookup_ptr(_py, ptr, name_bits);
                dec_ref_bits(_py, name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    return res_bits;
                }
            }
        }
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("bad operand type for abs(): '{type_name}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_divmod_builtin(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a_bits);
        let rhs = obj_from_bits(b_bits);
        // If either operand is a float, skip ALL integer paths so that
        // divmod(7, 2.0) returns (3.0, 1.0) instead of (3, 1).
        // Note: to_i64 / to_bigint coerce exact-integer floats (e.g. 2.0 -> 2),
        // so we must guard the bigint path too, not just the i64 fast path.
        let either_float = lhs.is_float() || rhs.is_float();
        if !either_float && let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let li128 = li as i128;
            let ri128 = ri as i128;
            let mut rem = li128 % ri128;
            if rem != 0 && (rem > 0) != (ri128 > 0) {
                rem += ri128;
            }
            let quot = (li128 - rem) / ri128;
            let q_bits = int_bits_from_i128(_py, quot);
            let r_bits = int_bits_from_i128(_py, rem);
            let tuple_ptr = alloc_tuple(_py, &[q_bits, r_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        if !either_float && let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if r_big.is_zero() {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let quot = l_big.div_floor(&r_big);
            let rem = l_big.mod_floor(&r_big);
            let q_bits = if let Some(i) = bigint_to_inline(&quot) {
                MoltObject::from_int(i).bits()
            } else {
                bigint_bits(_py, quot)
            };
            let r_bits = if let Some(i) = bigint_to_inline(&rem) {
                MoltObject::from_int(i).bits()
            } else {
                bigint_bits(_py, rem)
            };
            let tuple_ptr = alloc_tuple(_py, &[q_bits, r_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "float divmod()");
            }
            let quot = (lf / rf).floor();
            let mut rem = lf % rf;
            if rem != 0.0 && (rem > 0.0) != (rf > 0.0) {
                rem += rf;
            }
            let q_bits = MoltObject::from_float(quot).bits();
            let r_bits = MoltObject::from_float(rem).bits();
            let tuple_ptr = alloc_tuple(_py, &[q_bits, r_bits]);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let left = class_name_for_error(type_of_bits(_py, a_bits));
        let right = class_name_for_error(type_of_bits(_py, b_bits));
        let msg = format!("unsupported operand type(s) for divmod(): '{left}' and '{right}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[inline]
fn minmax_compare(_py: &PyToken<'_>, best_key_bits: u64, cand_key_bits: u64) -> CompareOutcome {
    compare_objects(
        _py,
        obj_from_bits(cand_key_bits),
        obj_from_bits(best_key_bits),
    )
}

fn molt_minmax_builtin(
    _py: &PyToken<'_>,
    args_bits: u64,
    key_bits: u64,
    default_bits: u64,
    want_max: bool,
    name: &str,
) -> u64 {
    let missing = missing_bits(_py);
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        let msg = format!("{name} expected at least 1 argument, got 0");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            let msg = format!("{name} expected at least 1 argument, got 0");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let args = seq_vec_ref(args_ptr);
        if args.is_empty() {
            let msg = format!("{name} expected at least 1 argument, got 0");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let has_default = default_bits != missing;
        if args.len() > 1 && has_default {
            let msg =
                format!("Cannot specify a default for {name}() with multiple positional arguments");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let use_key = !obj_from_bits(key_bits).is_none();
        let mut best_bits;
        let mut best_key_bits: u64;
        if args.len() == 1 {
            let iter_bits = molt_iter(args[0]);
            if obj_from_bits(iter_bits).is_none() {
                return raise_not_iterable(_py, args[0]);
            }
            let pair_bits = molt_iter_next(iter_bits);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
            };
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
                if has_default {
                    inc_ref_bits(_py, default_bits);
                    return default_bits;
                }
                let msg = format!("{name}() iterable argument is empty");
                return raise_exception::<_>(_py, "ValueError", &msg);
            }
            best_bits = val_bits;
            if use_key {
                best_key_bits = call_callable1(_py, key_bits, best_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            } else {
                best_key_bits = best_bits;
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                };
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
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                    }
                    inc_ref_bits(_py, best_bits);
                    return best_bits;
                }
                let cand_key_bits = if use_key {
                    let res_bits = call_callable1(_py, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    res_bits
                } else {
                    val_bits
                };
                let replace = match minmax_compare(_py, best_key_bits, cand_key_bits) {
                    CompareOutcome::Ordered(ordering) => {
                        if want_max {
                            ordering == Ordering::Greater
                        } else {
                            ordering == Ordering::Less
                        }
                    }
                    CompareOutcome::Unordered => false,
                    CompareOutcome::NotComparable => {
                        if use_key {
                            dec_ref_bits(_py, best_key_bits);
                            dec_ref_bits(_py, cand_key_bits);
                        }
                        return compare_type_error(
                            _py,
                            obj_from_bits(cand_key_bits),
                            obj_from_bits(best_key_bits),
                            if want_max { ">" } else { "<" },
                        );
                    }
                    CompareOutcome::Error => {
                        if use_key {
                            dec_ref_bits(_py, best_key_bits);
                            dec_ref_bits(_py, cand_key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                if replace {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                    }
                    best_bits = val_bits;
                    best_key_bits = cand_key_bits;
                } else if use_key {
                    dec_ref_bits(_py, cand_key_bits);
                }
            }
        }
        best_bits = args[0];
        if use_key {
            best_key_bits = call_callable1(_py, key_bits, best_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        } else {
            best_key_bits = best_bits;
        }
        for &val_bits in args.iter().skip(1) {
            let cand_key_bits = if use_key {
                let res_bits = call_callable1(_py, key_bits, val_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                res_bits
            } else {
                val_bits
            };
            let replace = match minmax_compare(_py, best_key_bits, cand_key_bits) {
                CompareOutcome::Ordered(ordering) => {
                    if want_max {
                        ordering == Ordering::Greater
                    } else {
                        ordering == Ordering::Less
                    }
                }
                CompareOutcome::Unordered => false,
                CompareOutcome::NotComparable => {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                        dec_ref_bits(_py, cand_key_bits);
                    }
                    return compare_type_error(
                        _py,
                        obj_from_bits(cand_key_bits),
                        obj_from_bits(best_key_bits),
                        if want_max { ">" } else { "<" },
                    );
                }
                CompareOutcome::Error => {
                    if use_key {
                        dec_ref_bits(_py, best_key_bits);
                        dec_ref_bits(_py, cand_key_bits);
                    }
                    return MoltObject::none().bits();
                }
            };
            if replace {
                if use_key {
                    dec_ref_bits(_py, best_key_bits);
                }
                best_bits = val_bits;
                best_key_bits = cand_key_bits;
            } else if use_key {
                dec_ref_bits(_py, cand_key_bits);
            }
        }
        if use_key {
            dec_ref_bits(_py, best_key_bits);
        }
        inc_ref_bits(_py, best_bits);
        best_bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_min_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, false, "min")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_max_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, true, "max")
    })
}


struct SortItem {
    key_bits: u64,
    value_bits: u64,
}

enum SortError {
    NotComparable(u64, u64),
    Exception,
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sorted_builtin(iter_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        let use_key = !obj_from_bits(key_bits).is_none();
        let reverse = is_truthy(_py, obj_from_bits(reverse_bits));
        let mut items: Vec<SortItem> = Vec::new();
        loop {
            let pair_bits = molt_iter_next(iter_obj);
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                if use_key {
                    for item in items.drain(..) {
                        dec_ref_bits(_py, item.key_bits);
                    }
                }
                // If an exception is pending, propagate it; otherwise the
                // iterator returned a non-pointer sentinel — treat as done
                // and fall through to build the (possibly empty) sorted list.
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                break;
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    if use_key {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    if use_key {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                    }
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let val_bits = elems[0];
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let key_val_bits = if use_key {
                    let res_bits = call_callable1(_py, key_bits, val_bits);
                    if exception_pending(_py) {
                        for item in items.drain(..) {
                            dec_ref_bits(_py, item.key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    res_bits
                } else {
                    val_bits
                };
                items.push(SortItem {
                    key_bits: key_val_bits,
                    value_bits: val_bits,
                });
            }
        }
        let mut error: Option<SortError> = None;
        items.sort_by(|left, right| {
            if error.is_some() {
                return Ordering::Equal;
            }
            let outcome = compare_objects(
                _py,
                obj_from_bits(left.key_bits),
                obj_from_bits(right.key_bits),
            );
            match outcome {
                CompareOutcome::Ordered(ordering) => {
                    if reverse {
                        ordering.reverse()
                    } else {
                        ordering
                    }
                }
                CompareOutcome::Unordered => Ordering::Equal,
                CompareOutcome::NotComparable => {
                    error = Some(SortError::NotComparable(left.key_bits, right.key_bits));
                    Ordering::Equal
                }
                CompareOutcome::Error => {
                    error = Some(SortError::Exception);
                    Ordering::Equal
                }
            }
        });
        if let Some(error) = error {
            if use_key {
                for item in items.drain(..) {
                    dec_ref_bits(_py, item.key_bits);
                }
            }
            match error {
                SortError::NotComparable(left_bits, right_bits) => {
                    let msg = format!(
                        "'<' not supported between instances of '{}' and '{}'",
                        type_name(_py, obj_from_bits(left_bits)),
                        type_name(_py, obj_from_bits(right_bits)),
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                SortError::Exception => {
                    return MoltObject::none().bits();
                }
            }
        }
        let mut out: Vec<u64> = Vec::with_capacity(items.len());
        for item in items.iter() {
            out.push(item.value_bits);
        }
        if use_key {
            for item in items.drain(..) {
                dec_ref_bits(_py, item.key_bits);
            }
        }
        let list_ptr = alloc_list(_py, &out);
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sum_builtin(iter_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let start_obj = obj_from_bits(start_bits);
        if let Some(ptr) = start_obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum strings [use ''.join(seq) instead]",
                    );
                }
                if type_id == TYPE_ID_BYTES {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum bytes [use b''.join(seq) instead]",
                    );
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "sum() can't sum bytearray [use b''.join(seq) instead]",
                    );
                }
            }
        }
        // Fast path: if the iterable is a list or tuple of integers, sum
        // directly without going through the iterator protocol.  This avoids
        // allocating a (value, done) tuple per element.
        {
            let iter_obj_check = obj_from_bits(iter_bits);
            if let Some(ptr) = iter_obj_check.as_ptr() {
                let type_id = unsafe { object_type_id(ptr) };
                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    let elems = unsafe { seq_vec_ref(ptr) };
                    let start_int = to_i64(start_obj);
                    if let Some(_) = start_int {
                        let mut acc128 = start_int.unwrap() as i128;
                        let mut all_int = true;
                        for &bits in elems.iter() {
                            let elem = obj_from_bits(bits);
                            if let Some(i) = to_i64(elem) {
                                acc128 += i as i128;
                            } else {
                                all_int = false;
                                break;
                            }
                        }
                        if all_int {
                            use crate::builtins::numbers::int_bits_from_i128;
                            return int_bits_from_i128(_py, acc128);
                        }
                    }
                }
            }
        }
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        // CPython >= 3.12 uses Neumaier compensated summation for float sums.
        // Detect float accumulation and switch to compensated mode.
        let mut total_bits = start_bits;
        let mut total_owned = false;
        let start_f = to_f64(start_obj);
        // If start is a number, try Neumaier compensated path.
        if let Some(start_val) = start_f {
            let mut fsum = start_val;
            let mut comp = 0.0_f64; // Neumaier compensation term
            let mut all_numeric = true;
            let mut has_float = start_obj.as_float().is_some();
            loop {
                let pair_bits = molt_iter_next(iter_obj);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                };
                unsafe {
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
                        if all_numeric {
                            let result = fsum + comp;
                            if has_float {
                                return MoltObject::from_float(result).bits();
                            } else {
                                return MoltObject::from_int(result as i64).bits();
                            }
                        }
                        if !total_owned {
                            inc_ref_bits(_py, total_bits);
                        }
                        return total_bits;
                    }
                    let val_obj = obj_from_bits(val_bits);
                    if all_numeric {
                        // Check if value is float-coercible and stay in compensated mode
                        let item_f = if let Some(f) = val_obj.as_float() {
                            has_float = true;
                            Some(f)
                        } else if let Some(i) = to_i64(val_obj) {
                            Some(i as f64)
                        } else {
                            None
                        };
                        if let Some(x) = item_f {
                            // Neumaier compensated summation step
                            let t = fsum + x;
                            if fsum.abs() >= x.abs() {
                                comp += (fsum - t) + x;
                            } else {
                                comp += (x - t) + fsum;
                            }
                            fsum = t;
                            total_bits = MoltObject::from_float(fsum).bits();
                            total_owned = true;
                            continue;
                        }
                        // Non-numeric value: fall back to generic sum.
                        // total_owned must be set here because the done-check
                        // at the top of the next iteration reads it.
                        all_numeric = false;
                        total_bits = MoltObject::from_float(fsum + comp).bits();
                        #[allow(unused_assignments)]
                        { total_owned = true; }
                    }
                    let next_bits = molt_add(total_bits, val_bits);
                    if obj_from_bits(next_bits).is_none() {
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        return binary_type_error(
                            _py,
                            obj_from_bits(total_bits),
                            obj_from_bits(val_bits),
                            "+",
                        );
                    }
                    total_bits = next_bits;
                    total_owned = true;
                }
            }
        }
        // Non-numeric start: generic path
        loop {
            let pair_bits = molt_iter_next(iter_obj);
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
                let next_bits = molt_add(total_bits, val_bits);
                if obj_from_bits(next_bits).is_none() {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return binary_type_error(
                        _py,
                        obj_from_bits(total_bits),
                        obj_from_bits(val_bits),
                        "+",
                    );
                }
                total_bits = next_bits;
                total_owned = true;
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if default_bits == missing {
            return molt_get_attr_name(obj_bits, name_bits);
        }
        molt_get_attr_name_default(obj_bits, name_bits, default_bits)
    })
}

/// Python `setattr(obj, name, value)` builtin.
#[unsafe(no_mangle)]
pub extern "C" fn molt_setattr_builtin(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_object_setattr(obj_bits, name_bits, val_bits);
        MoltObject::none().bits()
    })
}

/// Python `delattr(obj, name)` builtin.
#[unsafe(no_mangle)]
pub extern "C" fn molt_delattr_builtin(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_object_delattr(obj_bits, name_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        res
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_vars_builtin(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if obj_bits == missing {
            // CPython parity: vars() == locals() when called with no arguments.
            // Note: `molt_locals_builtin` is safe to call here; `with_gil_entry` is
            // re-entrant and uses the existing token.
            return crate::molt_locals_builtin();
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let dict_bits = molt_get_attr_name_default(obj_bits, dict_name_bits, missing);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if dict_bits == missing {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "vars() argument must have __dict__ attribute",
            );
        }
        dict_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_getstate(_self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(ptr) = obj_from_bits(_self_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let type_id = unsafe { object_type_id(ptr) };
        if type_id != crate::TYPE_ID_OBJECT && type_id != crate::TYPE_ID_DATACLASS {
            return MoltObject::none().bits();
        }

        // 1. Collect __dict__ entries.
        let mut dict_state_bits: Option<u64> = None;
        let dict_bits = if type_id == crate::TYPE_ID_DATACLASS {
            unsafe { crate::dataclass_dict_bits(ptr) }
        } else {
            unsafe { crate::instance_dict_bits(ptr) }
        };
        if dict_bits != 0
            && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
            && unsafe { object_type_id(dict_ptr) } == crate::TYPE_ID_DICT
            && !unsafe { crate::dict_order(dict_ptr).is_empty() }
        {
            inc_ref_bits(_py, dict_bits);
            dict_state_bits = Some(dict_bits);
        }

        // 2. Collect typed/slot field values.
        let slot_state_bits = if type_id == crate::TYPE_ID_DATACLASS {
            dataclass_getstate_slot_state(_py, ptr)
        } else {
            object_getstate_slot_state(_py, ptr)
        };

        // 3. Combine following CPython's (dict, slots) tuple convention.
        match (dict_state_bits, slot_state_bits) {
            (Some(d), Some(s)) => {
                let tuple_ptr = crate::alloc_tuple(_py, &[d, s]);
                dec_ref_bits(_py, d);
                dec_ref_bits(_py, s);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
            (None, Some(s)) => {
                let none_bits = MoltObject::none().bits();
                let tuple_ptr = crate::alloc_tuple(_py, &[none_bits, s]);
                dec_ref_bits(_py, s);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(tuple_ptr).bits()
            }
            (Some(d), None) => d,
            (None, None) => {
                // CPython returns self.__dict__ which may be empty {}.
                let dict_ptr = crate::alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(dict_ptr).bits()
            }
        }
    })
}

/// Extract typed field values from `__molt_field_offsets__` into a new dict.
fn object_getstate_slot_state(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    let class_bits = unsafe { object_class_bits(ptr) };
    let class_ptr = obj_from_bits(class_bits).as_ptr()?;
    if unsafe { object_type_id(class_ptr) } != crate::TYPE_ID_TYPE {
        return None;
    }
    let class_dict_bits = unsafe { crate::class_dict_bits(class_ptr) };
    let class_dict_ptr = obj_from_bits(class_dict_bits).as_ptr()?;
    if unsafe { object_type_id(class_dict_ptr) } != crate::TYPE_ID_DICT {
        return None;
    }
    let offsets_name_bits =
        crate::builtins::attr::attr_name_bits_from_bytes(_py, b"__molt_field_offsets__")?;
    let offsets_bits = unsafe { crate::dict_get_in_place(_py, class_dict_ptr, offsets_name_bits) };
    dec_ref_bits(_py, offsets_name_bits);
    if exception_pending(_py) {
        return None;
    }
    let offsets_bits = offsets_bits?;
    let offsets_ptr = obj_from_bits(offsets_bits).as_ptr()?;
    if unsafe { object_type_id(offsets_ptr) } != crate::TYPE_ID_DICT {
        return None;
    }

    let state_ptr = crate::alloc_dict_with_pairs(_py, &[]);
    if state_ptr.is_null() {
        return None;
    }
    let state_bits = MoltObject::from_ptr(state_ptr).bits();
    let mut wrote_any = false;
    let pairs = unsafe { crate::dict_order(offsets_ptr).to_vec() };
    let mut idx = 0usize;
    while idx + 1 < pairs.len() {
        let name_bits = pairs[idx];
        let offset_bits = pairs[idx + 1];
        idx += 2;
        let offset = obj_from_bits(offset_bits).as_int().filter(|&v| v >= 0)?;
        let value_bits = unsafe { crate::object_field_get_ptr_raw(_py, ptr, offset as usize) };
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return None;
        }
        if crate::builtins::methods::is_missing_bits(_py, value_bits) {
            dec_ref_bits(_py, value_bits);
            continue;
        }
        unsafe {
            crate::dict_set_in_place(_py, state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, value_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return None;
        }
        wrote_any = true;
    }
    if !wrote_any {
        dec_ref_bits(_py, state_bits);
        return None;
    }
    Some(state_bits)
}

/// Extract dataclass field values from the descriptor layout into a new dict.
fn dataclass_getstate_slot_state(_py: &crate::PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    let desc_ptr = unsafe { crate::dataclass_desc_ptr(ptr) };
    if desc_ptr.is_null() {
        return None;
    }
    let field_values = unsafe { crate::dataclass_fields_ref(ptr) };
    let field_names = unsafe { &(*desc_ptr).field_names };
    if field_names.is_empty() {
        return None;
    }

    let state_ptr = crate::alloc_dict_with_pairs(_py, &[]);
    if state_ptr.is_null() {
        return None;
    }
    let state_bits = MoltObject::from_ptr(state_ptr).bits();
    let mut wrote_any = false;
    for (name, &value_bits) in field_names.iter().zip(field_values.iter()) {
        if crate::builtins::methods::is_missing_bits(_py, value_bits) {
            continue;
        }
        let Some(name_bits) =
            crate::builtins::attr::attr_name_bits_from_bytes(_py, name.as_bytes())
        else {
            dec_ref_bits(_py, state_bits);
            return None;
        };
        unsafe {
            crate::dict_set_in_place(_py, state_ptr, name_bits, value_bits);
        }
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, state_bits);
            return None;
        }
        wrote_any = true;
    }
    if !wrote_any {
        dec_ref_bits(_py, state_bits);
        return None;
    }
    Some(state_bits)
}

fn dir_runtime_python_at_least(_py: &PyToken<'_>, major: i64, minor: i64) -> bool {
    let state = runtime_state(_py);
    let guard = state.sys_version_info.lock().unwrap();
    let (runtime_major, runtime_minor) = guard
        .as_ref()
        .map(|info| (info.major, info.minor))
        .unwrap_or((3, 12));
    runtime_major > major || (runtime_major == major && runtime_minor >= minor)
}

fn dir_add_builtin_method_surface(
    _py: &PyToken<'_>,
    target_class_bits: u64,
    add_name: &mut dyn FnMut(&[u8]) -> bool,
) -> bool {
    let builtins = builtin_classes(_py);
    if target_class_bits == builtins.str {
        for name in [
            &b"capitalize"[..],
            &b"casefold"[..],
            &b"center"[..],
            &b"count"[..],
            &b"encode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"find"[..],
            &b"format"[..],
            &b"format_map"[..],
            &b"index"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdecimal"[..],
            &b"isdigit"[..],
            &b"isidentifier"[..],
            &b"islower"[..],
            &b"isnumeric"[..],
            &b"isprintable"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.bytes {
        for name in [
            &b"capitalize"[..],
            &b"center"[..],
            &b"count"[..],
            &b"decode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"find"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"index"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdigit"[..],
            &b"islower"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.bytearray {
        for name in [
            &b"append"[..],
            &b"capitalize"[..],
            &b"center"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"count"[..],
            &b"decode"[..],
            &b"endswith"[..],
            &b"expandtabs"[..],
            &b"extend"[..],
            &b"find"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"index"[..],
            &b"insert"[..],
            &b"isalnum"[..],
            &b"isalpha"[..],
            &b"isascii"[..],
            &b"isdigit"[..],
            &b"islower"[..],
            &b"isspace"[..],
            &b"istitle"[..],
            &b"isupper"[..],
            &b"join"[..],
            &b"ljust"[..],
            &b"lower"[..],
            &b"lstrip"[..],
            &b"maketrans"[..],
            &b"partition"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"removeprefix"[..],
            &b"removesuffix"[..],
            &b"replace"[..],
            &b"reverse"[..],
            &b"rfind"[..],
            &b"rindex"[..],
            &b"rjust"[..],
            &b"rpartition"[..],
            &b"rsplit"[..],
            &b"rstrip"[..],
            &b"split"[..],
            &b"splitlines"[..],
            &b"startswith"[..],
            &b"strip"[..],
            &b"swapcase"[..],
            &b"title"[..],
            &b"translate"[..],
            &b"upper"[..],
            &b"zfill"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"resize"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.int || target_class_bits == builtins.bool {
        for name in [
            &b"as_integer_ratio"[..],
            &b"bit_count"[..],
            &b"bit_length"[..],
            &b"conjugate"[..],
            &b"from_bytes"[..],
            &b"is_integer"[..],
            &b"to_bytes"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.float {
        for name in [
            &b"as_integer_ratio"[..],
            &b"conjugate"[..],
            &b"fromhex"[..],
            &b"hex"[..],
            &b"is_integer"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"from_number"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.complex {
        if !add_name(&b"conjugate"[..]) {
            return false;
        }
        if dir_runtime_python_at_least(_py, 3, 14) && !add_name(&b"from_number"[..]) {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.list {
        for name in [
            &b"append"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"count"[..],
            &b"extend"[..],
            &b"index"[..],
            &b"insert"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"reverse"[..],
            &b"sort"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.tuple {
        return add_name(&b"count"[..]) && add_name(&b"index"[..]);
    }
    if target_class_bits == builtins.range {
        return add_name(&b"count"[..]) && add_name(&b"index"[..]);
    }
    if target_class_bits == builtins.dict {
        for name in [
            &b"clear"[..],
            &b"copy"[..],
            &b"fromkeys"[..],
            &b"get"[..],
            &b"items"[..],
            &b"keys"[..],
            &b"pop"[..],
            &b"popitem"[..],
            &b"setdefault"[..],
            &b"update"[..],
            &b"values"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.set {
        for name in [
            &b"add"[..],
            &b"clear"[..],
            &b"copy"[..],
            &b"difference"[..],
            &b"difference_update"[..],
            &b"discard"[..],
            &b"intersection"[..],
            &b"intersection_update"[..],
            &b"isdisjoint"[..],
            &b"issubset"[..],
            &b"issuperset"[..],
            &b"pop"[..],
            &b"remove"[..],
            &b"symmetric_difference"[..],
            &b"symmetric_difference_update"[..],
            &b"union"[..],
            &b"update"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.frozenset {
        for name in [
            &b"copy"[..],
            &b"difference"[..],
            &b"intersection"[..],
            &b"isdisjoint"[..],
            &b"issubset"[..],
            &b"issuperset"[..],
            &b"symmetric_difference"[..],
            &b"union"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        return true;
    }
    if target_class_bits == builtins.memoryview {
        for name in [
            &b"_from_flags"[..],
            &b"cast"[..],
            &b"hex"[..],
            &b"release"[..],
            &b"tobytes"[..],
            &b"tolist"[..],
            &b"toreadonly"[..],
        ] {
            if !add_name(name) {
                return false;
            }
        }
        if dir_runtime_python_at_least(_py, 3, 14)
            && (!add_name(&b"count"[..]) || !add_name(&b"index"[..]))
        {
            return false;
        }
        return true;
    }
    if target_class_bits == builtins.property {
        return add_name(&b"getter"[..]) && add_name(&b"setter"[..]) && add_name(&b"deleter"[..]);
    }
    if target_class_bits == builtins.base_exception_group
        || issubclass_bits(target_class_bits, builtins.base_exception_group)
    {
        return add_name(&b"add_note"[..])
            && add_name(&b"with_traceback"[..])
            && add_name(&b"derive"[..])
            && add_name(&b"split"[..])
            && add_name(&b"subgroup"[..]);
    }
    if target_class_bits == builtins.base_exception
        || issubclass_bits(target_class_bits, builtins.base_exception)
    {
        return add_name(&b"add_note"[..]) && add_name(&b"with_traceback"[..]);
    }
    if target_class_bits == builtins.slice {
        return add_name(&b"indices"[..]);
    }
    if target_class_bits == builtins.type_obj {
        return add_name(&b"mro"[..]);
    }
    true
}

unsafe fn dir_default_collect(_py: &PyToken<'_>, obj_bits: u64) -> u64 {
    unsafe {
        crate::gil_assert();

        let mut names: Vec<u64> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut extra_owned: Vec<u64> = Vec::new();

        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            let type_id = object_type_id(obj_ptr);
            if type_id == TYPE_ID_TYPE {
                dir_collect_from_class_bits(obj_bits, &mut seen, &mut names);
            } else {
                dir_collect_from_instance(_py, obj_ptr, &mut seen, &mut names);
                dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
            }
        } else {
            dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
        }

        // Our runtime keeps many builtin methods in fast method caches rather than in
        // `type.__dict__`. CPython's dir() includes those names, so ensure they're visible.
        let mut add_name = |name: &[u8]| -> bool {
            let Ok(name_str) = std::str::from_utf8(name) else {
                return true;
            };
            if !seen.insert(name_str.to_string()) {
                return true;
            }
            let Some(bits) = attr_name_bits_from_bytes(_py, name) else {
                return false;
            };
            extra_owned.push(bits);
            names.push(bits);
            true
        };

        // Object surface (ordering-critical names appear early in CPython's sorted dir()).
        for name in [
            &b"__class__"[..],
            &b"__delattr__"[..],
            &b"__dir__"[..],
            &b"__doc__"[..],
            &b"__eq__"[..],
            &b"__format__"[..],
            &b"__ge__"[..],
            &b"__getattribute__"[..],
            &b"__getstate__"[..],
            &b"__gt__"[..],
            &b"__hash__"[..],
            &b"__init__"[..],
            &b"__init_subclass__"[..],
            &b"__le__"[..],
            &b"__lt__"[..],
            &b"__ne__"[..],
            &b"__new__"[..],
            &b"__repr__"[..],
            &b"__setattr__"[..],
            &b"__str__"[..],
        ] {
            if !add_name(name) {
                for owned in extra_owned {
                    dec_ref_bits(_py, owned);
                }
                return MoltObject::none().bits();
            }
        }

        let builtins = builtin_classes(_py);
        let target_class_bits = if maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_TYPE)
        {
            obj_bits
        } else {
            type_of_bits(_py, obj_bits)
        };

        if target_class_bits == builtins.int || target_class_bits == builtins.bool {
            for name in [
                &b"__abs__"[..],
                &b"__add__"[..],
                &b"__and__"[..],
                &b"__bool__"[..],
                &b"__ceil__"[..],
                &b"__divmod__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.str {
            for name in [&b"__add__"[..], &b"__contains__"[..], &b"__getitem__"[..]] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.list {
            for name in [
                &b"__add__"[..],
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.dict {
            for name in [
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
                &b"__getitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.none_type && !add_name(&b"__bool__"[..]) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }
        if !dir_add_builtin_method_surface(_py, target_class_bits, &mut add_name) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }

        // Hide names that CPython deliberately excludes from dir() output (even though the
        // attributes exist).
        let hide_module = is_builtin_class_bits(_py, target_class_bits);
        names.retain(|&bits| {
            let Some(name) = string_obj_to_owned(obj_from_bits(bits)) else {
                return true;
            };
            if name == "__mro__" || name == "__bases__" || name == "__text_signature__" {
                return false;
            }
            if name.starts_with("__molt_") {
                return false;
            }
            if hide_module && name == "__module__" {
                return false;
            }
            true
        });

        let list_ptr = alloc_list(_py, &names);
        for owned in extra_owned {
            dec_ref_bits(_py, owned);
        }
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let none_bits = MoltObject::none().bits();
        let reverse_bits = MoltObject::from_int(0).bits();
        let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        list_bits
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_dir_method(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { unsafe { dir_default_collect(_py, self_bits) } })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_format_method(self_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let spec_obj = obj_from_bits(spec_bits);
        let Some(spec) = string_obj_to_owned(spec_obj) else {
            return raise_exception::<_>(_py, "TypeError", "format_spec must be str");
        };
        if spec.is_empty() {
            return molt_str_from_obj(self_bits);
        }
        let type_label = type_name(_py, obj_from_bits(self_bits));
        let msg = format!("unsupported format string passed to {type_label}.__format__");
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_lt_method(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_le_method(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_gt_method(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_ge_method(_self_bits: u64, _other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_bool_method(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(is_truthy(_py, obj_from_bits(self_bits))).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_ceil_method(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, self_bits);
        self_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_abs_method(self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_abs_builtin(self_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_add_method(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let other_ty = type_of_bits(_py, other_bits);
        if other_ty != builtins.int && other_ty != builtins.bool {
            return not_implemented_bits(_py);
        }
        molt_add(self_bits, other_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_and_method(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let other_ty = type_of_bits(_py, other_bits);
        if other_ty != builtins.int && other_ty != builtins.bool {
            return not_implemented_bits(_py);
        }
        molt_bit_and(self_bits, other_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_divmod_method(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let other_ty = type_of_bits(_py, other_bits);
        if other_ty != builtins.int && other_ty != builtins.bool {
            return not_implemented_bits(_py);
        }
        molt_divmod_builtin(self_bits, other_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_str_add_method(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let builtins = builtin_classes(_py);
        let other_ty = type_of_bits(_py, other_bits);
        if other_ty != builtins.str {
            return not_implemented_bits(_py);
        }
        molt_add(self_bits, other_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dir_builtin(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if obj_bits == missing {
            // CPython: dir() (no args) lists the caller's local scope.
            unsafe {
                // Note: `molt_locals_builtin` is safe to call here; `with_gil_entry` is
                // re-entrant and many runtime helpers rely on nested calls.
                let locals_bits = crate::molt_locals_builtin();
                if exception_pending(_py) {
                    if !obj_from_bits(locals_bits).is_none() {
                        dec_ref_bits(_py, locals_bits);
                    }
                    return MoltObject::none().bits();
                }
                let list_bits = list_from_iter_bits(_py, locals_bits)
                    .unwrap_or_else(|| MoltObject::none().bits());
                if !obj_from_bits(locals_bits).is_none() {
                    dec_ref_bits(_py, locals_bits);
                }
                if obj_from_bits(list_bits).is_none() || exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let none_bits = MoltObject::none().bits();
                let reverse_bits = MoltObject::from_int(0).bits();
                let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return list_bits;
            }
        }

        let mut names: Vec<u64> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut extra_owned: Vec<u64> = Vec::new();
        let _obj = obj_from_bits(obj_bits);
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            unsafe {
                // CPython's dir() respects user-defined `__dir__`, but it must *not* dispatch
                // to our internal fast-path method-cache implementation (which would recurse
                // back into this builtin).
                //
                // So: only consult instance `__dict__` and the class `__dict__` MRO chain,
                // skipping method caches entirely.
                static DIR_NAME: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                let dir_name_bits = intern_static_name(_py, &DIR_NAME, b"__dir__");
                let mut override_bits: u64 = 0;

                let dict_bits = instance_dict_bits(obj_ptr);
                if dict_bits != 0
                    && !obj_from_bits(dict_bits).is_none()
                    && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                    && let Some(val_bits) = dict_get_in_place(_py, dict_ptr, dir_name_bits)
                {
                    inc_ref_bits(_py, val_bits);
                    override_bits = val_bits;
                }

                if override_bits == 0 {
                    let class_bits = type_of_bits(_py, obj_bits);
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                        && let Some(attr_bits) =
                            class_attr_lookup_raw_mro(_py, class_ptr, dir_name_bits)
                    {
                        let bound_opt = descriptor_bind(_py, attr_bits, class_ptr, Some(obj_ptr));
                        dec_ref_bits(_py, attr_bits);

                        if exception_pending(_py) {
                            // `descriptor_bind` can create a temporary bound object; avoid leaks.
                            if let Some(bound_bits) = bound_opt
                                && !obj_from_bits(bound_bits).is_none()
                            {
                                dec_ref_bits(_py, bound_bits);
                            }
                            return MoltObject::none().bits();
                        }

                        if let Some(bound_bits) = bound_opt {
                            override_bits = bound_bits;
                        }
                    }
                }

                if override_bits != 0 && !obj_from_bits(override_bits).is_none() {
                    let res_bits = call_callable0(_py, override_bits);
                    dec_ref_bits(_py, override_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return res_bits;
                }
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_TYPE {
                    dir_collect_from_class_bits(obj_bits, &mut seen, &mut names);
                } else {
                    dir_collect_from_instance(_py, obj_ptr, &mut seen, &mut names);
                    dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
                }
            }
        } else {
            unsafe {
                dir_collect_from_class_bits(type_of_bits(_py, obj_bits), &mut seen, &mut names);
            }
        }

        // Our runtime keeps many builtin methods in fast method caches rather than in
        // `type.__dict__`. CPython's dir() includes those names, so ensure they're visible.
        let mut add_name = |name: &[u8]| -> bool {
            let Ok(name_str) = std::str::from_utf8(name) else {
                return true;
            };
            if !seen.insert(name_str.to_string()) {
                return true;
            }
            let Some(bits) = attr_name_bits_from_bytes(_py, name) else {
                return false;
            };
            extra_owned.push(bits);
            names.push(bits);
            true
        };

        // Object surface (ordering-critical names appear early in CPython's sorted dir()).
        for name in [
            &b"__class__"[..],
            &b"__delattr__"[..],
            &b"__dir__"[..],
            &b"__doc__"[..],
            &b"__eq__"[..],
            &b"__format__"[..],
            &b"__ge__"[..],
            &b"__getattribute__"[..],
            &b"__getstate__"[..],
            &b"__gt__"[..],
            &b"__hash__"[..],
            &b"__init__"[..],
            &b"__init_subclass__"[..],
            &b"__le__"[..],
            &b"__lt__"[..],
            &b"__ne__"[..],
            &b"__new__"[..],
            &b"__repr__"[..],
            &b"__setattr__"[..],
            &b"__str__"[..],
        ] {
            if !add_name(name) {
                for owned in extra_owned {
                    dec_ref_bits(_py, owned);
                }
                return MoltObject::none().bits();
            }
        }

        let builtins = builtin_classes(_py);
        let target_class_bits = if maybe_ptr_from_bits(obj_bits)
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_TYPE })
        {
            obj_bits
        } else {
            type_of_bits(_py, obj_bits)
        };

        if target_class_bits == builtins.int || target_class_bits == builtins.bool {
            for name in [
                &b"__abs__"[..],
                &b"__add__"[..],
                &b"__and__"[..],
                &b"__bool__"[..],
                &b"__ceil__"[..],
                &b"__divmod__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.str {
            for name in [&b"__add__"[..], &b"__contains__"[..], &b"__getitem__"[..]] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.list {
            for name in [
                &b"__add__"[..],
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.dict {
            for name in [
                &b"__class_getitem__"[..],
                &b"__contains__"[..],
                &b"__delitem__"[..],
                &b"__getitem__"[..],
            ] {
                if !add_name(name) {
                    for owned in extra_owned {
                        dec_ref_bits(_py, owned);
                    }
                    return MoltObject::none().bits();
                }
            }
        } else if target_class_bits == builtins.none_type && !add_name(&b"__bool__"[..]) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }
        if !dir_add_builtin_method_surface(_py, target_class_bits, &mut add_name) {
            for owned in extra_owned {
                dec_ref_bits(_py, owned);
            }
            return MoltObject::none().bits();
        }

        // Hide names that CPython deliberately excludes from dir() output (even though the
        // attributes exist).
        let hide_module = is_builtin_class_bits(_py, target_class_bits);
        names.retain(|&bits| {
            let Some(name) = string_obj_to_owned(obj_from_bits(bits)) else {
                return true;
            };
            if name == "__mro__" || name == "__bases__" || name == "__text_signature__" {
                return false;
            }
            if name.starts_with("__molt_") {
                return false;
            }
            if hide_module && name == "__module__" {
                return false;
            }
            true
        });

        let list_ptr = alloc_list(_py, &names);
        for owned in extra_owned {
            dec_ref_bits(_py, owned);
        }
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let list_bits = MoltObject::from_ptr(list_ptr).bits();
        let none_bits = MoltObject::none().bits();
        let reverse_bits = MoltObject::from_int(0).bits();
        let _ = molt_list_sort(list_bits, none_bits, reverse_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        list_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_init(_self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_init_subclass(_cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_getattribute(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let type_id = object_type_id(obj_ptr);
                let found = match type_id {
                    TYPE_ID_OBJECT => object_attr_lookup_raw(_py, obj_ptr, name_bits),
                    TYPE_ID_DATACLASS => dataclass_attr_lookup_raw(_py, obj_ptr, name_bits),
                    _ => attr_lookup_ptr(_py, obj_ptr, name_bits),
                };
                if let Some(val) = found {
                    return val;
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits();
                }
                if type_id == TYPE_ID_DATACLASS {
                    let desc_ptr = dataclass_desc_ptr(obj_ptr);
                    if !desc_ptr.is_null() && (*desc_ptr).slots {
                        let name = &(*desc_ptr).name;
                        let type_label = if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        };
                        return attr_error_with_obj(
                            _py,
                            type_label,
                            &attr_name,
                            MoltObject::from_ptr(obj_ptr).bits(),
                        ) as u64;
                    }
                    let type_label = if !desc_ptr.is_null() {
                        let name = &(*desc_ptr).name;
                        if name.is_empty() {
                            "dataclass"
                        } else {
                            name.as_str()
                        }
                    } else {
                        "dataclass"
                    };
                    return attr_error_with_obj(
                        _py,
                        type_label,
                        &attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    ) as u64;
                }
                if type_id == TYPE_ID_TYPE {
                    let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                    return attr_error_with_obj_message(
                        _py,
                        &msg,
                        &attr_name,
                        MoltObject::from_ptr(obj_ptr).bits(),
                    ) as u64;
                }
                return attr_error_with_obj(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    &attr_name,
                    MoltObject::from_ptr(obj_ptr).bits(),
                ) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            if (obj.is_int() || obj.is_bool())
                && let Some(func_bits) = crate::builtins::methods::int_method_bits(_py, &attr_name)
            {
                return crate::molt_bound_method_new(func_bits, obj_bits);
            }
            if obj.is_float()
                && let Some(func_bits) = crate::builtins::methods::float_method_bits(_py, &attr_name)
            {
                return crate::molt_bound_method_new(func_bits, obj_bits);
            }
            // Inline int/float/bool: fall back to class-based resolution
            // so that inherited methods (e.g. object.__init__) are found.
            {
                let builtins = builtin_classes(_py);
                let class_bits = if obj.is_float() {
                    builtins.float
                } else if obj.is_bool() {
                    builtins.bool
                } else if obj.is_int() {
                    builtins.int
                } else {
                    0
                };
                if class_bits != 0 {
                    if let Some(func_bits) = crate::builtins::methods::builtin_class_method_bits(_py, class_bits, &attr_name) {
                        return crate::molt_bound_method_new(func_bits, obj_bits);
                    }
                    if let Some(func_bits) = crate::builtins::methods::builtin_class_method_bits(_py, builtins.object, &attr_name) {
                        return crate::molt_bound_method_new(func_bits, obj_bits);
                    }
                }
            }
            attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits) as u64
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_getattribute(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let type_id = object_type_id(obj_ptr);
                if type_id != TYPE_ID_TYPE {
                    return molt_object_getattribute(obj_bits, name_bits);
                }
                let found = crate::builtins::attributes::type_attr_lookup_ptr_default(
                    _py, obj_ptr, name_bits,
                );
                if let Some(val) = found {
                    return val;
                }
                if exception_pending(_py) {
                    let exc_bits = molt_exception_last();
                    molt_exception_clear();
                    let _ = molt_raise(exc_bits);
                    dec_ref_bits(_py, exc_bits);
                    return MoltObject::none().bits();
                }
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                    .unwrap_or_default();
                let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                return attr_error_with_message(_py, &msg) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits) as u64
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_type_call(cls_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let cls_obj = obj_from_bits(cls_bits);
        let Some(cls_ptr) = cls_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
        };
        unsafe {
            if object_type_id(cls_ptr) != TYPE_ID_TYPE {
                return raise_exception::<_>(_py, "TypeError", "type.__call__ expects type");
            }
            if matches!(
                std::env::var("MOLT_TRACE_TYPE_CALL").ok().as_deref(),
                Some("1")
            ) {
                let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(cls_ptr)))
                    .unwrap_or_default();
                let builtins = builtin_classes(_py);
                let kind = if cls_bits == builtins.type_obj {
                    "builtins.type"
                } else {
                    "type"
                };
                eprintln!(
                    "molt direct: type.__call__ invoked kind={} name={} cls_bits={} (no builder args forwarded)",
                    kind, class_name, cls_bits
                );
            }
            call_class_init_with_args(_py, cls_ptr, &[])
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_setattr(obj_bits: u64, name_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, attr_name.as_bytes()) else {
                return MoltObject::none().bits();
            };
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_TYPE {
                    dec_ref_bits(_py, attr_bits);
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "can't apply this __setattr__ to type object",
                    );
                }
                let class_bits = object_class_bits(obj_ptr);
                let builtins = builtin_classes(_py);
                let is_dict_subclass =
                    type_id == TYPE_ID_DICT && class_bits != 0 && class_bits != builtins.dict;
                let res = if type_id == TYPE_ID_OBJECT || is_dict_subclass {
                    object_setattr_raw(_py, obj_ptr, attr_bits, &attr_name, val_bits)
                } else if type_id == TYPE_ID_DATACLASS {
                    dataclass_setattr_raw_unchecked(_py, obj_ptr, attr_bits, &attr_name, val_bits)
                } else {
                    let bytes = string_bytes(name_ptr);
                    let len = string_len(name_ptr);
                    molt_set_attr_generic(obj_ptr, bytes, len as u64, val_bits)
                };
                dec_ref_bits(_py, attr_bits);
                return res as u64;
            }
            let obj = obj_from_bits(obj_bits);
            let res = attr_error_with_obj(_py, type_name(_py, obj), &attr_name, obj_bits) as u64;
            dec_ref_bits(_py, attr_bits);
            res
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_delattr(obj_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name_obj = obj_from_bits(name_bits);
        let Some(name_ptr) = name_obj.as_ptr() else {
            return raise_attr_name_type_error(_py, name_bits);
        };
        unsafe {
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_attr_name_type_error(_py, name_bits);
            }
            let attr_name = string_obj_to_owned(obj_from_bits(name_bits))
                .unwrap_or_else(|| "<attr>".to_string());
            let Some(attr_bits) = attr_name_bits_from_bytes(_py, attr_name.as_bytes()) else {
                return MoltObject::none().bits();
            };
            if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
                let type_id = object_type_id(obj_ptr);
                if type_id == TYPE_ID_TYPE {
                    dec_ref_bits(_py, attr_bits);
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "can't apply this __delattr__ to type object",
                    );
                }
                let class_bits = object_class_bits(obj_ptr);
                let builtins = builtin_classes(_py);
                let is_dict_subclass =
                    type_id == TYPE_ID_DICT && class_bits != 0 && class_bits != builtins.dict;
                let res = if type_id == TYPE_ID_OBJECT || is_dict_subclass {
                    object_delattr_raw(_py, obj_ptr, attr_bits, &attr_name)
                } else if type_id == TYPE_ID_DATACLASS {
                    dataclass_delattr_raw_unchecked(_py, obj_ptr, attr_bits, &attr_name)
                } else {
                    del_attr_ptr(_py, obj_ptr, attr_bits, &attr_name)
                };
                dec_ref_bits(_py, attr_bits);
                return res as u64;
            }
            let obj = obj_from_bits(obj_bits);
            let res = attr_error(_py, type_name(_py, obj), &attr_name) as u64;
            dec_ref_bits(_py, attr_bits);
            res
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_bits == other_bits {
            return MoltObject::from_bool(true).bits();
        }
        not_implemented_bits(_py)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_object_ne(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_bits == other_bits {
            return MoltObject::from_bool(false).bits();
        }
        not_implemented_bits(_py)
    })
}


#[unsafe(no_mangle)]
pub extern "C" fn molt_print_builtin(
    args_bits: u64,
    sep_bits: u64,
    end_bits: u64,
    file_bits: u64,
    flush_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        fn print_string_arg_bits(
            _py: &PyToken<'_>,
            bits: u64,
            default: &[u8],
            label: &str,
        ) -> Option<u64> {
            let obj = obj_from_bits(bits);
            if obj.is_none() {
                let ptr = alloc_string(_py, default);
                if ptr.is_null() {
                    return None;
                }
                return Some(MoltObject::from_ptr(ptr).bits());
            }
            let Some(ptr) = obj.as_ptr() else {
                let msg = format!(
                    "{} must be None or a string, not {}",
                    label,
                    type_name(_py, obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            unsafe {
                if object_type_id(ptr) != TYPE_ID_STRING {
                    let msg = format!(
                        "{} must be None or a string, not {}",
                        label,
                        type_name(_py, obj)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
            inc_ref_bits(_py, bits);
            Some(bits)
        }

        fn string_bits_is_empty(bits: u64) -> bool {
            let obj = obj_from_bits(bits);
            let Some(ptr) = obj.as_ptr() else {
                return false;
            };
            unsafe { string_len(ptr) == 0 }
        }

        fn string_bits_contains_newline(bits: u64) -> bool {
            let obj = obj_from_bits(bits);
            let Some(ptr) = obj.as_ptr() else {
                return false;
            };
            unsafe {
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                bytes.contains(&b'\n')
            }
        }

        fn encode_print_bytes(
            _py: &PyToken<'_>,
            bits: u64,
            encoding: &str,
            errors: &str,
        ) -> Result<Vec<u8>, u64> {
            let obj = obj_from_bits(bits);
            let Some(ptr) = obj.as_ptr() else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "print expects a string",
                ));
            };
            unsafe {
                if object_type_id(ptr) != TYPE_ID_STRING {
                    return Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        "print expects a string",
                    ));
                }
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                match encode_string_with_errors(bytes, encoding, Some(errors)) {
                    Ok(out) => Ok(out),
                    Err(EncodeError::UnknownEncoding(name)) => {
                        let msg = format!("unknown encoding: {name}");
                        Err(raise_exception::<_>(_py, "LookupError", &msg))
                    }
                    Err(EncodeError::UnknownErrorHandler(name)) => {
                        let msg = format!("unknown error handler name '{name}'");
                        Err(raise_exception::<_>(_py, "LookupError", &msg))
                    }
                    Err(EncodeError::InvalidChar {
                        encoding,
                        code,
                        pos,
                        limit,
                    }) => {
                        let reason = encode_error_reason(encoding, code, limit);
                        Err(raise_unicode_encode_error::<_>(
                            _py,
                            encoding,
                            bits,
                            pos,
                            pos + 1,
                            &reason,
                        ))
                    }
                }
            }
        }

        let args_obj = obj_from_bits(args_bits);
        let Some(args_ptr) = args_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "print expects a tuple");
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "print expects a tuple");
            }
            let mut sep_bits_opt = match print_string_arg_bits(_py, sep_bits, b" ", "sep") {
                Some(bits) => Some(bits),
                None => return MoltObject::none().bits(),
            };
            let mut end_bits_opt = match print_string_arg_bits(_py, end_bits, b"\n", "end") {
                Some(bits) => Some(bits),
                None => {
                    if let Some(bits) = sep_bits_opt {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
            };
            if let Some(bits) = sep_bits_opt
                && string_bits_is_empty(bits)
            {
                dec_ref_bits(_py, bits);
                sep_bits_opt = None;
            }
            if let Some(bits) = end_bits_opt
                && string_bits_is_empty(bits)
            {
                dec_ref_bits(_py, bits);
                end_bits_opt = None;
            }

            let mut resolved_file_bits = file_bits;
            let mut sys_found = false;
            let mut file_from_sys = false;
            if obj_from_bits(resolved_file_bits).is_none() {
                let sys_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.sys_name, b"sys");
                if !obj_from_bits(sys_name_bits).is_none() {
                    let sys_bits = molt_module_cache_get(sys_name_bits);
                    if !obj_from_bits(sys_bits).is_none() {
                        sys_found = true;
                        let stdout_name_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.stdout_name,
                            b"stdout",
                        );
                        resolved_file_bits = molt_module_get_attr(sys_bits, stdout_name_bits);
                        dec_ref_bits(_py, sys_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        file_from_sys = true;
                    }
                }
            }

            let elems = seq_vec_ref(args_ptr);
            let do_flush = is_truthy(_py, obj_from_bits(flush_bits));

            if obj_from_bits(resolved_file_bits).is_none() && !sys_found {
                let encoding = "utf-8";
                let errors = "surrogateescape";
                let mut stdout = std::io::stdout();
                let mut wrote_newline = false;
                let sep_bytes = if let Some(bits) = sep_bits_opt {
                    match encode_print_bytes(_py, bits, encoding, errors) {
                        Ok(bytes) => Some(bytes),
                        Err(bits) => {
                            if let Some(end_bits) = end_bits_opt {
                                dec_ref_bits(_py, end_bits);
                            }
                            dec_ref_bits(_py, bits);
                            return bits;
                        }
                    }
                } else {
                    None
                };
                let end_bytes = if let Some(bits) = end_bits_opt {
                    match encode_print_bytes(_py, bits, encoding, errors) {
                        Ok(bytes) => Some(bytes),
                        Err(bits) => {
                            if let Some(sep_bits) = sep_bits_opt {
                                dec_ref_bits(_py, sep_bits);
                            }
                            dec_ref_bits(_py, bits);
                            return bits;
                        }
                    }
                } else {
                    None
                };
                for (idx, &val_bits) in elems.iter().enumerate() {
                    if idx > 0
                        && let Some(bytes) = sep_bytes.as_deref()
                    {
                        if bytes.contains(&b'\n') {
                            wrote_newline = true;
                        }
                        let _ = stdout.write_all(bytes);
                    }
                    let str_bits = molt_str_from_obj(val_bits);
                    if exception_pending(_py) {
                        if let Some(sep_bits) = sep_bits_opt {
                            dec_ref_bits(_py, sep_bits);
                        }
                        if let Some(end_bits) = end_bits_opt {
                            dec_ref_bits(_py, end_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let bytes = match encode_print_bytes(_py, str_bits, encoding, errors) {
                        Ok(bytes) => bytes,
                        Err(bits) => {
                            dec_ref_bits(_py, str_bits);
                            if let Some(sep_bits) = sep_bits_opt {
                                dec_ref_bits(_py, sep_bits);
                            }
                            if let Some(end_bits) = end_bits_opt {
                                dec_ref_bits(_py, end_bits);
                            }
                            return bits;
                        }
                    };
                    if bytes.contains(&b'\n') {
                        wrote_newline = true;
                    }
                    let _ = stdout.write_all(&bytes);
                    dec_ref_bits(_py, str_bits);
                }
                if let Some(bytes) = end_bytes.as_deref() {
                    if bytes.contains(&b'\n') {
                        wrote_newline = true;
                    }
                    let _ = stdout.write_all(bytes);
                }
                if do_flush || wrote_newline {
                    let _ = stdout.flush();
                }
                if let Some(bits) = sep_bits_opt {
                    dec_ref_bits(_py, bits);
                }
                if let Some(bits) = end_bits_opt {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }

            let sep_bits = sep_bits_opt;
            let end_bits = end_bits_opt;
            let end_has_newline = end_bits.map(string_bits_contains_newline).unwrap_or(false);

            let mut write_bits = MoltObject::none().bits();
            let mut use_file_handle = false;
            if let Some(ptr) = obj_from_bits(resolved_file_bits).as_ptr() {
                use_file_handle = object_type_id(ptr) == TYPE_ID_FILE_HANDLE;
            }
            if !use_file_handle {
                let write_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.write_name, b"write");
                write_bits = molt_get_attr_name(resolved_file_bits, write_name_bits);
                if exception_pending(_py) {
                    if let Some(bits) = sep_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = end_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
            }

            for (idx, &val_bits) in elems.iter().enumerate() {
                if idx > 0
                    && let Some(bits) = sep_bits
                {
                    if use_file_handle {
                        let _ = molt_file_write(resolved_file_bits, bits);
                    } else {
                        let res_bits = call_callable1(_py, write_bits, bits);
                        dec_ref_bits(_py, res_bits);
                    }
                    if exception_pending(_py) {
                        if !use_file_handle {
                            dec_ref_bits(_py, write_bits);
                        }
                        if let Some(bits) = sep_bits {
                            dec_ref_bits(_py, bits);
                        }
                        if let Some(bits) = end_bits {
                            dec_ref_bits(_py, bits);
                        }
                        if file_from_sys {
                            dec_ref_bits(_py, resolved_file_bits);
                        }
                        return MoltObject::none().bits();
                    }
                }
                let str_bits = molt_str_from_obj(val_bits);
                if exception_pending(_py) {
                    if !use_file_handle {
                        dec_ref_bits(_py, write_bits);
                    }
                    if let Some(bits) = sep_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = end_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
                if use_file_handle {
                    let _ = molt_file_write(resolved_file_bits, str_bits);
                } else {
                    let res_bits = call_callable1(_py, write_bits, str_bits);
                    dec_ref_bits(_py, res_bits);
                }
                dec_ref_bits(_py, str_bits);
                if exception_pending(_py) {
                    if !use_file_handle {
                        dec_ref_bits(_py, write_bits);
                    }
                    if let Some(bits) = sep_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if let Some(bits) = end_bits {
                        dec_ref_bits(_py, bits);
                    }
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
            }
            if let Some(bits) = end_bits {
                if use_file_handle {
                    let _ = molt_file_write(resolved_file_bits, bits);
                } else {
                    let res_bits = call_callable1(_py, write_bits, bits);
                    dec_ref_bits(_py, res_bits);
                }
                if exception_pending(_py) {
                    if !use_file_handle {
                        dec_ref_bits(_py, write_bits);
                    }
                    if let Some(bits) = sep_bits {
                        dec_ref_bits(_py, bits);
                    }
                    dec_ref_bits(_py, bits);
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
            }
            if !use_file_handle {
                dec_ref_bits(_py, write_bits);
            }
            if let Some(bits) = sep_bits {
                dec_ref_bits(_py, bits);
            }
            if let Some(bits) = end_bits {
                dec_ref_bits(_py, bits);
            }

            if do_flush || (file_from_sys && use_file_handle && end_has_newline) {
                if use_file_handle {
                    let _ = molt_file_flush(resolved_file_bits);
                } else {
                    let flush_name_bits =
                        intern_static_name(_py, &runtime_state(_py).interned.flush_name, b"flush");
                    let flush_method_bits = molt_get_attr_name(resolved_file_bits, flush_name_bits);
                    if exception_pending(_py) {
                        if file_from_sys {
                            dec_ref_bits(_py, resolved_file_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let flush_res_bits = call_callable0(_py, flush_method_bits);
                    dec_ref_bits(_py, flush_method_bits);
                    dec_ref_bits(_py, flush_res_bits);
                    if exception_pending(_py) {
                        if file_from_sys {
                            dec_ref_bits(_py, resolved_file_bits);
                        }
                        return MoltObject::none().bits();
                    }
                }
            }
            if file_from_sys {
                dec_ref_bits(_py, resolved_file_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_input_builtin(prompt_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sys_name_bits = intern_static_name(_py, &runtime_state(_py).interned.sys_name, b"sys");
        if obj_from_bits(sys_name_bits).is_none() {
            return raise_exception::<_>(_py, "RuntimeError", "sys module name missing");
        }
        let sys_bits = molt_module_cache_get(sys_name_bits);
        if obj_from_bits(sys_bits).is_none() {
            return raise_exception::<_>(_py, "RuntimeError", "sys module unavailable");
        }

        let stdout_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.stdout_name, b"stdout");
        let stdout_bits = molt_module_get_attr(sys_bits, stdout_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, sys_bits);
            return MoltObject::none().bits();
        }
        if obj_from_bits(stdout_bits).is_none() {
            dec_ref_bits(_py, sys_bits);
            return raise_exception::<_>(_py, "RuntimeError", "sys.stdout unavailable");
        }

        let prompt_str_bits = molt_str_from_obj(prompt_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, stdout_bits);
            dec_ref_bits(_py, sys_bits);
            return MoltObject::none().bits();
        }

        let mut stdout_is_handle = false;
        if let Some(ptr) = obj_from_bits(stdout_bits).as_ptr() {
            unsafe {
                stdout_is_handle = object_type_id(ptr) == TYPE_ID_FILE_HANDLE;
            }
        }

        if stdout_is_handle {
            let _ = molt_file_write(stdout_bits, prompt_str_bits);
        } else {
            let write_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.write_name, b"write");
            let write_method_bits = molt_get_attr_name(stdout_bits, write_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, prompt_str_bits);
                dec_ref_bits(_py, stdout_bits);
                dec_ref_bits(_py, sys_bits);
                return MoltObject::none().bits();
            }
            let write_res_bits = unsafe { call_callable1(_py, write_method_bits, prompt_str_bits) };
            dec_ref_bits(_py, write_method_bits);
            dec_ref_bits(_py, write_res_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, prompt_str_bits);
                dec_ref_bits(_py, stdout_bits);
                dec_ref_bits(_py, sys_bits);
                return MoltObject::none().bits();
            }
        }

        // Match CPython: flush stdout after writing the prompt.
        if stdout_is_handle {
            let _ = molt_file_flush(stdout_bits);
        } else {
            let missing = missing_bits(_py);
            let flush_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.flush_name, b"flush");
            let flush_bits = molt_getattr_builtin(stdout_bits, flush_name_bits, missing);
            if exception_pending(_py) {
                dec_ref_bits(_py, prompt_str_bits);
                dec_ref_bits(_py, stdout_bits);
                dec_ref_bits(_py, sys_bits);
                return MoltObject::none().bits();
            }
            if flush_bits != missing {
                let callable_bits = molt_is_callable(flush_bits);
                let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
                dec_ref_bits(_py, callable_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, flush_bits);
                    dec_ref_bits(_py, prompt_str_bits);
                    dec_ref_bits(_py, stdout_bits);
                    dec_ref_bits(_py, sys_bits);
                    return MoltObject::none().bits();
                }
                if is_callable {
                    let flush_res_bits = unsafe { call_callable0(_py, flush_bits) };
                    dec_ref_bits(_py, flush_res_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, flush_bits);
                        dec_ref_bits(_py, prompt_str_bits);
                        dec_ref_bits(_py, stdout_bits);
                        dec_ref_bits(_py, sys_bits);
                        return MoltObject::none().bits();
                    }
                }
                dec_ref_bits(_py, flush_bits);
            }
        }

        dec_ref_bits(_py, prompt_str_bits);
        dec_ref_bits(_py, stdout_bits);

        let stdin_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.stdin_name, b"stdin");
        let stdin_bits = molt_module_get_attr(sys_bits, stdin_name_bits);
        dec_ref_bits(_py, sys_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if obj_from_bits(stdin_bits).is_none() {
            return raise_exception::<_>(_py, "RuntimeError", "sys.stdin unavailable");
        }

        let mut stdin_is_handle = false;
        if let Some(ptr) = obj_from_bits(stdin_bits).as_ptr() {
            unsafe {
                stdin_is_handle = object_type_id(ptr) == TYPE_ID_FILE_HANDLE;
            }
        }
        let line_bits = if stdin_is_handle {
            molt_file_readline(stdin_bits, MoltObject::from_int(-1).bits())
        } else {
            let readline_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.readline_name, b"readline");
            let method_bits = molt_get_attr_name(stdin_bits, readline_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, stdin_bits);
                return MoltObject::none().bits();
            }
            let out_bits = unsafe { call_callable0(_py, method_bits) };
            dec_ref_bits(_py, method_bits);
            out_bits
        };
        dec_ref_bits(_py, stdin_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let Some(line_ptr) = obj_from_bits(line_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "input() returned non-string");
        };
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "input() returned non-string");
            }
            let bytes = std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            if bytes.is_empty() {
                dec_ref_bits(_py, line_bits);
                return raise_exception::<_>(_py, "EOFError", "");
            }
            let mut end = bytes.len();
            if bytes[end - 1] == b'\n' {
                end -= 1;
                if end > 0 && bytes[end - 1] == b'\r' {
                    end -= 1;
                }
            } else if bytes[end - 1] == b'\r' {
                end -= 1;
            }
            if end == bytes.len() {
                return line_bits;
            }
            let out_ptr = alloc_string(_py, &bytes[..end]);
            dec_ref_bits(_py, line_bits);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_super_builtin(type_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_super_new(type_bits, obj_bits) })
}

// ---------------------------------------------------------------------------
// Type-constructor builtins: thin `extern "C"` wrappers so the compiler can
// emit direct calls to `molt_<type>_builtin` for Python's builtin types.
// ---------------------------------------------------------------------------

/// `int(x=0, base=10)` — wraps `molt_int_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_int_builtin(val_bits: u64, base_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            // int() with no args => 0
            return MoltObject::from_int(0).bits();
        }
        let has_base = base_bits != missing;
        let has_base_bits = if has_base { 1u64 } else { 0u64 };
        let actual_base = if has_base {
            base_bits
        } else {
            MoltObject::from_int(10).bits()
        };
        molt_int_from_obj(val_bits, actual_base, has_base_bits)
    })
}

/// `float(x=0.0)` — wraps `molt_float_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_float_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return MoltObject::from_float(0.0).bits();
        }
        molt_float_from_obj(val_bits)
    })
}

/// `bool(x=False)` — wraps `is_truthy`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bool_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return MoltObject::from_bool(false).bits();
        }
        MoltObject::from_bool(is_truthy(_py, obj_from_bits(val_bits))).bits()
    })
}

/// `str(object='')` — wraps `molt_str_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_str_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_string(_py, b"");
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_str_from_obj(val_bits)
    })
}

/// `bytes(source=b'')` — wraps `molt_bytes_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_bytes(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_bytes_from_obj(val_bits)
    })
}

/// `bytearray(source=bytearray())` — wraps `molt_bytearray_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_bytearray(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        molt_bytearray_from_obj(val_bits)
    })
}

/// `list(iterable=())` — constructs a list from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_list(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        unsafe {
            let Some(bits) = list_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `tuple(iterable=())` — constructs a tuple from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            let ptr = alloc_tuple(_py, &[]);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            return MoltObject::from_ptr(ptr).bits();
        }
        unsafe {
            let Some(bits) = tuple_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `dict(mapping_or_iterable=None)` — wraps `molt_dict_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_dict_new(0);
        }
        molt_dict_from_obj(val_bits)
    })
}

/// `set(iterable=())` — constructs a set from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_set_new(0);
        }
        let set_bits = molt_set_new(0);
        if obj_from_bits(set_bits).is_none() {
            return MoltObject::none().bits();
        }
        let _ = molt_set_update(set_bits, val_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, set_bits);
            return MoltObject::none().bits();
        }
        set_bits
    })
}

/// `frozenset(iterable=())` — constructs a frozenset from an iterable.
#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if val_bits == missing {
            return molt_frozenset_new(0);
        }
        unsafe {
            let Some(bits) = frozenset_from_iter_bits(_py, val_bits) else {
                return MoltObject::none().bits();
            };
            bits
        }
    })
}

/// `range(stop)` / `range(start, stop[, step])` — wraps `molt_range_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_range_builtin(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if start_bits == missing {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "range expected at least 1 argument, got 0",
            );
        }
        if stop_bits == missing {
            // range(stop) — single-arg form
            let zero = MoltObject::from_int(0).bits();
            let one = MoltObject::from_int(1).bits();
            return molt_range_new(zero, start_bits, one);
        }
        let actual_step = if step_bits == missing {
            MoltObject::from_int(1).bits()
        } else {
            step_bits
        };
        molt_range_new(start_bits, stop_bits, actual_step)
    })
}

/// `slice(stop)` / `slice(start, stop[, step])` — wraps `molt_slice_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_slice_builtin(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let none = MoltObject::none().bits();
        if start_bits == missing {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "slice expected at least 1 argument, got 0",
            );
        }
        if stop_bits == missing {
            // slice(stop) — single-arg form
            return molt_slice_new(none, start_bits, none);
        }
        let actual_step = if step_bits == missing { none } else { step_bits };
        molt_slice_new(start_bits, stop_bits, actual_step)
    })
}

/// `object()` — wraps `molt_object_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_object_builtin() -> u64 {
    molt_object_new()
}

/// `type(object)` — wraps `molt_builtin_type`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_type_builtin(val_bits: u64) -> u64 {
    molt_builtin_type(val_bits)
}

/// `complex(real=0, imag=0)` — wraps `molt_complex_from_obj`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_builtin(real_bits: u64, imag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let actual_real = if real_bits == missing {
            MoltObject::from_int(0).bits()
        } else {
            real_bits
        };
        let has_imag = imag_bits != missing;
        let has_imag_bits = if has_imag { 1u64 } else { 0u64 };
        let actual_imag = if has_imag {
            imag_bits
        } else {
            MoltObject::from_int(0).bits()
        };
        molt_complex_from_obj(actual_real, actual_imag, has_imag_bits)
    })
}

/// `memoryview(obj)` — wraps `molt_memoryview_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_builtin(val_bits: u64) -> u64 {
    molt_memoryview_new(val_bits)
}

/// `classmethod(func)` — wraps `molt_classmethod_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_classmethod_builtin(func_bits: u64) -> u64 {
    molt_classmethod_new(func_bits)
}

/// `staticmethod(func)` — wraps `molt_staticmethod_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_staticmethod_builtin(func_bits: u64) -> u64 {
    molt_staticmethod_new(func_bits)
}

/// `property(fget=None, fset=None, fdel=None)` — wraps `molt_property_new`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_property_builtin(
    get_bits: u64,
    set_bits: u64,
    del_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let none = MoltObject::none().bits();
        let g = if get_bits == missing { none } else { get_bits };
        let s = if set_bits == missing { none } else { set_bits };
        let d = if del_bits == missing { none } else { del_bits };
        molt_property_new(g, s, d)
    })
}

/// `isinstance(obj, classinfo)` — wraps `molt_isinstance`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_isinstance_builtin(val_bits: u64, class_bits: u64) -> u64 {
    molt_isinstance(val_bits, class_bits)
}

/// `issubclass(sub, classinfo)` — wraps `molt_issubclass`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_issubclass_builtin(sub_bits: u64, class_bits: u64) -> u64 {
    molt_issubclass(sub_bits, class_bits)
}

/// `hasattr(obj, name)` — wraps `molt_has_attr_name`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_hasattr_builtin(obj_bits: u64, name_bits: u64) -> u64 {
    molt_has_attr_name(obj_bits, name_bits)
}

/// `aiter(async_iterable)` — wraps `molt_aiter`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_aiter_builtin(obj_bits: u64) -> u64 {
    molt_aiter(obj_bits)
}

/// `iter(object)` — wraps `molt_iter_checked`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_iter_builtin(obj_bits: u64) -> u64 {
    molt_iter_checked(obj_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_slice(obj_bits: u64, start_bits: u64, end_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let start_obj = obj_from_bits(start_bits);
        let end_obj = obj_from_bits(end_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let total_chars =
                        utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize)) as isize;
                    let start = match decode_slice_bound(_py, start_obj, total_chars, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, total_chars, total_chars) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_string(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let start_byte = utf8_char_to_byte_index_cached(
                        _py,
                        bytes,
                        start as i64,
                        Some(ptr as usize),
                    );
                    let end_byte =
                        utf8_char_to_byte_index_cached(_py, bytes, end as i64, Some(ptr as usize));
                    let slice = &bytes[start_byte..end_byte];
                    let out = alloc_string(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                if type_id == TYPE_ID_BYTES {
                    let len = bytes_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_bytes(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len as usize);
                    let slice = &bytes[start as usize..end as usize];
                    let out = alloc_bytes(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_bytearray(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len as usize);
                    let slice = &bytes[start as usize..end as usize];
                    let out = alloc_bytearray(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    let len = memoryview_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let base_offset = memoryview_offset(ptr);
                        let stride = memoryview_stride(ptr);
                        let out_ptr = alloc_memoryview(
                            _py,
                            memoryview_owner_bits(ptr),
                            base_offset + start * stride,
                            0,
                            memoryview_itemsize(ptr),
                            stride,
                            memoryview_readonly(ptr),
                            memoryview_format_bits(ptr),
                        );
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let base_offset = memoryview_offset(ptr);
                    let new_offset = base_offset + start * memoryview_stride(ptr);
                    let new_len = (end - start) as usize;
                    let out_ptr = alloc_memoryview(
                        _py,
                        memoryview_owner_bits(ptr),
                        new_offset,
                        new_len,
                        memoryview_itemsize(ptr),
                        memoryview_stride(ptr),
                        memoryview_readonly(ptr),
                        memoryview_format_bits(ptr),
                    );
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                if type_id == TYPE_ID_LIST {
                    let len = list_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_list(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let elems = seq_vec_ref(ptr);
                    let slice = &elems[start as usize..end as usize];
                    let out = alloc_list(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
                if type_id == TYPE_ID_TUPLE {
                    let len = tuple_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    if end < start {
                        let out = alloc_tuple(_py, &[]);
                        if out.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out).bits();
                    }
                    let elems = seq_vec_ref(ptr);
                    let slice = &elems[start as usize..end as usize];
                    let out = alloc_tuple(_py, slice);
                    if out.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out).bits();
                }
            }
        }
        let slice_bits = molt_slice_new(start_bits, end_bits, MoltObject::none().bits());
        if obj_from_bits(slice_bits).is_none() {
            return MoltObject::none().bits();
        }
        let res_bits = molt_index(obj_bits, slice_bits);
        dec_ref_bits(_py, slice_bits);
        res_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_intarray_from_seq(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                let elems = if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    seq_vec_ref(ptr)
                } else {
                    return MoltObject::none().bits();
                };
                let mut out = Vec::with_capacity(elems.len());
                for &elem in elems {
                    let val = MoltObject::from_bits(elem);
                    if let Some(i) = val.as_int() {
                        out.push(i);
                    } else {
                        return MoltObject::none().bits();
                    }
                }
                let out_ptr = alloc_intarray(_py, &out);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_from_list(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_TUPLE {
                    inc_ref_bits(_py, bits);
                    return bits;
                }
                if type_id == TYPE_ID_LIST {
                    let elems = seq_vec_ref(ptr);
                    let out_ptr = alloc_tuple(_py, elems);
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}


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
pub extern "C" fn molt_memoryview_new(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview expects a bytes-like object",
                );
            }
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_MEMORYVIEW {
                let owner_bits = memoryview_owner_bits(ptr);
                let offset = memoryview_offset(ptr);
                let len = memoryview_len(ptr);
                let itemsize = memoryview_itemsize(ptr);
                let stride = memoryview_stride(ptr);
                let readonly = memoryview_readonly(ptr);
                let format_bits = memoryview_format_bits(ptr);
                let shape = memoryview_shape(ptr).unwrap_or(&[]).to_vec();
                let strides = memoryview_strides(ptr).unwrap_or(&[]).to_vec();
                let out_ptr = if shape.len() > 1 || memoryview_ndim(ptr) == 0 {
                    alloc_memoryview_shaped(
                        _py,
                        owner_bits,
                        offset,
                        itemsize,
                        readonly,
                        format_bits,
                        shape,
                        strides,
                    )
                } else {
                    alloc_memoryview(
                        _py,
                        owner_bits,
                        offset,
                        len,
                        itemsize,
                        stride,
                        readonly,
                        format_bits,
                    )
                };
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let readonly = type_id == TYPE_ID_BYTES;
                let format_ptr = alloc_string(_py, b"B");
                if format_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let format_bits = MoltObject::from_ptr(format_ptr).bits();
                let out_ptr = alloc_memoryview(_py, bits, 0, len, 1, 1, readonly, format_bits);
                dec_ref_bits(_py, format_bits);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
        }
        raise_exception::<_>(_py, "TypeError", "memoryview expects a bytes-like object")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_from_flags(obj_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let flag_type = class_name_for_error(type_of_bits(_py, flags_bits));
        let err = format!("'{flag_type}' object cannot be interpreted as an integer");
        let Some(flags) = index_bigint_from_obj(_py, flags_bits, &err) else {
            return MoltObject::none().bits();
        };
        if flags.is_odd()
            && let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits)
        {
            unsafe {
                let type_id = object_type_id(obj_ptr);
                // CPython ignores writable-flag checks when the input is already a memoryview.
                if type_id == TYPE_ID_BYTES {
                    return raise_exception::<_>(_py, "BufferError", "Object is not writable.");
                }
            }
        }
        molt_memoryview_new(obj_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_cast(
    view_bits: u64,
    format_bits: u64,
    shape_bits: u64,
    has_shape_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let view = obj_from_bits(view_bits);
        let view_ptr = match view.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cast() argument 'view' must be a memoryview",
                );
            }
        };
        unsafe {
            if object_type_id(view_ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cast() argument 'view' must be a memoryview",
                );
            }
            let format_obj = obj_from_bits(format_bits);
            let format_str = match string_obj_to_owned(format_obj) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!(
                            "cast() argument 'format' must be str, not {}",
                            type_name(_py, format_obj)
                        ),
                    );
                }
            };
            let fmt = match memoryview_format_from_str(&format_str) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "memoryview: destination format must be a native single character format prefixed with an optional '@'",
                    );
                }
            };
            if !memoryview_is_c_contiguous_view(view_ptr) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview: casts are restricted to C-contiguous views",
                );
            }
            let shape_view = memoryview_shape(view_ptr).unwrap_or(&[]);
            let nbytes = match memoryview_nbytes_big(shape_view, memoryview_itemsize(view_ptr)) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let has_shape = is_truthy(_py, obj_from_bits(has_shape_bits));
            let shape = if has_shape {
                let shape_obj = obj_from_bits(shape_bits);
                let shape_ptr = match shape_obj.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "shape must be a list or a tuple",
                        );
                    }
                };
                let type_id = object_type_id(shape_ptr);
                if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "shape must be a list or a tuple",
                    );
                }
                let elems = seq_vec_ref(shape_ptr);
                let mut shape = Vec::with_capacity(elems.len());
                for &elem_bits in elems.iter() {
                    let elem_obj = obj_from_bits(elem_bits);
                    let Some(val) = to_i64(elem_obj).or_else(|| {
                        bigint_ptr_from_bits(elem_bits).and_then(|ptr| bigint_ref(ptr).to_i64())
                    }) else {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "memoryview.cast(): elements of shape must be integers",
                        );
                    };
                    if val <= 0 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "memoryview.cast(): elements of shape must be integers > 0",
                        );
                    }
                    shape.push(val as isize);
                }
                shape
            } else {
                let itemsize = fmt.itemsize as i128;
                if itemsize == 0 || nbytes % itemsize != 0 {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: length is not a multiple of itemsize",
                    );
                }
                let len = (nbytes / itemsize) as isize;
                vec![len]
            };
            let product = match memoryview_shape_product(&shape) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            if product.saturating_mul(fmt.itemsize as i128) != nbytes {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview: product(shape) * itemsize != buffer size",
                );
            }
            let mut strides = vec![0isize; shape.len()];
            let mut stride = fmt.itemsize as isize;
            for idx in (0..shape.len()).rev() {
                strides[idx] = stride;
                stride = stride.saturating_mul(shape[idx].max(1));
            }
            let out_ptr = alloc_memoryview_shaped(
                _py,
                memoryview_owner_bits(view_ptr),
                memoryview_offset(view_ptr),
                fmt.itemsize,
                memoryview_readonly(view_ptr),
                format_bits,
                shape,
                strides,
            );
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_tobytes(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "tobytes expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "tobytes expects a memoryview");
            }
            let out = match memoryview_collect_bytes(ptr) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let out_ptr = alloc_bytes(_py, &out);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

unsafe fn memoryview_tolist_recursive(
    _py: &PyToken<'_>,
    data: &[u8],
    fmt: MemoryViewFormat,
    shape: &[isize],
    strides: &[isize],
    dim: usize,
    base_offset: isize,
) -> Option<u64> {
    if dim >= shape.len() || shape.len() != strides.len() {
        return None;
    }
    let dim_len = shape[dim].max(0) as usize;
    let mut items: Vec<u64> = Vec::with_capacity(dim_len);
    if dim + 1 == shape.len() {
        for i in 0..dim_len {
            let item_offset = base_offset.checked_add((i as isize).saturating_mul(strides[dim]))?;
            let scalar = unsafe { memoryview_read_scalar(_py, data, item_offset, fmt) }?;
            items.push(scalar);
        }
    } else {
        for i in 0..dim_len {
            let child_offset =
                base_offset.checked_add((i as isize).saturating_mul(strides[dim]))?;
            let child = unsafe {
                memoryview_tolist_recursive(_py, data, fmt, shape, strides, dim + 1, child_offset)
            }?;
            items.push(child);
        }
    }
    let out_ptr = alloc_list(_py, items.as_slice());
    if out_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(out_ptr).bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_tolist(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "tolist expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "tolist expects a memoryview");
            }
            let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                Some(fmt) => fmt,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: unsupported format for tolist()",
                    );
                }
            };
            let owner_bits = memoryview_owner_bits(ptr);
            let owner = obj_from_bits(owner_bits);
            let owner_ptr = match owner.as_ptr() {
                Some(ptr) => ptr,
                None => return MoltObject::none().bits(),
            };
            let data = match bytes_like_slice_raw(owner_ptr) {
                Some(slice) => slice,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: tolist() requires a bytes-like exporter",
                    );
                }
            };
            let shape = memoryview_shape(ptr).unwrap_or(&[]);
            let strides = memoryview_strides(ptr).unwrap_or(&[]);
            if shape.is_empty() || memoryview_ndim(ptr) == 0 {
                let scalar = match memoryview_read_scalar(_py, data, memoryview_offset(ptr), fmt) {
                    Some(bits) => bits,
                    None => return MoltObject::none().bits(),
                };
                return scalar;
            }
            match memoryview_tolist_recursive(
                _py,
                data,
                fmt,
                shape,
                strides,
                0,
                memoryview_offset(ptr),
            ) {
                Some(bits) => bits,
                None => MoltObject::none().bits(),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_count(bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "count expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "count expects a memoryview");
            }
            let ndim = memoryview_ndim(ptr);
            if ndim == 0 {
                return raise_exception::<_>(_py, "TypeError", "invalid indexing of 0-dim memory");
            }
            if ndim > 1 {
                return raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    "multi-dimensional sub-views are not implemented",
                );
            }
            let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                Some(fmt) => fmt,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: unsupported format for count()",
                    );
                }
            };
            let owner_bits = memoryview_owner_bits(ptr);
            let owner = obj_from_bits(owner_bits);
            let Some(owner_ptr) = owner.as_ptr() else {
                return MoltObject::none().bits();
            };
            let Some(base) = bytes_like_slice_raw(owner_ptr) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview: count() requires a bytes-like exporter",
                );
            };
            let len = memoryview_len(ptr);
            let offset = memoryview_offset(ptr);
            let stride = memoryview_stride(ptr);
            let mut count = 0i64;
            for idx in 0..len {
                let item_offset = offset.saturating_add((idx as isize).saturating_mul(stride));
                let Some(item_bits) = memoryview_read_scalar(_py, base, item_offset, fmt) else {
                    return MoltObject::none().bits();
                };
                let eq = match eq_bool_from_bits(_py, item_bits, val_bits) {
                    Some(val) => val,
                    None => {
                        if obj_from_bits(item_bits).as_ptr().is_some() {
                            dec_ref_bits(_py, item_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                if obj_from_bits(item_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, item_bits);
                }
                if eq {
                    count += 1;
                }
            }
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_index(bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "index expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "index expects a memoryview");
            }
            let ndim = memoryview_ndim(ptr);
            if ndim == 0 {
                return raise_exception::<_>(_py, "TypeError", "invalid lookup on 0-dim memory");
            }
            if ndim > 1 {
                return raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    "multi-dimensional lookup is not implemented",
                );
            }
            let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                Some(fmt) => fmt,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: unsupported format for index()",
                    );
                }
            };
            let owner_bits = memoryview_owner_bits(ptr);
            let owner = obj_from_bits(owner_bits);
            let Some(owner_ptr) = owner.as_ptr() else {
                return MoltObject::none().bits();
            };
            let Some(base) = bytes_like_slice_raw(owner_ptr) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview: index() requires a bytes-like exporter",
                );
            };
            let len = memoryview_len(ptr);
            let offset = memoryview_offset(ptr);
            let stride = memoryview_stride(ptr);
            for idx in 0..len {
                let item_offset = offset.saturating_add((idx as isize).saturating_mul(stride));
                let Some(item_bits) = memoryview_read_scalar(_py, base, item_offset, fmt) else {
                    return MoltObject::none().bits();
                };
                let eq = match eq_bool_from_bits(_py, item_bits, val_bits) {
                    Some(val) => val,
                    None => {
                        if obj_from_bits(item_bits).as_ptr().is_some() {
                            dec_ref_bits(_py, item_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                if obj_from_bits(item_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, item_bits);
                }
                if eq {
                    return MoltObject::from_int(idx as i64).bits();
                }
            }
            raise_exception::<_>(_py, "ValueError", "memoryview.index(x): x not found")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_hex(bits: u64, sep_bits: u64, bytes_per_sep_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "hex expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "hex expects a memoryview");
            }
            let out = match memoryview_collect_bytes(ptr) {
                Some(out) => out,
                None => return MoltObject::none().bits(),
            };
            bytes_hex_from_bits(_py, out.as_slice(), sep_bits, bytes_per_sep_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_release(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "release expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "release expects a memoryview");
            }
        }
        // release() currently behaves as a no-op until released-view state is modeled.
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_toreadonly(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_exception::<_>(_py, "TypeError", "toreadonly expects a memoryview");
            }
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "toreadonly expects a memoryview");
            }
            let owner_bits = memoryview_owner_bits(ptr);
            let offset = memoryview_offset(ptr);
            let len = memoryview_len(ptr);
            let itemsize = memoryview_itemsize(ptr);
            let stride = memoryview_stride(ptr);
            let format_bits = memoryview_format_bits(ptr);
            let shape = memoryview_shape(ptr).unwrap_or(&[]).to_vec();
            let strides = memoryview_strides(ptr).unwrap_or(&[]).to_vec();
            let out_ptr = if shape.len() > 1 || memoryview_ndim(ptr) == 0 {
                alloc_memoryview_shaped(
                    _py,
                    owner_bits,
                    offset,
                    itemsize,
                    true,
                    format_bits,
                    shape,
                    strides,
                )
            } else {
                alloc_memoryview(
                    _py,
                    owner_bits,
                    offset,
                    len,
                    itemsize,
                    stride,
                    true,
                    format_bits,
                )
            };
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[repr(C)]
pub struct BufferExport {
    pub ptr: u64,
    pub len: u64,
    pub readonly: u64,
    pub stride: i64,
    pub itemsize: u64,
}

/// # Safety
/// Caller must ensure `out_ptr` is valid and writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_buffer_export(obj_bits: u64, out_ptr: *mut BufferExport) -> i32 {
    unsafe {
        crate::with_gil_entry!(_py, {
            if out_ptr.is_null() {
                return 1;
            }
            let obj = obj_from_bits(obj_bits);
            let ptr = match obj.as_ptr() {
                Some(ptr) => ptr,
                None => return 1,
            };
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let data_ptr = bytes_data(ptr) as u64;
                let len = bytes_len(ptr) as u64;
                let readonly = if type_id == TYPE_ID_BYTES { 1 } else { 0 };
                *out_ptr = BufferExport {
                    ptr: data_ptr,
                    len,
                    readonly,
                    stride: 1,
                    itemsize: 1,
                };
                return 0;
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                let owner_bits = memoryview_owner_bits(ptr);
                let owner = obj_from_bits(owner_bits);
                let owner_ptr = match owner.as_ptr() {
                    Some(ptr) => ptr,
                    None => return 1,
                };
                let base = match bytes_like_slice_raw(owner_ptr) {
                    Some(slice) => slice,
                    None => return 1,
                };
                let offset = memoryview_offset(ptr);
                if offset < 0 {
                    return 1;
                }
                let offset = offset as usize;
                if offset > base.len() {
                    return 1;
                }
                let data_ptr = base.as_ptr().add(offset) as u64;
                let len = memoryview_len(ptr) as u64;
                let readonly = if memoryview_readonly(ptr) { 1 } else { 0 };
                let stride = memoryview_stride(ptr) as i64;
                let itemsize = memoryview_itemsize(ptr) as u64;
                *out_ptr = BufferExport {
                    ptr: data_ptr,
                    len,
                    readonly,
                    stride,
                    itemsize,
                };
                return 0;
            }
            1
        })
    }
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
                if type_id == TYPE_ID_TYPE
                    && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__class_getitem__")
                {
                    if let Some(call_bits) = class_attr_lookup(_py, ptr, ptr, Some(ptr), name_bits)
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
                eprintln!("[MOLT-DEBUG] subscript fail (TYPE_ID_TYPE, no __class_getitem__): class_name={}, obj_bits=0x{:016x}, key_bits=0x{:016x}", class_name, obj_bits, key_bits);
                format!("type '{}' is not subscriptable", class_name)
            } else {
                let tn = type_name(_py, obj);
                let tid = unsafe { object_type_id(ptr) };
                eprintln!("[MOLT-DEBUG] subscript fail (ptr path): type_name={}, type_id={}, obj_bits=0x{:016x}, key_bits=0x{:016x}", tn, tid, obj_bits, key_bits);
                format!("'{}' object is not subscriptable", tn)
            };
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let obj_dbg = obj_from_bits(obj_bits);
        eprintln!("[MOLT-DEBUG] subscript fail (no-ptr path): type_name={}, obj_bits=0x{:016x}, key_bits=0x{:016x}, is_int={}, is_float={}, is_bool={}, is_none={}, is_pending={}", type_name(_py, obj_dbg), obj_bits, key_bits, obj_dbg.is_int(), obj_dbg.is_float(), obj_dbg.is_bool(), obj_dbg.is_none(), obj_dbg.is_pending());
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

pub(super) unsafe fn eq_bool_from_bits(_py: &PyToken<'_>, lhs_bits: u64, rhs_bits: u64) -> Option<bool> {
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
            raise_exception::<u64>(
                _py,
                "TypeError",
                "cannot unpack non-sequence",
            );
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
                    let msg = format!(
                        "too many values to unpack (expected {})",
                        expected
                    );
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
                    raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "cannot unpack non-sequence",
                    );
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
                        dec_ref_bits(_py, iter_bits);
                        let msg = format!(
                            "too many values to unpack (expected {})",
                            expected
                        );
                        raise_exception::<u64>(_py, "ValueError", &msg);
                        return MoltObject::none().bits();
                    }
                }
                dec_ref_bits(_py, iter_bits);
                if count < expected {
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

unsafe fn call_binary_dunder(
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

unsafe fn call_inplace_dunder(
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
            if ltype == TYPE_ID_LIST {
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
            if ltype == TYPE_ID_DICT {
                let l_pairs = dict_order(lp);
                let r_pairs = dict_order(rp);
                if l_pairs.len() != r_pairs.len() {
                    return false;
                }
                let r_table = dict_table(rp);
                let entries = l_pairs.len() / 2;
                for entry_idx in 0..entries {
                    let key_bits = l_pairs[entry_idx * 2];
                    let val_bits = l_pairs[entry_idx * 2 + 1];
                    let Some(r_entry_idx) = dict_find_entry_fast(_py, r_pairs, r_table, key_bits)
                    else {
                        return false;
                    };
                    let r_val_bits = r_pairs[r_entry_idx * 2 + 1];
                    if !obj_eq(_py, obj_from_bits(val_bits), obj_from_bits(r_val_bits)) {
                        return false;
                    }
                }
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
pub(crate) fn dict_rebuild(_py: &PyToken<'_>, order: &[u64], table: &mut Vec<usize>, capacity: usize) {
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
            if len == 0 { return true; }
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }

        // Short strings (9-31 bytes): use NEON/SSE2 16-byte loads instead of
        // scalar memcmp. Two overlapping 16-byte loads cover any length in 9..31
        // without a loop, which is measurably faster for dict-key comparisons
        // where keys are typically short identifiers (< 32 bytes).
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

/// NEON short-string equality for 9-31 bytes: two overlapping 16-byte loads.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn simd_bytes_eq_short_neon(a: *const u8, b: *const u8, len: usize) -> bool {
    use std::arch::aarch64::*;
    debug_assert!(len >= 9 && len < 32);
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

/// SSE2 short-string equality for 9-31 bytes: two overlapping 16-byte loads.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn simd_bytes_eq_short_sse2(a: *const u8, b: *const u8, len: usize) -> bool {
    use std::arch::x86_64::*;
    debug_assert!(len >= 9 && len < 32);
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
    if len == 0 { return false; }

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
            if elem == needle { return true; }
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
        if haystack[i] == needle { return true; }
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
pub(super) fn set_rebuild(_py: &PyToken<'_>, order: &[u64], table: &mut Vec<usize>, capacity: usize) {
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
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {}
            None => return None,
        }
        slot = (slot + 1) & mask;
    }
}

fn concat_bytes_like(_py: &PyToken<'_>, left: &[u8], right: &[u8], type_id: u32) -> Option<u64> {
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

fn fill_repeated_bytes(dst: &mut [u8], pattern: &[u8]) {
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
    use crate::builtins::types::{
        molt_class_apply_set_name, molt_class_new, molt_class_set_base,
        molt_class_set_layout_version,
    };
    use crate::builtins::attributes::molt_set_attr_name;
    use molt_obj_model::MoltObject;

    let none = MoltObject::none().bits();
    let class_bits = molt_class_new(name_bits);
    if class_bits == none {
        return class_bits;
    }

    let nb = nbases as usize;
    if nb > 0 {
        unsafe {
            let bases_slice = std::slice::from_raw_parts(bases_ptr, nb);
            if nb == 1 {
                molt_class_set_base(class_bits, bases_slice[0]);
            } else {
                crate::with_gil_entry!(_py, {
                    let tuple_ptr = crate::object::builders::alloc_tuple(_py, bases_slice);
                    if !tuple_ptr.is_null() {
                        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
                        molt_class_set_base(class_bits, tuple_bits);
                        crate::dec_ref_bits(_py, tuple_bits);
                    }
                });
            }
        }
    }

    let na = nattrs as usize;
    if na > 0 {
        unsafe {
            let attrs_slice = std::slice::from_raw_parts(attrs_ptr, na * 2);
            for pair in attrs_slice.chunks_exact(2) {
                molt_set_attr_name(class_bits, pair[0], pair[1]);
            }
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
        unsafe {
            let bases_slice = std::slice::from_raw_parts(bases_ptr, nb);
            crate::with_gil_entry!(_py, {
                let init_name = crate::intern_static_name(
                    _py,
                    &crate::runtime_state(_py).interned.init_subclass_name,
                    b"__init_subclass__",
                );
                for &base in bases_slice {
                    // Guard: base must be a valid heap pointer (type object).
                    // A CSE alias bug can cause float/int bits to appear in
                    // the base slot — skip non-pointer values to prevent
                    // "'float' object has no attribute '__init__'" crashes.
                    let base_obj = obj_from_bits(base);
                    let Some(base_ptr) = base_obj.as_ptr() else {
                        continue;
                    };
                    if object_type_id(base_ptr) != TYPE_ID_TYPE {
                        continue;
                    }
                    let init_attr = crate::builtins::attributes::molt_get_attr_name_default(
                        base, init_name, none,
                    );
                    if init_attr != none {
                        // __init_subclass__(cls) or __init_subclass__(cls, **kwargs)
                        // The compiled function may have arity 2 when **kwargs
                        // is present; pass an empty dict to satisfy the extra
                        // parameter.  This mirrors CPython's implicit classmethod
                        // wrapping + empty kwargs for class statements without
                        // keyword arguments.
                        let init_obj = obj_from_bits(init_attr);
                        let needs_kwargs = match init_obj.as_ptr() {
                            Some(ptr) if object_type_id(ptr) == TYPE_ID_FUNCTION => {
                                function_arity(ptr) > 1
                            }
                            _ => false,
                        };
                        if needs_kwargs {
                            let empty_dict =
                                crate::builtins::containers_alloc::molt_dict_new(0);
                            let _ = crate::call::dispatch::call_callable2(
                                _py, init_attr, class_bits, empty_dict,
                            );
                            crate::dec_ref_bits(_py, empty_dict);
                        } else {
                            let _ = crate::call::dispatch::call_callable1(
                                _py, init_attr, class_bits,
                            );
                        }
                        crate::dec_ref_bits(_py, init_attr);
                    }
                }
                // init_name is globally interned — do NOT dec_ref it.
            });
        }
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
pub extern "C" fn molt_fstring_build(
    parts_ptr: *const u64,
    n_parts: u64,
) -> u64 {
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
        crate::with_gil_entry!(_py, {
            let val = elems[idx as usize];
            inc_ref_bits(_py, val);
            val
        })
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

