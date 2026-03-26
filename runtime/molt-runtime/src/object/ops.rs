// Re-export iter impl functions for backward compatibility with crate::object::ops::* paths

// Re-exports for backward compatibility with crate::object::ops::* paths
pub use crate::object::ops_arith::*;
pub use crate::object::ops_compare::*;
pub use crate::object::ops_convert::*;
pub use crate::object::ops_sys::*;
pub use crate::object::ops_builtins::*;

pub(crate) use crate::object::ops_iter::{
    enumerate_new_impl, filter_new_impl, map_new_impl, reversed_new_impl, zip_new_impl,
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
pub(crate) fn debug_index_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_INDEX").as_deref() == Ok("1"))
}

#[inline]
pub(crate) fn debug_index_list_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("MOLT_DEBUG_INDEX_LIST").as_deref() == Ok("1"))
}

#[inline]
pub(crate) fn debug_store_index_enabled() -> bool {
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


#[derive(Clone, Copy)]
pub(crate) enum EncodingKind {
    Utf8,
    Utf8Sig,
    Cp1252,
    Cp437,
    Cp850,
    Cp860,
    Cp862,
    Cp863,
    Cp865,
    Cp866,
    Cp874,
    Cp1250,
    Cp1251,
    Cp1253,
    Cp1254,
    Cp1255,
    Cp1256,
    Cp1257,
    Koi8R,
    Koi8U,
    Iso8859_2,
    Iso8859_3,
    Iso8859_4,
    Iso8859_5,
    Iso8859_6,
    Iso8859_7,
    Iso8859_8,
    Iso8859_10,
    Iso8859_15,
    MacRoman,
    Latin1,
    Ascii,
    UnicodeEscape,
    Utf16,
    Utf16LE,
    Utf16BE,
    Utf32,
    Utf32LE,
    Utf32BE,
}

impl EncodingKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            EncodingKind::Utf8 => "utf-8",
            EncodingKind::Utf8Sig => "utf-8-sig",
            EncodingKind::Cp1252 => "cp1252",
            EncodingKind::Cp437 => "cp437",
            EncodingKind::Cp850 => "cp850",
            EncodingKind::Cp860 => "cp860",
            EncodingKind::Cp862 => "cp862",
            EncodingKind::Cp863 => "cp863",
            EncodingKind::Cp865 => "cp865",
            EncodingKind::Cp866 => "cp866",
            EncodingKind::Cp874 => "cp874",
            EncodingKind::Cp1250 => "cp1250",
            EncodingKind::Cp1251 => "cp1251",
            EncodingKind::Cp1253 => "cp1253",
            EncodingKind::Cp1254 => "cp1254",
            EncodingKind::Cp1255 => "cp1255",
            EncodingKind::Cp1256 => "cp1256",
            EncodingKind::Cp1257 => "cp1257",
            EncodingKind::Koi8R => "koi8-r",
            EncodingKind::Koi8U => "koi8-u",
            EncodingKind::Iso8859_2 => "iso8859-2",
            EncodingKind::Iso8859_3 => "iso8859-3",
            EncodingKind::Iso8859_4 => "iso8859-4",
            EncodingKind::Iso8859_5 => "iso8859-5",
            EncodingKind::Iso8859_6 => "iso8859-6",
            EncodingKind::Iso8859_7 => "iso8859-7",
            EncodingKind::Iso8859_8 => "iso8859-8",
            EncodingKind::Iso8859_10 => "iso8859-10",
            EncodingKind::Iso8859_15 => "iso8859-15",
            EncodingKind::MacRoman => "mac-roman",
            EncodingKind::Latin1 => "latin-1",
            EncodingKind::Ascii => "ascii",
            EncodingKind::UnicodeEscape => "unicode-escape",
            EncodingKind::Utf16 => "utf-16",
            EncodingKind::Utf16LE => "utf-16-le",
            EncodingKind::Utf16BE => "utf-16-be",
            EncodingKind::Utf32 => "utf-32",
            EncodingKind::Utf32LE => "utf-32-le",
            EncodingKind::Utf32BE => "utf-32-be",
        }
    }

    fn ordinal_limit(self) -> u32 {
        match self {
            EncodingKind::Ascii => 128,
            EncodingKind::Latin1 => 256,
            EncodingKind::UnicodeEscape => u32::MAX,
            EncodingKind::Cp1252 => u32::MAX,
            EncodingKind::Cp437 => u32::MAX,
            EncodingKind::Cp850 => u32::MAX,
            EncodingKind::Cp860 => u32::MAX,
            EncodingKind::Cp862 => u32::MAX,
            EncodingKind::Cp863 => u32::MAX,
            EncodingKind::Cp865 => u32::MAX,
            EncodingKind::Cp866 => u32::MAX,
            EncodingKind::Cp874 => u32::MAX,
            EncodingKind::Cp1250 => u32::MAX,
            EncodingKind::Cp1251 => u32::MAX,
            EncodingKind::Cp1253 => u32::MAX,
            EncodingKind::Cp1254 => u32::MAX,
            EncodingKind::Cp1255 => u32::MAX,
            EncodingKind::Cp1256 => u32::MAX,
            EncodingKind::Cp1257 => u32::MAX,
            EncodingKind::Koi8R => u32::MAX,
            EncodingKind::Koi8U => u32::MAX,
            EncodingKind::Iso8859_2 => u32::MAX,
            EncodingKind::Iso8859_3 => u32::MAX,
            EncodingKind::Iso8859_4 => u32::MAX,
            EncodingKind::Iso8859_5 => u32::MAX,
            EncodingKind::Iso8859_6 => u32::MAX,
            EncodingKind::Iso8859_7 => u32::MAX,
            EncodingKind::Iso8859_8 => u32::MAX,
            EncodingKind::Iso8859_10 => u32::MAX,
            EncodingKind::Iso8859_15 => u32::MAX,
            EncodingKind::MacRoman => u32::MAX,
            EncodingKind::Utf8
            | EncodingKind::Utf8Sig
            | EncodingKind::Utf16
            | EncodingKind::Utf16LE
            | EncodingKind::Utf16BE
            | EncodingKind::Utf32
            | EncodingKind::Utf32LE
            | EncodingKind::Utf32BE => u32::MAX,
        }
    }
}

pub(crate) fn encoding_kind_name(kind: EncodingKind) -> &'static str {
    kind.name()
}

pub(crate) enum EncodeError {
    UnknownEncoding(String),
    UnknownErrorHandler(String),
    InvalidChar {
        encoding: &'static str,
        code: u32,
        pos: usize,
        limit: u32,
    },
}

pub(crate) fn normalize_encoding(name: &str) -> Option<EncodingKind> {
    let normalized = name.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "utf-8" | "utf8" => Some(EncodingKind::Utf8),
        "utf-8-sig" | "utf8-sig" => Some(EncodingKind::Utf8Sig),
        "cp1252" | "cp-1252" | "windows-1252" => Some(EncodingKind::Cp1252),
        "cp437" | "ibm437" | "437" => Some(EncodingKind::Cp437),
        "cp850" | "ibm850" | "850" | "cp-850" => Some(EncodingKind::Cp850),
        "cp860" | "ibm860" | "860" | "cp-860" => Some(EncodingKind::Cp860),
        "cp862" | "ibm862" | "862" | "cp-862" => Some(EncodingKind::Cp862),
        "cp863" | "ibm863" | "863" | "cp-863" => Some(EncodingKind::Cp863),
        "cp865" | "ibm865" | "865" | "cp-865" => Some(EncodingKind::Cp865),
        "cp866" | "ibm866" | "866" | "cp-866" => Some(EncodingKind::Cp866),
        "cp874" | "cp-874" | "windows-874" => Some(EncodingKind::Cp874),
        "cp1250" | "cp-1250" | "windows-1250" => Some(EncodingKind::Cp1250),
        "cp1251" | "cp-1251" | "windows-1251" => Some(EncodingKind::Cp1251),
        "cp1253" | "cp-1253" | "windows-1253" => Some(EncodingKind::Cp1253),
        "cp1254" | "cp-1254" | "windows-1254" => Some(EncodingKind::Cp1254),
        "cp1255" | "cp-1255" | "windows-1255" => Some(EncodingKind::Cp1255),
        "cp1256" | "cp-1256" | "windows-1256" => Some(EncodingKind::Cp1256),
        "cp1257" | "cp-1257" | "windows-1257" => Some(EncodingKind::Cp1257),
        "koi8-r" | "koi8r" | "koi8_r" => Some(EncodingKind::Koi8R),
        "koi8-u" | "koi8u" | "koi8_u" => Some(EncodingKind::Koi8U),
        "iso-8859-2" | "iso8859-2" | "latin2" | "latin-2" => Some(EncodingKind::Iso8859_2),
        "iso-8859-3" | "iso8859-3" | "latin3" | "latin-3" => Some(EncodingKind::Iso8859_3),
        "iso-8859-4" | "iso8859-4" | "latin4" | "latin-4" => Some(EncodingKind::Iso8859_4),
        "iso-8859-5" | "iso8859-5" | "cyrillic" => Some(EncodingKind::Iso8859_5),
        "iso-8859-6" | "iso8859-6" | "arabic" => Some(EncodingKind::Iso8859_6),
        "iso-8859-7" | "iso8859-7" | "greek" => Some(EncodingKind::Iso8859_7),
        "iso-8859-8" | "iso8859-8" | "hebrew" => Some(EncodingKind::Iso8859_8),
        "iso-8859-10" | "iso8859-10" | "latin6" | "latin-6" => Some(EncodingKind::Iso8859_10),
        "iso-8859-15" | "iso8859-15" | "latin9" | "latin-9" | "latin_9" => {
            Some(EncodingKind::Iso8859_15)
        }
        "mac-roman" | "macroman" | "mac_roman" => Some(EncodingKind::MacRoman),
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" => Some(EncodingKind::Latin1),
        "ascii" | "us-ascii" => Some(EncodingKind::Ascii),
        "unicode-escape" | "unicodeescape" => Some(EncodingKind::UnicodeEscape),
        "utf-16" | "utf16" => Some(EncodingKind::Utf16),
        "utf-16le" | "utf-16-le" | "utf16le" => Some(EncodingKind::Utf16LE),
        "utf-16be" | "utf-16-be" | "utf16be" => Some(EncodingKind::Utf16BE),
        "utf-32" | "utf32" => Some(EncodingKind::Utf32),
        "utf-32le" | "utf-32-le" | "utf32le" => Some(EncodingKind::Utf32LE),
        "utf-32be" | "utf-32-be" | "utf32be" => Some(EncodingKind::Utf32BE),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Endian {
    Little,
    Big,
}

fn native_endian() -> Endian {
    if cfg!(target_endian = "big") {
        Endian::Big
    } else {
        Endian::Little
    }
}

fn push_u16(out: &mut Vec<u8>, val: u16, endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(&val.to_le_bytes()),
        Endian::Big => out.extend_from_slice(&val.to_be_bytes()),
    }
}

fn push_u32(out: &mut Vec<u8>, val: u32, endian: Endian) {
    match endian {
        Endian::Little => out.extend_from_slice(&val.to_le_bytes()),
        Endian::Big => out.extend_from_slice(&val.to_be_bytes()),
    }
}

#[allow(dead_code)]
fn encode_utf16(text: &str, endian: Endian, with_bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len().saturating_mul(2) + if with_bom { 2 } else { 0 });
    if with_bom {
        push_u16(&mut out, 0xFEFF, endian);
    }
    for code in text.encode_utf16() {
        push_u16(&mut out, code, endian);
    }
    out
}

#[allow(dead_code)]
fn encode_utf32(text: &str, endian: Endian, with_bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len().saturating_mul(4) + if with_bom { 4 } else { 0 });
    if with_bom {
        push_u32(&mut out, 0x0000_FEFF, endian);
    }
    for ch in text.chars() {
        push_u32(&mut out, ch as u32, endian);
    }
    out
}

fn is_surrogate(code: u32) -> bool {
    (0xD800..=0xDFFF).contains(&code)
}

fn unicode_escape_codepoint(code: u32) -> String {
    if code <= 0xFF {
        format!("\\x{code:02x}")
    } else if code <= 0xFFFF {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

fn unicode_name_escape(code: u32) -> String {
    #[cfg(feature = "stdlib_unicode_names")]
    if let Some(ch) = char::from_u32(code)
        && let Some(name) = unicode_names2::name(ch)
    {
        return format!("\\N{{{name}}}");
    }
    unicode_escape_codepoint(code)
}

fn unicode_escape(ch: char) -> String {
    unicode_escape_codepoint(ch as u32)
}

pub(crate) fn encode_error_reason(encoding: &str, code: u32, limit: u32) -> String {
    if encoding == "charmap" {
        return "character maps to <undefined>".to_string();
    }
    if is_surrogate(code) && encoding.starts_with("utf-") {
        return "surrogates not allowed".to_string();
    }
    format!("ordinal not in range({limit})")
}

#[allow(dead_code)]
fn push_backslash_bytes(out: &mut String, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in bytes {
        out.push('\\');
        out.push('x');
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
}

fn push_backslash_bytes_vec(out: &mut Vec<u8>, bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &byte in bytes {
        out.push(b'\\');
        out.push(b'x');
        out.push(HEX[(byte >> 4) as usize]);
        out.push(HEX[(byte & 0x0f) as usize]);
    }
}

fn push_hex_escape(out: &mut Vec<u8>, prefix: u8, code: u32, width: usize) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'\\');
    out.push(prefix);
    for shift in (0..width).rev() {
        let nibble = ((code >> (shift * 4)) & 0x0f) as usize;
        out.push(HEX[nibble]);
    }
}

fn xmlcharref_bytes(code: u32, buf: &mut [u8; 16]) -> &[u8] {
    buf[0] = b'&';
    buf[1] = b'#';
    let mut digits = [0u8; 10];
    let mut idx = digits.len();
    let mut value = code;
    loop {
        idx = idx.saturating_sub(1);
        digits[idx] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let digits_len = digits.len() - idx;
    buf[2..2 + digits_len].copy_from_slice(&digits[idx..]);
    buf[2 + digits_len] = b';';
    &buf[..2 + digits_len + 1]
}

fn push_xmlcharref_ascii(out: &mut Vec<u8>, code: u32) {
    let mut buf = [0u8; 16];
    let bytes = xmlcharref_bytes(code, &mut buf);
    out.extend_from_slice(bytes);
}

fn push_xmlcharref_utf16(out: &mut Vec<u8>, code: u32, endian: Endian) {
    let mut buf = [0u8; 16];
    let bytes = xmlcharref_bytes(code, &mut buf);
    for &byte in bytes {
        push_u16(out, byte as u16, endian);
    }
}

fn push_xmlcharref_utf32(out: &mut Vec<u8>, code: u32, endian: Endian) {
    let mut buf = [0u8; 16];
    let bytes = xmlcharref_bytes(code, &mut buf);
    for &byte in bytes {
        push_u32(out, byte as u32, endian);
    }
}

fn encode_cp1252_byte(code: u32) -> Option<u8> {
    if code <= 0x7F || (0xA0..=0xFF).contains(&code) {
        return Some(code as u8);
    }
    match code {
        0x20AC => Some(0x80),
        0x201A => Some(0x82),
        0x0192 => Some(0x83),
        0x201E => Some(0x84),
        0x2026 => Some(0x85),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x02C6 => Some(0x88),
        0x2030 => Some(0x89),
        0x0160 => Some(0x8A),
        0x2039 => Some(0x8B),
        0x0152 => Some(0x8C),
        0x017D => Some(0x8E),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x2022 => Some(0x95),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x02DC => Some(0x98),
        0x2122 => Some(0x99),
        0x0161 => Some(0x9A),
        0x203A => Some(0x9B),
        0x0153 => Some(0x9C),
        0x017E => Some(0x9E),
        0x0178 => Some(0x9F),
        _ => None,
    }
}

fn encode_cp437_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xFF),
        0x00A1 => Some(0xAD),
        0x00A2 => Some(0x9B),
        0x00A3 => Some(0x9C),
        0x00A5 => Some(0x9D),
        0x00AA => Some(0xA6),
        0x00AB => Some(0xAE),
        0x00AC => Some(0xAA),
        0x00B0 => Some(0xF8),
        0x00B1 => Some(0xF1),
        0x00B2 => Some(0xFD),
        0x00B5 => Some(0xE6),
        0x00B7 => Some(0xFA),
        0x00BA => Some(0xA7),
        0x00BB => Some(0xAF),
        0x00BC => Some(0xAC),
        0x00BD => Some(0xAB),
        0x00BF => Some(0xA8),
        0x00C4 => Some(0x8E),
        0x00C5 => Some(0x8F),
        0x00C6 => Some(0x92),
        0x00C7 => Some(0x80),
        0x00C9 => Some(0x90),
        0x00D1 => Some(0xA5),
        0x00D6 => Some(0x99),
        0x00DC => Some(0x9A),
        0x00DF => Some(0xE1),
        0x00E0 => Some(0x85),
        0x00E1 => Some(0xA0),
        0x00E2 => Some(0x83),
        0x00E4 => Some(0x84),
        0x00E5 => Some(0x86),
        0x00E6 => Some(0x91),
        0x00E7 => Some(0x87),
        0x00E8 => Some(0x8A),
        0x00E9 => Some(0x82),
        0x00EA => Some(0x88),
        0x00EB => Some(0x89),
        0x00EC => Some(0x8D),
        0x00ED => Some(0xA1),
        0x00EE => Some(0x8C),
        0x00EF => Some(0x8B),
        0x00F1 => Some(0xA4),
        0x00F2 => Some(0x95),
        0x00F3 => Some(0xA2),
        0x00F4 => Some(0x93),
        0x00F6 => Some(0x94),
        0x00F7 => Some(0xF6),
        0x00F9 => Some(0x97),
        0x00FA => Some(0xA3),
        0x00FB => Some(0x96),
        0x00FC => Some(0x81),
        0x00FF => Some(0x98),
        0x0192 => Some(0x9F),
        0x0393 => Some(0xE2),
        0x0398 => Some(0xE9),
        0x03A3 => Some(0xE4),
        0x03A6 => Some(0xE8),
        0x03A9 => Some(0xEA),
        0x03B1 => Some(0xE0),
        0x03B4 => Some(0xEB),
        0x03B5 => Some(0xEE),
        0x03C0 => Some(0xE3),
        0x03C3 => Some(0xE5),
        0x03C4 => Some(0xE7),
        0x03C6 => Some(0xED),
        0x207F => Some(0xFC),
        0x20A7 => Some(0x9E),
        0x2219 => Some(0xF9),
        0x221A => Some(0xFB),
        0x221E => Some(0xEC),
        0x2229 => Some(0xEF),
        0x2248 => Some(0xF7),
        0x2261 => Some(0xF0),
        0x2264 => Some(0xF3),
        0x2265 => Some(0xF2),
        0x2310 => Some(0xA9),
        0x2320 => Some(0xF4),
        0x2321 => Some(0xF5),
        0x2500 => Some(0xC4),
        0x2502 => Some(0xB3),
        0x250C => Some(0xDA),
        0x2510 => Some(0xBF),
        0x2514 => Some(0xC0),
        0x2518 => Some(0xD9),
        0x251C => Some(0xC3),
        0x2524 => Some(0xB4),
        0x252C => Some(0xC2),
        0x2534 => Some(0xC1),
        0x253C => Some(0xC5),
        0x2550 => Some(0xCD),
        0x2551 => Some(0xBA),
        0x2552 => Some(0xD5),
        0x2553 => Some(0xD6),
        0x2554 => Some(0xC9),
        0x2555 => Some(0xB8),
        0x2556 => Some(0xB7),
        0x2557 => Some(0xBB),
        0x2558 => Some(0xD4),
        0x2559 => Some(0xD3),
        0x255A => Some(0xC8),
        0x255B => Some(0xBE),
        0x255C => Some(0xBD),
        0x255D => Some(0xBC),
        0x255E => Some(0xC6),
        0x255F => Some(0xC7),
        0x2560 => Some(0xCC),
        0x2561 => Some(0xB5),
        0x2562 => Some(0xB6),
        0x2563 => Some(0xB9),
        0x2564 => Some(0xD1),
        0x2565 => Some(0xD2),
        0x2566 => Some(0xCB),
        0x2567 => Some(0xCF),
        0x2568 => Some(0xD0),
        0x2569 => Some(0xCA),
        0x256A => Some(0xD8),
        0x256B => Some(0xD7),
        0x256C => Some(0xCE),
        0x2580 => Some(0xDF),
        0x2584 => Some(0xDC),
        0x2588 => Some(0xDB),
        0x258C => Some(0xDD),
        0x2590 => Some(0xDE),
        0x2591 => Some(0xB0),
        0x2592 => Some(0xB1),
        0x2593 => Some(0xB2),
        0x25A0 => Some(0xFE),
        _ => None,
    }
}

fn encode_cp850_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xFF),
        0x00A1 => Some(0xAD),
        0x00A2 => Some(0xBD),
        0x00A3 => Some(0x9C),
        0x00A4 => Some(0xCF),
        0x00A5 => Some(0xBE),
        0x00A6 => Some(0xDD),
        0x00A7 => Some(0xF5),
        0x00A8 => Some(0xF9),
        0x00A9 => Some(0xB8),
        0x00AA => Some(0xA6),
        0x00AB => Some(0xAE),
        0x00AC => Some(0xAA),
        0x00AD => Some(0xF0),
        0x00AE => Some(0xA9),
        0x00AF => Some(0xEE),
        0x00B0 => Some(0xF8),
        0x00B1 => Some(0xF1),
        0x00B2 => Some(0xFD),
        0x00B3 => Some(0xFC),
        0x00B4 => Some(0xEF),
        0x00B5 => Some(0xE6),
        0x00B6 => Some(0xF4),
        0x00B7 => Some(0xFA),
        0x00B8 => Some(0xF7),
        0x00B9 => Some(0xFB),
        0x00BA => Some(0xA7),
        0x00BB => Some(0xAF),
        0x00BC => Some(0xAC),
        0x00BD => Some(0xAB),
        0x00BE => Some(0xF3),
        0x00BF => Some(0xA8),
        0x00C0 => Some(0xB7),
        0x00C1 => Some(0xB5),
        0x00C2 => Some(0xB6),
        0x00C3 => Some(0xC7),
        0x00C4 => Some(0x8E),
        0x00C5 => Some(0x8F),
        0x00C6 => Some(0x92),
        0x00C7 => Some(0x80),
        0x00C8 => Some(0xD4),
        0x00C9 => Some(0x90),
        0x00CA => Some(0xD2),
        0x00CB => Some(0xD3),
        0x00CC => Some(0xDE),
        0x00CD => Some(0xD6),
        0x00CE => Some(0xD7),
        0x00CF => Some(0xD8),
        0x00D0 => Some(0xD1),
        0x00D1 => Some(0xA5),
        0x00D2 => Some(0xE3),
        0x00D3 => Some(0xE0),
        0x00D4 => Some(0xE2),
        0x00D5 => Some(0xE5),
        0x00D6 => Some(0x99),
        0x00D7 => Some(0x9E),
        0x00D8 => Some(0x9D),
        0x00D9 => Some(0xEB),
        0x00DA => Some(0xE9),
        0x00DB => Some(0xEA),
        0x00DC => Some(0x9A),
        0x00DD => Some(0xED),
        0x00DE => Some(0xE8),
        0x00DF => Some(0xE1),
        0x00E0 => Some(0x85),
        0x00E1 => Some(0xA0),
        0x00E2 => Some(0x83),
        0x00E3 => Some(0xC6),
        0x00E4 => Some(0x84),
        0x00E5 => Some(0x86),
        0x00E6 => Some(0x91),
        0x00E7 => Some(0x87),
        0x00E8 => Some(0x8A),
        0x00E9 => Some(0x82),
        0x00EA => Some(0x88),
        0x00EB => Some(0x89),
        0x00EC => Some(0x8D),
        0x00ED => Some(0xA1),
        0x00EE => Some(0x8C),
        0x00EF => Some(0x8B),
        0x00F0 => Some(0xD0),
        0x00F1 => Some(0xA4),
        0x00F2 => Some(0x95),
        0x00F3 => Some(0xA2),
        0x00F4 => Some(0x93),
        0x00F5 => Some(0xE4),
        0x00F6 => Some(0x94),
        0x00F7 => Some(0xF6),
        0x00F8 => Some(0x9B),
        0x00F9 => Some(0x97),
        0x00FA => Some(0xA3),
        0x00FB => Some(0x96),
        0x00FC => Some(0x81),
        0x00FD => Some(0xEC),
        0x00FE => Some(0xE7),
        0x00FF => Some(0x98),
        0x0131 => Some(0xD5),
        0x0192 => Some(0x9F),
        0x2017 => Some(0xF2),
        0x2500 => Some(0xC4),
        0x2502 => Some(0xB3),
        0x250C => Some(0xDA),
        0x2510 => Some(0xBF),
        0x2514 => Some(0xC0),
        0x2518 => Some(0xD9),
        0x251C => Some(0xC3),
        0x2524 => Some(0xB4),
        0x252C => Some(0xC2),
        0x2534 => Some(0xC1),
        0x253C => Some(0xC5),
        0x2550 => Some(0xCD),
        0x2551 => Some(0xBA),
        0x2554 => Some(0xC9),
        0x2557 => Some(0xBB),
        0x255A => Some(0xC8),
        0x255D => Some(0xBC),
        0x2560 => Some(0xCC),
        0x2563 => Some(0xB9),
        0x2566 => Some(0xCB),
        0x2569 => Some(0xCA),
        0x256C => Some(0xCE),
        0x2580 => Some(0xDF),
        0x2584 => Some(0xDC),
        0x2588 => Some(0xDB),
        0x2591 => Some(0xB0),
        0x2592 => Some(0xB1),
        0x2593 => Some(0xB2),
        0x25A0 => Some(0xFE),
        _ => None,
    }
}

fn encode_cp865_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xFF),
        0x00A1 => Some(0xAD),
        0x00A3 => Some(0x9C),
        0x00A4 => Some(0xAF),
        0x00AA => Some(0xA6),
        0x00AB => Some(0xAE),
        0x00AC => Some(0xAA),
        0x00B0 => Some(0xF8),
        0x00B1 => Some(0xF1),
        0x00B2 => Some(0xFD),
        0x00B5 => Some(0xE6),
        0x00B7 => Some(0xFA),
        0x00BA => Some(0xA7),
        0x00BC => Some(0xAC),
        0x00BD => Some(0xAB),
        0x00BF => Some(0xA8),
        0x00C4 => Some(0x8E),
        0x00C5 => Some(0x8F),
        0x00C6 => Some(0x92),
        0x00C7 => Some(0x80),
        0x00C9 => Some(0x90),
        0x00D1 => Some(0xA5),
        0x00D6 => Some(0x99),
        0x00D8 => Some(0x9D),
        0x00DC => Some(0x9A),
        0x00DF => Some(0xE1),
        0x00E0 => Some(0x85),
        0x00E1 => Some(0xA0),
        0x00E2 => Some(0x83),
        0x00E4 => Some(0x84),
        0x00E5 => Some(0x86),
        0x00E6 => Some(0x91),
        0x00E7 => Some(0x87),
        0x00E8 => Some(0x8A),
        0x00E9 => Some(0x82),
        0x00EA => Some(0x88),
        0x00EB => Some(0x89),
        0x00EC => Some(0x8D),
        0x00ED => Some(0xA1),
        0x00EE => Some(0x8C),
        0x00EF => Some(0x8B),
        0x00F1 => Some(0xA4),
        0x00F2 => Some(0x95),
        0x00F3 => Some(0xA2),
        0x00F4 => Some(0x93),
        0x00F6 => Some(0x94),
        0x00F7 => Some(0xF6),
        0x00F8 => Some(0x9B),
        0x00F9 => Some(0x97),
        0x00FA => Some(0xA3),
        0x00FB => Some(0x96),
        0x00FC => Some(0x81),
        0x00FF => Some(0x98),
        0x0192 => Some(0x9F),
        0x0393 => Some(0xE2),
        0x0398 => Some(0xE9),
        0x03A3 => Some(0xE4),
        0x03A6 => Some(0xE8),
        0x03A9 => Some(0xEA),
        0x03B1 => Some(0xE0),
        0x03B4 => Some(0xEB),
        0x03B5 => Some(0xEE),
        0x03C0 => Some(0xE3),
        0x03C3 => Some(0xE5),
        0x03C4 => Some(0xE7),
        0x03C6 => Some(0xED),
        0x207F => Some(0xFC),
        0x20A7 => Some(0x9E),
        0x2219 => Some(0xF9),
        0x221A => Some(0xFB),
        0x221E => Some(0xEC),
        0x2229 => Some(0xEF),
        0x2248 => Some(0xF7),
        0x2261 => Some(0xF0),
        0x2264 => Some(0xF3),
        0x2265 => Some(0xF2),
        0x2310 => Some(0xA9),
        0x2320 => Some(0xF4),
        0x2321 => Some(0xF5),
        0x2500 => Some(0xC4),
        0x2502 => Some(0xB3),
        0x250C => Some(0xDA),
        0x2510 => Some(0xBF),
        0x2514 => Some(0xC0),
        0x2518 => Some(0xD9),
        0x251C => Some(0xC3),
        0x2524 => Some(0xB4),
        0x252C => Some(0xC2),
        0x2534 => Some(0xC1),
        0x253C => Some(0xC5),
        0x2550 => Some(0xCD),
        0x2551 => Some(0xBA),
        0x2552 => Some(0xD5),
        0x2553 => Some(0xD6),
        0x2554 => Some(0xC9),
        0x2555 => Some(0xB8),
        0x2556 => Some(0xB7),
        0x2557 => Some(0xBB),
        0x2558 => Some(0xD4),
        0x2559 => Some(0xD3),
        0x255A => Some(0xC8),
        0x255B => Some(0xBE),
        0x255C => Some(0xBD),
        0x255D => Some(0xBC),
        0x255E => Some(0xC6),
        0x255F => Some(0xC7),
        0x2560 => Some(0xCC),
        0x2561 => Some(0xB5),
        0x2562 => Some(0xB6),
        0x2563 => Some(0xB9),
        0x2564 => Some(0xD1),
        0x2565 => Some(0xD2),
        0x2566 => Some(0xCB),
        0x2567 => Some(0xCF),
        0x2568 => Some(0xD0),
        0x2569 => Some(0xCA),
        0x256A => Some(0xD8),
        0x256B => Some(0xD7),
        0x256C => Some(0xCE),
        0x2580 => Some(0xDF),
        0x2584 => Some(0xDC),
        0x2588 => Some(0xDB),
        0x258C => Some(0xDD),
        0x2590 => Some(0xDE),
        0x2591 => Some(0xB0),
        0x2592 => Some(0xB1),
        0x2593 => Some(0xB2),
        0x25A0 => Some(0xFE),
        _ => None,
    }
}

fn encode_cp874_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xA0),
        0x0E01 => Some(0xA1),
        0x0E02 => Some(0xA2),
        0x0E03 => Some(0xA3),
        0x0E04 => Some(0xA4),
        0x0E05 => Some(0xA5),
        0x0E06 => Some(0xA6),
        0x0E07 => Some(0xA7),
        0x0E08 => Some(0xA8),
        0x0E09 => Some(0xA9),
        0x0E0A => Some(0xAA),
        0x0E0B => Some(0xAB),
        0x0E0C => Some(0xAC),
        0x0E0D => Some(0xAD),
        0x0E0E => Some(0xAE),
        0x0E0F => Some(0xAF),
        0x0E10 => Some(0xB0),
        0x0E11 => Some(0xB1),
        0x0E12 => Some(0xB2),
        0x0E13 => Some(0xB3),
        0x0E14 => Some(0xB4),
        0x0E15 => Some(0xB5),
        0x0E16 => Some(0xB6),
        0x0E17 => Some(0xB7),
        0x0E18 => Some(0xB8),
        0x0E19 => Some(0xB9),
        0x0E1A => Some(0xBA),
        0x0E1B => Some(0xBB),
        0x0E1C => Some(0xBC),
        0x0E1D => Some(0xBD),
        0x0E1E => Some(0xBE),
        0x0E1F => Some(0xBF),
        0x0E20 => Some(0xC0),
        0x0E21 => Some(0xC1),
        0x0E22 => Some(0xC2),
        0x0E23 => Some(0xC3),
        0x0E24 => Some(0xC4),
        0x0E25 => Some(0xC5),
        0x0E26 => Some(0xC6),
        0x0E27 => Some(0xC7),
        0x0E28 => Some(0xC8),
        0x0E29 => Some(0xC9),
        0x0E2A => Some(0xCA),
        0x0E2B => Some(0xCB),
        0x0E2C => Some(0xCC),
        0x0E2D => Some(0xCD),
        0x0E2E => Some(0xCE),
        0x0E2F => Some(0xCF),
        0x0E30 => Some(0xD0),
        0x0E31 => Some(0xD1),
        0x0E32 => Some(0xD2),
        0x0E33 => Some(0xD3),
        0x0E34 => Some(0xD4),
        0x0E35 => Some(0xD5),
        0x0E36 => Some(0xD6),
        0x0E37 => Some(0xD7),
        0x0E38 => Some(0xD8),
        0x0E39 => Some(0xD9),
        0x0E3A => Some(0xDA),
        0x0E3F => Some(0xDF),
        0x0E40 => Some(0xE0),
        0x0E41 => Some(0xE1),
        0x0E42 => Some(0xE2),
        0x0E43 => Some(0xE3),
        0x0E44 => Some(0xE4),
        0x0E45 => Some(0xE5),
        0x0E46 => Some(0xE6),
        0x0E47 => Some(0xE7),
        0x0E48 => Some(0xE8),
        0x0E49 => Some(0xE9),
        0x0E4A => Some(0xEA),
        0x0E4B => Some(0xEB),
        0x0E4C => Some(0xEC),
        0x0E4D => Some(0xED),
        0x0E4E => Some(0xEE),
        0x0E4F => Some(0xEF),
        0x0E50 => Some(0xF0),
        0x0E51 => Some(0xF1),
        0x0E52 => Some(0xF2),
        0x0E53 => Some(0xF3),
        0x0E54 => Some(0xF4),
        0x0E55 => Some(0xF5),
        0x0E56 => Some(0xF6),
        0x0E57 => Some(0xF7),
        0x0E58 => Some(0xF8),
        0x0E59 => Some(0xF9),
        0x0E5A => Some(0xFA),
        0x0E5B => Some(0xFB),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x2022 => Some(0x95),
        0x2026 => Some(0x85),
        0x20AC => Some(0x80),
        _ => None,
    }
}

fn encode_cp1250_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xA0),
        0x00A4 => Some(0xA4),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00A9 => Some(0xA9),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B4 => Some(0xB4),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00B8 => Some(0xB8),
        0x00BB => Some(0xBB),
        0x00C1 => Some(0xC1),
        0x00C2 => Some(0xC2),
        0x00C4 => Some(0xC4),
        0x00C7 => Some(0xC7),
        0x00C9 => Some(0xC9),
        0x00CB => Some(0xCB),
        0x00CD => Some(0xCD),
        0x00CE => Some(0xCE),
        0x00D3 => Some(0xD3),
        0x00D4 => Some(0xD4),
        0x00D6 => Some(0xD6),
        0x00D7 => Some(0xD7),
        0x00DA => Some(0xDA),
        0x00DC => Some(0xDC),
        0x00DD => Some(0xDD),
        0x00DF => Some(0xDF),
        0x00E1 => Some(0xE1),
        0x00E2 => Some(0xE2),
        0x00E4 => Some(0xE4),
        0x00E7 => Some(0xE7),
        0x00E9 => Some(0xE9),
        0x00EB => Some(0xEB),
        0x00ED => Some(0xED),
        0x00EE => Some(0xEE),
        0x00F3 => Some(0xF3),
        0x00F4 => Some(0xF4),
        0x00F6 => Some(0xF6),
        0x00F7 => Some(0xF7),
        0x00FA => Some(0xFA),
        0x00FC => Some(0xFC),
        0x00FD => Some(0xFD),
        0x0102 => Some(0xC3),
        0x0103 => Some(0xE3),
        0x0104 => Some(0xA5),
        0x0105 => Some(0xB9),
        0x0106 => Some(0xC6),
        0x0107 => Some(0xE6),
        0x010C => Some(0xC8),
        0x010D => Some(0xE8),
        0x010E => Some(0xCF),
        0x010F => Some(0xEF),
        0x0110 => Some(0xD0),
        0x0111 => Some(0xF0),
        0x0118 => Some(0xCA),
        0x0119 => Some(0xEA),
        0x011A => Some(0xCC),
        0x011B => Some(0xEC),
        0x0139 => Some(0xC5),
        0x013A => Some(0xE5),
        0x013D => Some(0xBC),
        0x013E => Some(0xBE),
        0x0141 => Some(0xA3),
        0x0142 => Some(0xB3),
        0x0143 => Some(0xD1),
        0x0144 => Some(0xF1),
        0x0147 => Some(0xD2),
        0x0148 => Some(0xF2),
        0x0150 => Some(0xD5),
        0x0151 => Some(0xF5),
        0x0154 => Some(0xC0),
        0x0155 => Some(0xE0),
        0x0158 => Some(0xD8),
        0x0159 => Some(0xF8),
        0x015A => Some(0x8C),
        0x015B => Some(0x9C),
        0x015E => Some(0xAA),
        0x015F => Some(0xBA),
        0x0160 => Some(0x8A),
        0x0161 => Some(0x9A),
        0x0162 => Some(0xDE),
        0x0163 => Some(0xFE),
        0x0164 => Some(0x8D),
        0x0165 => Some(0x9D),
        0x016E => Some(0xD9),
        0x016F => Some(0xF9),
        0x0170 => Some(0xDB),
        0x0171 => Some(0xFB),
        0x0179 => Some(0x8F),
        0x017A => Some(0x9F),
        0x017B => Some(0xAF),
        0x017C => Some(0xBF),
        0x017D => Some(0x8E),
        0x017E => Some(0x9E),
        0x02C7 => Some(0xA1),
        0x02D8 => Some(0xA2),
        0x02D9 => Some(0xFF),
        0x02DB => Some(0xB2),
        0x02DD => Some(0xBD),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201A => Some(0x82),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x201E => Some(0x84),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x2022 => Some(0x95),
        0x2026 => Some(0x85),
        0x2030 => Some(0x89),
        0x2039 => Some(0x8B),
        0x203A => Some(0x9B),
        0x20AC => Some(0x80),
        0x2122 => Some(0x99),
        _ => None,
    }
}

fn encode_cp1251_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xA0),
        0x00A4 => Some(0xA4),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A9 => Some(0xA9),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00BB => Some(0xBB),
        0x0401 => Some(0xA8),
        0x0402 => Some(0x80),
        0x0403 => Some(0x81),
        0x0404 => Some(0xAA),
        0x0405 => Some(0xBD),
        0x0406 => Some(0xB2),
        0x0407 => Some(0xAF),
        0x0408 => Some(0xA3),
        0x0409 => Some(0x8A),
        0x040A => Some(0x8C),
        0x040B => Some(0x8E),
        0x040C => Some(0x8D),
        0x040E => Some(0xA1),
        0x040F => Some(0x8F),
        0x0410 => Some(0xC0),
        0x0411 => Some(0xC1),
        0x0412 => Some(0xC2),
        0x0413 => Some(0xC3),
        0x0414 => Some(0xC4),
        0x0415 => Some(0xC5),
        0x0416 => Some(0xC6),
        0x0417 => Some(0xC7),
        0x0418 => Some(0xC8),
        0x0419 => Some(0xC9),
        0x041A => Some(0xCA),
        0x041B => Some(0xCB),
        0x041C => Some(0xCC),
        0x041D => Some(0xCD),
        0x041E => Some(0xCE),
        0x041F => Some(0xCF),
        0x0420 => Some(0xD0),
        0x0421 => Some(0xD1),
        0x0422 => Some(0xD2),
        0x0423 => Some(0xD3),
        0x0424 => Some(0xD4),
        0x0425 => Some(0xD5),
        0x0426 => Some(0xD6),
        0x0427 => Some(0xD7),
        0x0428 => Some(0xD8),
        0x0429 => Some(0xD9),
        0x042A => Some(0xDA),
        0x042B => Some(0xDB),
        0x042C => Some(0xDC),
        0x042D => Some(0xDD),
        0x042E => Some(0xDE),
        0x042F => Some(0xDF),
        0x0430 => Some(0xE0),
        0x0431 => Some(0xE1),
        0x0432 => Some(0xE2),
        0x0433 => Some(0xE3),
        0x0434 => Some(0xE4),
        0x0435 => Some(0xE5),
        0x0436 => Some(0xE6),
        0x0437 => Some(0xE7),
        0x0438 => Some(0xE8),
        0x0439 => Some(0xE9),
        0x043A => Some(0xEA),
        0x043B => Some(0xEB),
        0x043C => Some(0xEC),
        0x043D => Some(0xED),
        0x043E => Some(0xEE),
        0x043F => Some(0xEF),
        0x0440 => Some(0xF0),
        0x0441 => Some(0xF1),
        0x0442 => Some(0xF2),
        0x0443 => Some(0xF3),
        0x0444 => Some(0xF4),
        0x0445 => Some(0xF5),
        0x0446 => Some(0xF6),
        0x0447 => Some(0xF7),
        0x0448 => Some(0xF8),
        0x0449 => Some(0xF9),
        0x044A => Some(0xFA),
        0x044B => Some(0xFB),
        0x044C => Some(0xFC),
        0x044D => Some(0xFD),
        0x044E => Some(0xFE),
        0x044F => Some(0xFF),
        0x0451 => Some(0xB8),
        0x0452 => Some(0x90),
        0x0453 => Some(0x83),
        0x0454 => Some(0xBA),
        0x0455 => Some(0xBE),
        0x0456 => Some(0xB3),
        0x0457 => Some(0xBF),
        0x0458 => Some(0xBC),
        0x0459 => Some(0x9A),
        0x045A => Some(0x9C),
        0x045B => Some(0x9E),
        0x045C => Some(0x9D),
        0x045E => Some(0xA2),
        0x045F => Some(0x9F),
        0x0490 => Some(0xA5),
        0x0491 => Some(0xB4),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201A => Some(0x82),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x201E => Some(0x84),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x2022 => Some(0x95),
        0x2026 => Some(0x85),
        0x2030 => Some(0x89),
        0x2039 => Some(0x8B),
        0x203A => Some(0x9B),
        0x20AC => Some(0x88),
        0x2116 => Some(0xB9),
        0x2122 => Some(0x99),
        _ => None,
    }
}

fn encode_cp866_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xFF),
        0x00A4 => Some(0xFD),
        0x00B0 => Some(0xF8),
        0x00B7 => Some(0xFA),
        0x0401 => Some(0xF0),
        0x0404 => Some(0xF2),
        0x0407 => Some(0xF4),
        0x040E => Some(0xF6),
        0x0410 => Some(0x80),
        0x0411 => Some(0x81),
        0x0412 => Some(0x82),
        0x0413 => Some(0x83),
        0x0414 => Some(0x84),
        0x0415 => Some(0x85),
        0x0416 => Some(0x86),
        0x0417 => Some(0x87),
        0x0418 => Some(0x88),
        0x0419 => Some(0x89),
        0x041A => Some(0x8A),
        0x041B => Some(0x8B),
        0x041C => Some(0x8C),
        0x041D => Some(0x8D),
        0x041E => Some(0x8E),
        0x041F => Some(0x8F),
        0x0420 => Some(0x90),
        0x0421 => Some(0x91),
        0x0422 => Some(0x92),
        0x0423 => Some(0x93),
        0x0424 => Some(0x94),
        0x0425 => Some(0x95),
        0x0426 => Some(0x96),
        0x0427 => Some(0x97),
        0x0428 => Some(0x98),
        0x0429 => Some(0x99),
        0x042A => Some(0x9A),
        0x042B => Some(0x9B),
        0x042C => Some(0x9C),
        0x042D => Some(0x9D),
        0x042E => Some(0x9E),
        0x042F => Some(0x9F),
        0x0430 => Some(0xA0),
        0x0431 => Some(0xA1),
        0x0432 => Some(0xA2),
        0x0433 => Some(0xA3),
        0x0434 => Some(0xA4),
        0x0435 => Some(0xA5),
        0x0436 => Some(0xA6),
        0x0437 => Some(0xA7),
        0x0438 => Some(0xA8),
        0x0439 => Some(0xA9),
        0x043A => Some(0xAA),
        0x043B => Some(0xAB),
        0x043C => Some(0xAC),
        0x043D => Some(0xAD),
        0x043E => Some(0xAE),
        0x043F => Some(0xAF),
        0x0440 => Some(0xE0),
        0x0441 => Some(0xE1),
        0x0442 => Some(0xE2),
        0x0443 => Some(0xE3),
        0x0444 => Some(0xE4),
        0x0445 => Some(0xE5),
        0x0446 => Some(0xE6),
        0x0447 => Some(0xE7),
        0x0448 => Some(0xE8),
        0x0449 => Some(0xE9),
        0x044A => Some(0xEA),
        0x044B => Some(0xEB),
        0x044C => Some(0xEC),
        0x044D => Some(0xED),
        0x044E => Some(0xEE),
        0x044F => Some(0xEF),
        0x0451 => Some(0xF1),
        0x0454 => Some(0xF3),
        0x0457 => Some(0xF5),
        0x045E => Some(0xF7),
        0x2116 => Some(0xFC),
        0x2219 => Some(0xF9),
        0x221A => Some(0xFB),
        0x2500 => Some(0xC4),
        0x2502 => Some(0xB3),
        0x250C => Some(0xDA),
        0x2510 => Some(0xBF),
        0x2514 => Some(0xC0),
        0x2518 => Some(0xD9),
        0x251C => Some(0xC3),
        0x2524 => Some(0xB4),
        0x252C => Some(0xC2),
        0x2534 => Some(0xC1),
        0x253C => Some(0xC5),
        0x2550 => Some(0xCD),
        0x2551 => Some(0xBA),
        0x2552 => Some(0xD5),
        0x2553 => Some(0xD6),
        0x2554 => Some(0xC9),
        0x2555 => Some(0xB8),
        0x2556 => Some(0xB7),
        0x2557 => Some(0xBB),
        0x2558 => Some(0xD4),
        0x2559 => Some(0xD3),
        0x255A => Some(0xC8),
        0x255B => Some(0xBE),
        0x255C => Some(0xBD),
        0x255D => Some(0xBC),
        0x255E => Some(0xC6),
        0x255F => Some(0xC7),
        0x2560 => Some(0xCC),
        0x2561 => Some(0xB5),
        0x2562 => Some(0xB6),
        0x2563 => Some(0xB9),
        0x2564 => Some(0xD1),
        0x2565 => Some(0xD2),
        0x2566 => Some(0xCB),
        0x2567 => Some(0xCF),
        0x2568 => Some(0xD0),
        0x2569 => Some(0xCA),
        0x256A => Some(0xD8),
        0x256B => Some(0xD7),
        0x256C => Some(0xCE),
        0x2580 => Some(0xDF),
        0x2584 => Some(0xDC),
        0x2588 => Some(0xDB),
        0x258C => Some(0xDD),
        0x2590 => Some(0xDE),
        0x2591 => Some(0xB0),
        0x2592 => Some(0xB1),
        0x2593 => Some(0xB2),
        0x25A0 => Some(0xFE),
        _ => None,
    }
}

fn encode_cp860_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xFF),
        0x00A1 => Some(0xAD),
        0x00A2 => Some(0x9B),
        0x00A3 => Some(0x9C),
        0x00AA => Some(0xA6),
        0x00AB => Some(0xAE),
        0x00AC => Some(0xAA),
        0x00B0 => Some(0xF8),
        0x00B1 => Some(0xF1),
        0x00B2 => Some(0xFD),
        0x00B5 => Some(0xE6),
        0x00B7 => Some(0xFA),
        0x00BA => Some(0xA7),
        0x00BB => Some(0xAF),
        0x00BC => Some(0xAC),
        0x00BD => Some(0xAB),
        0x00BF => Some(0xA8),
        0x00C0 => Some(0x91),
        0x00C1 => Some(0x86),
        0x00C2 => Some(0x8F),
        0x00C3 => Some(0x8E),
        0x00C7 => Some(0x80),
        0x00C8 => Some(0x92),
        0x00C9 => Some(0x90),
        0x00CA => Some(0x89),
        0x00CC => Some(0x98),
        0x00CD => Some(0x8B),
        0x00D1 => Some(0xA5),
        0x00D2 => Some(0xA9),
        0x00D3 => Some(0x9F),
        0x00D4 => Some(0x8C),
        0x00D5 => Some(0x99),
        0x00D9 => Some(0x9D),
        0x00DA => Some(0x96),
        0x00DC => Some(0x9A),
        0x00DF => Some(0xE1),
        0x00E0 => Some(0x85),
        0x00E1 => Some(0xA0),
        0x00E2 => Some(0x83),
        0x00E3 => Some(0x84),
        0x00E7 => Some(0x87),
        0x00E8 => Some(0x8A),
        0x00E9 => Some(0x82),
        0x00EA => Some(0x88),
        0x00EC => Some(0x8D),
        0x00ED => Some(0xA1),
        0x00F1 => Some(0xA4),
        0x00F2 => Some(0x95),
        0x00F3 => Some(0xA2),
        0x00F4 => Some(0x93),
        0x00F5 => Some(0x94),
        0x00F7 => Some(0xF6),
        0x00F9 => Some(0x97),
        0x00FA => Some(0xA3),
        0x00FC => Some(0x81),
        0x0393 => Some(0xE2),
        0x0398 => Some(0xE9),
        0x03A3 => Some(0xE4),
        0x03A6 => Some(0xE8),
        0x03A9 => Some(0xEA),
        0x03B1 => Some(0xE0),
        0x03B4 => Some(0xEB),
        0x03B5 => Some(0xEE),
        0x03C0 => Some(0xE3),
        0x03C3 => Some(0xE5),
        0x03C4 => Some(0xE7),
        0x03C6 => Some(0xED),
        0x207F => Some(0xFC),
        0x20A7 => Some(0x9E),
        0x2219 => Some(0xF9),
        0x221A => Some(0xFB),
        0x221E => Some(0xEC),
        0x2229 => Some(0xEF),
        0x2248 => Some(0xF7),
        0x2261 => Some(0xF0),
        0x2264 => Some(0xF3),
        0x2265 => Some(0xF2),
        0x2320 => Some(0xF4),
        0x2321 => Some(0xF5),
        0x2500 => Some(0xC4),
        0x2502 => Some(0xB3),
        0x250C => Some(0xDA),
        0x2510 => Some(0xBF),
        0x2514 => Some(0xC0),
        0x2518 => Some(0xD9),
        0x251C => Some(0xC3),
        0x2524 => Some(0xB4),
        0x252C => Some(0xC2),
        0x2534 => Some(0xC1),
        0x253C => Some(0xC5),
        0x2550 => Some(0xCD),
        0x2551 => Some(0xBA),
        0x2552 => Some(0xD5),
        0x2553 => Some(0xD6),
        0x2554 => Some(0xC9),
        0x2555 => Some(0xB8),
        0x2556 => Some(0xB7),
        0x2557 => Some(0xBB),
        0x2558 => Some(0xD4),
        0x2559 => Some(0xD3),
        0x255A => Some(0xC8),
        0x255B => Some(0xBE),
        0x255C => Some(0xBD),
        0x255D => Some(0xBC),
        0x255E => Some(0xC6),
        0x255F => Some(0xC7),
        0x2560 => Some(0xCC),
        0x2561 => Some(0xB5),
        0x2562 => Some(0xB6),
        0x2563 => Some(0xB9),
        0x2564 => Some(0xD1),
        0x2565 => Some(0xD2),
        0x2566 => Some(0xCB),
        0x2567 => Some(0xCF),
        0x2568 => Some(0xD0),
        0x2569 => Some(0xCA),
        0x256A => Some(0xD8),
        0x256B => Some(0xD7),
        0x256C => Some(0xCE),
        0x2580 => Some(0xDF),
        0x2584 => Some(0xDC),
        0x2588 => Some(0xDB),
        0x258C => Some(0xDD),
        0x2590 => Some(0xDE),
        0x2591 => Some(0xB0),
        0x2592 => Some(0xB1),
        0x2593 => Some(0xB2),
        0x25A0 => Some(0xFE),
        _ => None,
    }
}
fn encode_cp862_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xFF),
        0x00A1 => Some(0xAD),
        0x00A2 => Some(0x9B),
        0x00A3 => Some(0x9C),
        0x00A5 => Some(0x9D),
        0x00AA => Some(0xA6),
        0x00AB => Some(0xAE),
        0x00AC => Some(0xAA),
        0x00B0 => Some(0xF8),
        0x00B1 => Some(0xF1),
        0x00B2 => Some(0xFD),
        0x00B5 => Some(0xE6),
        0x00B7 => Some(0xFA),
        0x00BA => Some(0xA7),
        0x00BB => Some(0xAF),
        0x00BC => Some(0xAC),
        0x00BD => Some(0xAB),
        0x00BF => Some(0xA8),
        0x00D1 => Some(0xA5),
        0x00DF => Some(0xE1),
        0x00E1 => Some(0xA0),
        0x00ED => Some(0xA1),
        0x00F1 => Some(0xA4),
        0x00F3 => Some(0xA2),
        0x00F7 => Some(0xF6),
        0x00FA => Some(0xA3),
        0x0192 => Some(0x9F),
        0x0393 => Some(0xE2),
        0x0398 => Some(0xE9),
        0x03A3 => Some(0xE4),
        0x03A6 => Some(0xE8),
        0x03A9 => Some(0xEA),
        0x03B1 => Some(0xE0),
        0x03B4 => Some(0xEB),
        0x03B5 => Some(0xEE),
        0x03C0 => Some(0xE3),
        0x03C3 => Some(0xE5),
        0x03C4 => Some(0xE7),
        0x03C6 => Some(0xED),
        0x05D0 => Some(0x80),
        0x05D1 => Some(0x81),
        0x05D2 => Some(0x82),
        0x05D3 => Some(0x83),
        0x05D4 => Some(0x84),
        0x05D5 => Some(0x85),
        0x05D6 => Some(0x86),
        0x05D7 => Some(0x87),
        0x05D8 => Some(0x88),
        0x05D9 => Some(0x89),
        0x05DA => Some(0x8A),
        0x05DB => Some(0x8B),
        0x05DC => Some(0x8C),
        0x05DD => Some(0x8D),
        0x05DE => Some(0x8E),
        0x05DF => Some(0x8F),
        0x05E0 => Some(0x90),
        0x05E1 => Some(0x91),
        0x05E2 => Some(0x92),
        0x05E3 => Some(0x93),
        0x05E4 => Some(0x94),
        0x05E5 => Some(0x95),
        0x05E6 => Some(0x96),
        0x05E7 => Some(0x97),
        0x05E8 => Some(0x98),
        0x05E9 => Some(0x99),
        0x05EA => Some(0x9A),
        0x207F => Some(0xFC),
        0x20A7 => Some(0x9E),
        0x2219 => Some(0xF9),
        0x221A => Some(0xFB),
        0x221E => Some(0xEC),
        0x2229 => Some(0xEF),
        0x2248 => Some(0xF7),
        0x2261 => Some(0xF0),
        0x2264 => Some(0xF3),
        0x2265 => Some(0xF2),
        0x2310 => Some(0xA9),
        0x2320 => Some(0xF4),
        0x2321 => Some(0xF5),
        0x2500 => Some(0xC4),
        0x2502 => Some(0xB3),
        0x250C => Some(0xDA),
        0x2510 => Some(0xBF),
        0x2514 => Some(0xC0),
        0x2518 => Some(0xD9),
        0x251C => Some(0xC3),
        0x2524 => Some(0xB4),
        0x252C => Some(0xC2),
        0x2534 => Some(0xC1),
        0x253C => Some(0xC5),
        0x2550 => Some(0xCD),
        0x2551 => Some(0xBA),
        0x2552 => Some(0xD5),
        0x2553 => Some(0xD6),
        0x2554 => Some(0xC9),
        0x2555 => Some(0xB8),
        0x2556 => Some(0xB7),
        0x2557 => Some(0xBB),
        0x2558 => Some(0xD4),
        0x2559 => Some(0xD3),
        0x255A => Some(0xC8),
        0x255B => Some(0xBE),
        0x255C => Some(0xBD),
        0x255D => Some(0xBC),
        0x255E => Some(0xC6),
        0x255F => Some(0xC7),
        0x2560 => Some(0xCC),
        0x2561 => Some(0xB5),
        0x2562 => Some(0xB6),
        0x2563 => Some(0xB9),
        0x2564 => Some(0xD1),
        0x2565 => Some(0xD2),
        0x2566 => Some(0xCB),
        0x2567 => Some(0xCF),
        0x2568 => Some(0xD0),
        0x2569 => Some(0xCA),
        0x256A => Some(0xD8),
        0x256B => Some(0xD7),
        0x256C => Some(0xCE),
        0x2580 => Some(0xDF),
        0x2584 => Some(0xDC),
        0x2588 => Some(0xDB),
        0x258C => Some(0xDD),
        0x2590 => Some(0xDE),
        0x2591 => Some(0xB0),
        0x2592 => Some(0xB1),
        0x2593 => Some(0xB2),
        0x25A0 => Some(0xFE),
        _ => None,
    }
}
fn encode_cp863_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xFF),
        0x00A2 => Some(0x9B),
        0x00A3 => Some(0x9C),
        0x00A4 => Some(0x98),
        0x00A6 => Some(0xA0),
        0x00A7 => Some(0x8F),
        0x00A8 => Some(0xA4),
        0x00AB => Some(0xAE),
        0x00AC => Some(0xAA),
        0x00AF => Some(0xA7),
        0x00B0 => Some(0xF8),
        0x00B1 => Some(0xF1),
        0x00B2 => Some(0xFD),
        0x00B3 => Some(0xA6),
        0x00B4 => Some(0xA1),
        0x00B5 => Some(0xE6),
        0x00B6 => Some(0x86),
        0x00B7 => Some(0xFA),
        0x00B8 => Some(0xA5),
        0x00BB => Some(0xAF),
        0x00BC => Some(0xAC),
        0x00BD => Some(0xAB),
        0x00BE => Some(0xAD),
        0x00C0 => Some(0x8E),
        0x00C2 => Some(0x84),
        0x00C7 => Some(0x80),
        0x00C8 => Some(0x91),
        0x00C9 => Some(0x90),
        0x00CA => Some(0x92),
        0x00CB => Some(0x94),
        0x00CE => Some(0xA8),
        0x00CF => Some(0x95),
        0x00D4 => Some(0x99),
        0x00D9 => Some(0x9D),
        0x00DB => Some(0x9E),
        0x00DC => Some(0x9A),
        0x00DF => Some(0xE1),
        0x00E0 => Some(0x85),
        0x00E2 => Some(0x83),
        0x00E7 => Some(0x87),
        0x00E8 => Some(0x8A),
        0x00E9 => Some(0x82),
        0x00EA => Some(0x88),
        0x00EB => Some(0x89),
        0x00EE => Some(0x8C),
        0x00EF => Some(0x8B),
        0x00F3 => Some(0xA2),
        0x00F4 => Some(0x93),
        0x00F7 => Some(0xF6),
        0x00F9 => Some(0x97),
        0x00FA => Some(0xA3),
        0x00FB => Some(0x96),
        0x00FC => Some(0x81),
        0x0192 => Some(0x9F),
        0x0393 => Some(0xE2),
        0x0398 => Some(0xE9),
        0x03A3 => Some(0xE4),
        0x03A6 => Some(0xE8),
        0x03A9 => Some(0xEA),
        0x03B1 => Some(0xE0),
        0x03B4 => Some(0xEB),
        0x03B5 => Some(0xEE),
        0x03C0 => Some(0xE3),
        0x03C3 => Some(0xE5),
        0x03C4 => Some(0xE7),
        0x03C6 => Some(0xED),
        0x2017 => Some(0x8D),
        0x207F => Some(0xFC),
        0x2219 => Some(0xF9),
        0x221A => Some(0xFB),
        0x221E => Some(0xEC),
        0x2229 => Some(0xEF),
        0x2248 => Some(0xF7),
        0x2261 => Some(0xF0),
        0x2264 => Some(0xF3),
        0x2265 => Some(0xF2),
        0x2310 => Some(0xA9),
        0x2320 => Some(0xF4),
        0x2321 => Some(0xF5),
        0x2500 => Some(0xC4),
        0x2502 => Some(0xB3),
        0x250C => Some(0xDA),
        0x2510 => Some(0xBF),
        0x2514 => Some(0xC0),
        0x2518 => Some(0xD9),
        0x251C => Some(0xC3),
        0x2524 => Some(0xB4),
        0x252C => Some(0xC2),
        0x2534 => Some(0xC1),
        0x253C => Some(0xC5),
        0x2550 => Some(0xCD),
        0x2551 => Some(0xBA),
        0x2552 => Some(0xD5),
        0x2553 => Some(0xD6),
        0x2554 => Some(0xC9),
        0x2555 => Some(0xB8),
        0x2556 => Some(0xB7),
        0x2557 => Some(0xBB),
        0x2558 => Some(0xD4),
        0x2559 => Some(0xD3),
        0x255A => Some(0xC8),
        0x255B => Some(0xBE),
        0x255C => Some(0xBD),
        0x255D => Some(0xBC),
        0x255E => Some(0xC6),
        0x255F => Some(0xC7),
        0x2560 => Some(0xCC),
        0x2561 => Some(0xB5),
        0x2562 => Some(0xB6),
        0x2563 => Some(0xB9),
        0x2564 => Some(0xD1),
        0x2565 => Some(0xD2),
        0x2566 => Some(0xCB),
        0x2567 => Some(0xCF),
        0x2568 => Some(0xD0),
        0x2569 => Some(0xCA),
        0x256A => Some(0xD8),
        0x256B => Some(0xD7),
        0x256C => Some(0xCE),
        0x2580 => Some(0xDF),
        0x2584 => Some(0xDC),
        0x2588 => Some(0xDB),
        0x258C => Some(0xDD),
        0x2590 => Some(0xDE),
        0x2591 => Some(0xB0),
        0x2592 => Some(0xB1),
        0x2593 => Some(0xB2),
        0x25A0 => Some(0xFE),
        _ => None,
    }
}
fn encode_cp1253_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xA0),
        0x00A3 => Some(0xA3),
        0x00A4 => Some(0xA4),
        0x00A5 => Some(0xA5),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00A9 => Some(0xA9),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00BB => Some(0xBB),
        0x00BD => Some(0xBD),
        0x0192 => Some(0x83),
        0x0384 => Some(0xB4),
        0x0385 => Some(0xA1),
        0x0386 => Some(0xA2),
        0x0388 => Some(0xB8),
        0x0389 => Some(0xB9),
        0x038A => Some(0xBA),
        0x038C => Some(0xBC),
        0x038E => Some(0xBE),
        0x038F => Some(0xBF),
        0x0390 => Some(0xC0),
        0x0391 => Some(0xC1),
        0x0392 => Some(0xC2),
        0x0393 => Some(0xC3),
        0x0394 => Some(0xC4),
        0x0395 => Some(0xC5),
        0x0396 => Some(0xC6),
        0x0397 => Some(0xC7),
        0x0398 => Some(0xC8),
        0x0399 => Some(0xC9),
        0x039A => Some(0xCA),
        0x039B => Some(0xCB),
        0x039C => Some(0xCC),
        0x039D => Some(0xCD),
        0x039E => Some(0xCE),
        0x039F => Some(0xCF),
        0x03A0 => Some(0xD0),
        0x03A1 => Some(0xD1),
        0x03A3 => Some(0xD3),
        0x03A4 => Some(0xD4),
        0x03A5 => Some(0xD5),
        0x03A6 => Some(0xD6),
        0x03A7 => Some(0xD7),
        0x03A8 => Some(0xD8),
        0x03A9 => Some(0xD9),
        0x03AA => Some(0xDA),
        0x03AB => Some(0xDB),
        0x03AC => Some(0xDC),
        0x03AD => Some(0xDD),
        0x03AE => Some(0xDE),
        0x03AF => Some(0xDF),
        0x03B0 => Some(0xE0),
        0x03B1 => Some(0xE1),
        0x03B2 => Some(0xE2),
        0x03B3 => Some(0xE3),
        0x03B4 => Some(0xE4),
        0x03B5 => Some(0xE5),
        0x03B6 => Some(0xE6),
        0x03B7 => Some(0xE7),
        0x03B8 => Some(0xE8),
        0x03B9 => Some(0xE9),
        0x03BA => Some(0xEA),
        0x03BB => Some(0xEB),
        0x03BC => Some(0xEC),
        0x03BD => Some(0xED),
        0x03BE => Some(0xEE),
        0x03BF => Some(0xEF),
        0x03C0 => Some(0xF0),
        0x03C1 => Some(0xF1),
        0x03C2 => Some(0xF2),
        0x03C3 => Some(0xF3),
        0x03C4 => Some(0xF4),
        0x03C5 => Some(0xF5),
        0x03C6 => Some(0xF6),
        0x03C7 => Some(0xF7),
        0x03C8 => Some(0xF8),
        0x03C9 => Some(0xF9),
        0x03CA => Some(0xFA),
        0x03CB => Some(0xFB),
        0x03CC => Some(0xFC),
        0x03CD => Some(0xFD),
        0x03CE => Some(0xFE),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2015 => Some(0xAF),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201A => Some(0x82),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x201E => Some(0x84),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x2022 => Some(0x95),
        0x2026 => Some(0x85),
        0x2030 => Some(0x89),
        0x2039 => Some(0x8B),
        0x203A => Some(0x9B),
        0x20AC => Some(0x80),
        0x2122 => Some(0x99),
        _ => None,
    }
}
fn encode_cp1254_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xA0),
        0x00A1 => Some(0xA1),
        0x00A2 => Some(0xA2),
        0x00A3 => Some(0xA3),
        0x00A4 => Some(0xA4),
        0x00A5 => Some(0xA5),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00A9 => Some(0xA9),
        0x00AA => Some(0xAA),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00AF => Some(0xAF),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B4 => Some(0xB4),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00B8 => Some(0xB8),
        0x00B9 => Some(0xB9),
        0x00BA => Some(0xBA),
        0x00BB => Some(0xBB),
        0x00BC => Some(0xBC),
        0x00BD => Some(0xBD),
        0x00BE => Some(0xBE),
        0x00BF => Some(0xBF),
        0x00C0 => Some(0xC0),
        0x00C1 => Some(0xC1),
        0x00C2 => Some(0xC2),
        0x00C3 => Some(0xC3),
        0x00C4 => Some(0xC4),
        0x00C5 => Some(0xC5),
        0x00C6 => Some(0xC6),
        0x00C7 => Some(0xC7),
        0x00C8 => Some(0xC8),
        0x00C9 => Some(0xC9),
        0x00CA => Some(0xCA),
        0x00CB => Some(0xCB),
        0x00CC => Some(0xCC),
        0x00CD => Some(0xCD),
        0x00CE => Some(0xCE),
        0x00CF => Some(0xCF),
        0x00D1 => Some(0xD1),
        0x00D2 => Some(0xD2),
        0x00D3 => Some(0xD3),
        0x00D4 => Some(0xD4),
        0x00D5 => Some(0xD5),
        0x00D6 => Some(0xD6),
        0x00D7 => Some(0xD7),
        0x00D8 => Some(0xD8),
        0x00D9 => Some(0xD9),
        0x00DA => Some(0xDA),
        0x00DB => Some(0xDB),
        0x00DC => Some(0xDC),
        0x00DF => Some(0xDF),
        0x00E0 => Some(0xE0),
        0x00E1 => Some(0xE1),
        0x00E2 => Some(0xE2),
        0x00E3 => Some(0xE3),
        0x00E4 => Some(0xE4),
        0x00E5 => Some(0xE5),
        0x00E6 => Some(0xE6),
        0x00E7 => Some(0xE7),
        0x00E8 => Some(0xE8),
        0x00E9 => Some(0xE9),
        0x00EA => Some(0xEA),
        0x00EB => Some(0xEB),
        0x00EC => Some(0xEC),
        0x00ED => Some(0xED),
        0x00EE => Some(0xEE),
        0x00EF => Some(0xEF),
        0x00F1 => Some(0xF1),
        0x00F2 => Some(0xF2),
        0x00F3 => Some(0xF3),
        0x00F4 => Some(0xF4),
        0x00F5 => Some(0xF5),
        0x00F6 => Some(0xF6),
        0x00F7 => Some(0xF7),
        0x00F8 => Some(0xF8),
        0x00F9 => Some(0xF9),
        0x00FA => Some(0xFA),
        0x00FB => Some(0xFB),
        0x00FC => Some(0xFC),
        0x00FF => Some(0xFF),
        0x011E => Some(0xD0),
        0x011F => Some(0xF0),
        0x0130 => Some(0xDD),
        0x0131 => Some(0xFD),
        0x0152 => Some(0x8C),
        0x0153 => Some(0x9C),
        0x015E => Some(0xDE),
        0x015F => Some(0xFE),
        0x0160 => Some(0x8A),
        0x0161 => Some(0x9A),
        0x0178 => Some(0x9F),
        0x0192 => Some(0x83),
        0x02C6 => Some(0x88),
        0x02DC => Some(0x98),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201A => Some(0x82),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x201E => Some(0x84),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x2022 => Some(0x95),
        0x2026 => Some(0x85),
        0x2030 => Some(0x89),
        0x2039 => Some(0x8B),
        0x203A => Some(0x9B),
        0x20AC => Some(0x80),
        0x2122 => Some(0x99),
        _ => None,
    }
}
fn encode_cp1255_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xA0),
        0x00A1 => Some(0xA1),
        0x00A2 => Some(0xA2),
        0x00A3 => Some(0xA3),
        0x00A5 => Some(0xA5),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00A9 => Some(0xA9),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00AF => Some(0xAF),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B4 => Some(0xB4),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00B8 => Some(0xB8),
        0x00B9 => Some(0xB9),
        0x00BB => Some(0xBB),
        0x00BC => Some(0xBC),
        0x00BD => Some(0xBD),
        0x00BE => Some(0xBE),
        0x00BF => Some(0xBF),
        0x00D7 => Some(0xAA),
        0x00F7 => Some(0xBA),
        0x0192 => Some(0x83),
        0x02C6 => Some(0x88),
        0x02DC => Some(0x98),
        0x05B0 => Some(0xC0),
        0x05B1 => Some(0xC1),
        0x05B2 => Some(0xC2),
        0x05B3 => Some(0xC3),
        0x05B4 => Some(0xC4),
        0x05B5 => Some(0xC5),
        0x05B6 => Some(0xC6),
        0x05B7 => Some(0xC7),
        0x05B8 => Some(0xC8),
        0x05B9 => Some(0xC9),
        0x05BB => Some(0xCB),
        0x05BC => Some(0xCC),
        0x05BD => Some(0xCD),
        0x05BE => Some(0xCE),
        0x05BF => Some(0xCF),
        0x05C0 => Some(0xD0),
        0x05C1 => Some(0xD1),
        0x05C2 => Some(0xD2),
        0x05C3 => Some(0xD3),
        0x05D0 => Some(0xE0),
        0x05D1 => Some(0xE1),
        0x05D2 => Some(0xE2),
        0x05D3 => Some(0xE3),
        0x05D4 => Some(0xE4),
        0x05D5 => Some(0xE5),
        0x05D6 => Some(0xE6),
        0x05D7 => Some(0xE7),
        0x05D8 => Some(0xE8),
        0x05D9 => Some(0xE9),
        0x05DA => Some(0xEA),
        0x05DB => Some(0xEB),
        0x05DC => Some(0xEC),
        0x05DD => Some(0xED),
        0x05DE => Some(0xEE),
        0x05DF => Some(0xEF),
        0x05E0 => Some(0xF0),
        0x05E1 => Some(0xF1),
        0x05E2 => Some(0xF2),
        0x05E3 => Some(0xF3),
        0x05E4 => Some(0xF4),
        0x05E5 => Some(0xF5),
        0x05E6 => Some(0xF6),
        0x05E7 => Some(0xF7),
        0x05E8 => Some(0xF8),
        0x05E9 => Some(0xF9),
        0x05EA => Some(0xFA),
        0x05F0 => Some(0xD4),
        0x05F1 => Some(0xD5),
        0x05F2 => Some(0xD6),
        0x05F3 => Some(0xD7),
        0x05F4 => Some(0xD8),
        0x200E => Some(0xFD),
        0x200F => Some(0xFE),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201A => Some(0x82),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x201E => Some(0x84),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x2022 => Some(0x95),
        0x2026 => Some(0x85),
        0x2030 => Some(0x89),
        0x2039 => Some(0x8B),
        0x203A => Some(0x9B),
        0x20AA => Some(0xA4),
        0x20AC => Some(0x80),
        0x2122 => Some(0x99),
        _ => None,
    }
}
fn encode_cp1256_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xA0),
        0x00A2 => Some(0xA2),
        0x00A3 => Some(0xA3),
        0x00A4 => Some(0xA4),
        0x00A5 => Some(0xA5),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00A9 => Some(0xA9),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00AF => Some(0xAF),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B4 => Some(0xB4),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00B8 => Some(0xB8),
        0x00B9 => Some(0xB9),
        0x00BB => Some(0xBB),
        0x00BC => Some(0xBC),
        0x00BD => Some(0xBD),
        0x00BE => Some(0xBE),
        0x00D7 => Some(0xD7),
        0x00E0 => Some(0xE0),
        0x00E2 => Some(0xE2),
        0x00E7 => Some(0xE7),
        0x00E8 => Some(0xE8),
        0x00E9 => Some(0xE9),
        0x00EA => Some(0xEA),
        0x00EB => Some(0xEB),
        0x00EE => Some(0xEE),
        0x00EF => Some(0xEF),
        0x00F4 => Some(0xF4),
        0x00F7 => Some(0xF7),
        0x00F9 => Some(0xF9),
        0x00FB => Some(0xFB),
        0x00FC => Some(0xFC),
        0x0152 => Some(0x8C),
        0x0153 => Some(0x9C),
        0x0192 => Some(0x83),
        0x02C6 => Some(0x88),
        0x060C => Some(0xA1),
        0x061B => Some(0xBA),
        0x061F => Some(0xBF),
        0x0621 => Some(0xC1),
        0x0622 => Some(0xC2),
        0x0623 => Some(0xC3),
        0x0624 => Some(0xC4),
        0x0625 => Some(0xC5),
        0x0626 => Some(0xC6),
        0x0627 => Some(0xC7),
        0x0628 => Some(0xC8),
        0x0629 => Some(0xC9),
        0x062A => Some(0xCA),
        0x062B => Some(0xCB),
        0x062C => Some(0xCC),
        0x062D => Some(0xCD),
        0x062E => Some(0xCE),
        0x062F => Some(0xCF),
        0x0630 => Some(0xD0),
        0x0631 => Some(0xD1),
        0x0632 => Some(0xD2),
        0x0633 => Some(0xD3),
        0x0634 => Some(0xD4),
        0x0635 => Some(0xD5),
        0x0636 => Some(0xD6),
        0x0637 => Some(0xD8),
        0x0638 => Some(0xD9),
        0x0639 => Some(0xDA),
        0x063A => Some(0xDB),
        0x0640 => Some(0xDC),
        0x0641 => Some(0xDD),
        0x0642 => Some(0xDE),
        0x0643 => Some(0xDF),
        0x0644 => Some(0xE1),
        0x0645 => Some(0xE3),
        0x0646 => Some(0xE4),
        0x0647 => Some(0xE5),
        0x0648 => Some(0xE6),
        0x0649 => Some(0xEC),
        0x064A => Some(0xED),
        0x064B => Some(0xF0),
        0x064C => Some(0xF1),
        0x064D => Some(0xF2),
        0x064E => Some(0xF3),
        0x064F => Some(0xF5),
        0x0650 => Some(0xF6),
        0x0651 => Some(0xF8),
        0x0652 => Some(0xFA),
        0x0679 => Some(0x8A),
        0x067E => Some(0x81),
        0x0686 => Some(0x8D),
        0x0688 => Some(0x8F),
        0x0691 => Some(0x9A),
        0x0698 => Some(0x8E),
        0x06A9 => Some(0x98),
        0x06AF => Some(0x90),
        0x06BA => Some(0x9F),
        0x06BE => Some(0xAA),
        0x06C1 => Some(0xC0),
        0x06D2 => Some(0xFF),
        0x200C => Some(0x9D),
        0x200D => Some(0x9E),
        0x200E => Some(0xFD),
        0x200F => Some(0xFE),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201A => Some(0x82),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x201E => Some(0x84),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x2022 => Some(0x95),
        0x2026 => Some(0x85),
        0x2030 => Some(0x89),
        0x2039 => Some(0x8B),
        0x203A => Some(0x9B),
        0x20AC => Some(0x80),
        0x2122 => Some(0x99),
        _ => None,
    }
}
fn encode_cp1257_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xA0),
        0x00A2 => Some(0xA2),
        0x00A3 => Some(0xA3),
        0x00A4 => Some(0xA4),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0x8D),
        0x00A9 => Some(0xA9),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00AF => Some(0x9D),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B4 => Some(0xB4),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00B8 => Some(0x8F),
        0x00B9 => Some(0xB9),
        0x00BB => Some(0xBB),
        0x00BC => Some(0xBC),
        0x00BD => Some(0xBD),
        0x00BE => Some(0xBE),
        0x00C4 => Some(0xC4),
        0x00C5 => Some(0xC5),
        0x00C6 => Some(0xAF),
        0x00C9 => Some(0xC9),
        0x00D3 => Some(0xD3),
        0x00D5 => Some(0xD5),
        0x00D6 => Some(0xD6),
        0x00D7 => Some(0xD7),
        0x00D8 => Some(0xA8),
        0x00DC => Some(0xDC),
        0x00DF => Some(0xDF),
        0x00E4 => Some(0xE4),
        0x00E5 => Some(0xE5),
        0x00E6 => Some(0xBF),
        0x00E9 => Some(0xE9),
        0x00F3 => Some(0xF3),
        0x00F5 => Some(0xF5),
        0x00F6 => Some(0xF6),
        0x00F7 => Some(0xF7),
        0x00F8 => Some(0xB8),
        0x00FC => Some(0xFC),
        0x0100 => Some(0xC2),
        0x0101 => Some(0xE2),
        0x0104 => Some(0xC0),
        0x0105 => Some(0xE0),
        0x0106 => Some(0xC3),
        0x0107 => Some(0xE3),
        0x010C => Some(0xC8),
        0x010D => Some(0xE8),
        0x0112 => Some(0xC7),
        0x0113 => Some(0xE7),
        0x0116 => Some(0xCB),
        0x0117 => Some(0xEB),
        0x0118 => Some(0xC6),
        0x0119 => Some(0xE6),
        0x0122 => Some(0xCC),
        0x0123 => Some(0xEC),
        0x012A => Some(0xCE),
        0x012B => Some(0xEE),
        0x012E => Some(0xC1),
        0x012F => Some(0xE1),
        0x0136 => Some(0xCD),
        0x0137 => Some(0xED),
        0x013B => Some(0xCF),
        0x013C => Some(0xEF),
        0x0141 => Some(0xD9),
        0x0142 => Some(0xF9),
        0x0143 => Some(0xD1),
        0x0144 => Some(0xF1),
        0x0145 => Some(0xD2),
        0x0146 => Some(0xF2),
        0x014C => Some(0xD4),
        0x014D => Some(0xF4),
        0x0156 => Some(0xAA),
        0x0157 => Some(0xBA),
        0x015A => Some(0xDA),
        0x015B => Some(0xFA),
        0x0160 => Some(0xD0),
        0x0161 => Some(0xF0),
        0x016A => Some(0xDB),
        0x016B => Some(0xFB),
        0x0172 => Some(0xD8),
        0x0173 => Some(0xF8),
        0x0179 => Some(0xCA),
        0x017A => Some(0xEA),
        0x017B => Some(0xDD),
        0x017C => Some(0xFD),
        0x017D => Some(0xDE),
        0x017E => Some(0xFE),
        0x02C7 => Some(0x8E),
        0x02D9 => Some(0xFF),
        0x02DB => Some(0x9E),
        0x2013 => Some(0x96),
        0x2014 => Some(0x97),
        0x2018 => Some(0x91),
        0x2019 => Some(0x92),
        0x201A => Some(0x82),
        0x201C => Some(0x93),
        0x201D => Some(0x94),
        0x201E => Some(0x84),
        0x2020 => Some(0x86),
        0x2021 => Some(0x87),
        0x2022 => Some(0x95),
        0x2026 => Some(0x85),
        0x2030 => Some(0x89),
        0x2039 => Some(0x8B),
        0x203A => Some(0x9B),
        0x20AC => Some(0x80),
        0x2122 => Some(0x99),
        _ => None,
    }
}
fn encode_koi8_r_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0x9A),
        0x00A9 => Some(0xBF),
        0x00B0 => Some(0x9C),
        0x00B2 => Some(0x9D),
        0x00B7 => Some(0x9E),
        0x00F7 => Some(0x9F),
        0x0401 => Some(0xB3),
        0x0410 => Some(0xE1),
        0x0411 => Some(0xE2),
        0x0412 => Some(0xF7),
        0x0413 => Some(0xE7),
        0x0414 => Some(0xE4),
        0x0415 => Some(0xE5),
        0x0416 => Some(0xF6),
        0x0417 => Some(0xFA),
        0x0418 => Some(0xE9),
        0x0419 => Some(0xEA),
        0x041A => Some(0xEB),
        0x041B => Some(0xEC),
        0x041C => Some(0xED),
        0x041D => Some(0xEE),
        0x041E => Some(0xEF),
        0x041F => Some(0xF0),
        0x0420 => Some(0xF2),
        0x0421 => Some(0xF3),
        0x0422 => Some(0xF4),
        0x0423 => Some(0xF5),
        0x0424 => Some(0xE6),
        0x0425 => Some(0xE8),
        0x0426 => Some(0xE3),
        0x0427 => Some(0xFE),
        0x0428 => Some(0xFB),
        0x0429 => Some(0xFD),
        0x042A => Some(0xFF),
        0x042B => Some(0xF9),
        0x042C => Some(0xF8),
        0x042D => Some(0xFC),
        0x042E => Some(0xE0),
        0x042F => Some(0xF1),
        0x0430 => Some(0xC1),
        0x0431 => Some(0xC2),
        0x0432 => Some(0xD7),
        0x0433 => Some(0xC7),
        0x0434 => Some(0xC4),
        0x0435 => Some(0xC5),
        0x0436 => Some(0xD6),
        0x0437 => Some(0xDA),
        0x0438 => Some(0xC9),
        0x0439 => Some(0xCA),
        0x043A => Some(0xCB),
        0x043B => Some(0xCC),
        0x043C => Some(0xCD),
        0x043D => Some(0xCE),
        0x043E => Some(0xCF),
        0x043F => Some(0xD0),
        0x0440 => Some(0xD2),
        0x0441 => Some(0xD3),
        0x0442 => Some(0xD4),
        0x0443 => Some(0xD5),
        0x0444 => Some(0xC6),
        0x0445 => Some(0xC8),
        0x0446 => Some(0xC3),
        0x0447 => Some(0xDE),
        0x0448 => Some(0xDB),
        0x0449 => Some(0xDD),
        0x044A => Some(0xDF),
        0x044B => Some(0xD9),
        0x044C => Some(0xD8),
        0x044D => Some(0xDC),
        0x044E => Some(0xC0),
        0x044F => Some(0xD1),
        0x0451 => Some(0xA3),
        0x2219 => Some(0x95),
        0x221A => Some(0x96),
        0x2248 => Some(0x97),
        0x2264 => Some(0x98),
        0x2265 => Some(0x99),
        0x2320 => Some(0x93),
        0x2321 => Some(0x9B),
        0x2500 => Some(0x80),
        0x2502 => Some(0x81),
        0x250C => Some(0x82),
        0x2510 => Some(0x83),
        0x2514 => Some(0x84),
        0x2518 => Some(0x85),
        0x251C => Some(0x86),
        0x2524 => Some(0x87),
        0x252C => Some(0x88),
        0x2534 => Some(0x89),
        0x253C => Some(0x8A),
        0x2550 => Some(0xA0),
        0x2551 => Some(0xA1),
        0x2552 => Some(0xA2),
        0x2553 => Some(0xA4),
        0x2554 => Some(0xA5),
        0x2555 => Some(0xA6),
        0x2556 => Some(0xA7),
        0x2557 => Some(0xA8),
        0x2558 => Some(0xA9),
        0x2559 => Some(0xAA),
        0x255A => Some(0xAB),
        0x255B => Some(0xAC),
        0x255C => Some(0xAD),
        0x255D => Some(0xAE),
        0x255E => Some(0xAF),
        0x255F => Some(0xB0),
        0x2560 => Some(0xB1),
        0x2561 => Some(0xB2),
        0x2562 => Some(0xB4),
        0x2563 => Some(0xB5),
        0x2564 => Some(0xB6),
        0x2565 => Some(0xB7),
        0x2566 => Some(0xB8),
        0x2567 => Some(0xB9),
        0x2568 => Some(0xBA),
        0x2569 => Some(0xBB),
        0x256A => Some(0xBC),
        0x256B => Some(0xBD),
        0x256C => Some(0xBE),
        0x2580 => Some(0x8B),
        0x2584 => Some(0x8C),
        0x2588 => Some(0x8D),
        0x258C => Some(0x8E),
        0x2590 => Some(0x8F),
        0x2591 => Some(0x90),
        0x2592 => Some(0x91),
        0x2593 => Some(0x92),
        0x25A0 => Some(0x94),
        _ => None,
    }
}

fn encode_iso8859_2_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A4 => Some(0xA4),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00AD => Some(0xAD),
        0x00B0 => Some(0xB0),
        0x00B4 => Some(0xB4),
        0x00B8 => Some(0xB8),
        0x00C1 => Some(0xC1),
        0x00C2 => Some(0xC2),
        0x00C4 => Some(0xC4),
        0x00C7 => Some(0xC7),
        0x00C9 => Some(0xC9),
        0x00CB => Some(0xCB),
        0x00CD => Some(0xCD),
        0x00CE => Some(0xCE),
        0x00D3 => Some(0xD3),
        0x00D4 => Some(0xD4),
        0x00D6 => Some(0xD6),
        0x00D7 => Some(0xD7),
        0x00DA => Some(0xDA),
        0x00DC => Some(0xDC),
        0x00DD => Some(0xDD),
        0x00DF => Some(0xDF),
        0x00E1 => Some(0xE1),
        0x00E2 => Some(0xE2),
        0x00E4 => Some(0xE4),
        0x00E7 => Some(0xE7),
        0x00E9 => Some(0xE9),
        0x00EB => Some(0xEB),
        0x00ED => Some(0xED),
        0x00EE => Some(0xEE),
        0x00F3 => Some(0xF3),
        0x00F4 => Some(0xF4),
        0x00F6 => Some(0xF6),
        0x00F7 => Some(0xF7),
        0x00FA => Some(0xFA),
        0x00FC => Some(0xFC),
        0x00FD => Some(0xFD),
        0x0102 => Some(0xC3),
        0x0103 => Some(0xE3),
        0x0104 => Some(0xA1),
        0x0105 => Some(0xB1),
        0x0106 => Some(0xC6),
        0x0107 => Some(0xE6),
        0x010C => Some(0xC8),
        0x010D => Some(0xE8),
        0x010E => Some(0xCF),
        0x010F => Some(0xEF),
        0x0110 => Some(0xD0),
        0x0111 => Some(0xF0),
        0x0118 => Some(0xCA),
        0x0119 => Some(0xEA),
        0x011A => Some(0xCC),
        0x011B => Some(0xEC),
        0x0139 => Some(0xC5),
        0x013A => Some(0xE5),
        0x013D => Some(0xA5),
        0x013E => Some(0xB5),
        0x0141 => Some(0xA3),
        0x0142 => Some(0xB3),
        0x0143 => Some(0xD1),
        0x0144 => Some(0xF1),
        0x0147 => Some(0xD2),
        0x0148 => Some(0xF2),
        0x0150 => Some(0xD5),
        0x0151 => Some(0xF5),
        0x0154 => Some(0xC0),
        0x0155 => Some(0xE0),
        0x0158 => Some(0xD8),
        0x0159 => Some(0xF8),
        0x015A => Some(0xA6),
        0x015B => Some(0xB6),
        0x015E => Some(0xAA),
        0x015F => Some(0xBA),
        0x0160 => Some(0xA9),
        0x0161 => Some(0xB9),
        0x0162 => Some(0xDE),
        0x0163 => Some(0xFE),
        0x0164 => Some(0xAB),
        0x0165 => Some(0xBB),
        0x016E => Some(0xD9),
        0x016F => Some(0xF9),
        0x0170 => Some(0xDB),
        0x0171 => Some(0xFB),
        0x0179 => Some(0xAC),
        0x017A => Some(0xBC),
        0x017B => Some(0xAF),
        0x017C => Some(0xBF),
        0x017D => Some(0xAE),
        0x017E => Some(0xBE),
        0x02C7 => Some(0xB7),
        0x02D8 => Some(0xA2),
        0x02D9 => Some(0xFF),
        0x02DB => Some(0xB2),
        0x02DD => Some(0xBD),
        _ => None,
    }
}

fn encode_iso8859_3_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A3 => Some(0xA3),
        0x00A4 => Some(0xA4),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00AD => Some(0xAD),
        0x00B0 => Some(0xB0),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B4 => Some(0xB4),
        0x00B5 => Some(0xB5),
        0x00B7 => Some(0xB7),
        0x00B8 => Some(0xB8),
        0x00BD => Some(0xBD),
        0x00C0 => Some(0xC0),
        0x00C1 => Some(0xC1),
        0x00C2 => Some(0xC2),
        0x00C4 => Some(0xC4),
        0x00C7 => Some(0xC7),
        0x00C8 => Some(0xC8),
        0x00C9 => Some(0xC9),
        0x00CA => Some(0xCA),
        0x00CB => Some(0xCB),
        0x00CC => Some(0xCC),
        0x00CD => Some(0xCD),
        0x00CE => Some(0xCE),
        0x00CF => Some(0xCF),
        0x00D1 => Some(0xD1),
        0x00D2 => Some(0xD2),
        0x00D3 => Some(0xD3),
        0x00D4 => Some(0xD4),
        0x00D6 => Some(0xD6),
        0x00D7 => Some(0xD7),
        0x00D9 => Some(0xD9),
        0x00DA => Some(0xDA),
        0x00DB => Some(0xDB),
        0x00DC => Some(0xDC),
        0x00DF => Some(0xDF),
        0x00E0 => Some(0xE0),
        0x00E1 => Some(0xE1),
        0x00E2 => Some(0xE2),
        0x00E4 => Some(0xE4),
        0x00E7 => Some(0xE7),
        0x00E8 => Some(0xE8),
        0x00E9 => Some(0xE9),
        0x00EA => Some(0xEA),
        0x00EB => Some(0xEB),
        0x00EC => Some(0xEC),
        0x00ED => Some(0xED),
        0x00EE => Some(0xEE),
        0x00EF => Some(0xEF),
        0x00F1 => Some(0xF1),
        0x00F2 => Some(0xF2),
        0x00F3 => Some(0xF3),
        0x00F4 => Some(0xF4),
        0x00F6 => Some(0xF6),
        0x00F7 => Some(0xF7),
        0x00F9 => Some(0xF9),
        0x00FA => Some(0xFA),
        0x00FB => Some(0xFB),
        0x00FC => Some(0xFC),
        0x0108 => Some(0xC6),
        0x0109 => Some(0xE6),
        0x010A => Some(0xC5),
        0x010B => Some(0xE5),
        0x011C => Some(0xD8),
        0x011D => Some(0xF8),
        0x011E => Some(0xAB),
        0x011F => Some(0xBB),
        0x0120 => Some(0xD5),
        0x0121 => Some(0xF5),
        0x0124 => Some(0xA6),
        0x0125 => Some(0xB6),
        0x0126 => Some(0xA1),
        0x0127 => Some(0xB1),
        0x0130 => Some(0xA9),
        0x0131 => Some(0xB9),
        0x0134 => Some(0xAC),
        0x0135 => Some(0xBC),
        0x015C => Some(0xDE),
        0x015D => Some(0xFE),
        0x015E => Some(0xAA),
        0x015F => Some(0xBA),
        0x016C => Some(0xDD),
        0x016D => Some(0xFD),
        0x017B => Some(0xAF),
        0x017C => Some(0xBF),
        0x02D8 => Some(0xA2),
        0x02D9 => Some(0xFF),
        _ => None,
    }
}
fn encode_iso8859_4_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A4 => Some(0xA4),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00AD => Some(0xAD),
        0x00AF => Some(0xAF),
        0x00B0 => Some(0xB0),
        0x00B4 => Some(0xB4),
        0x00B8 => Some(0xB8),
        0x00C1 => Some(0xC1),
        0x00C2 => Some(0xC2),
        0x00C3 => Some(0xC3),
        0x00C4 => Some(0xC4),
        0x00C5 => Some(0xC5),
        0x00C6 => Some(0xC6),
        0x00C9 => Some(0xC9),
        0x00CB => Some(0xCB),
        0x00CD => Some(0xCD),
        0x00CE => Some(0xCE),
        0x00D4 => Some(0xD4),
        0x00D5 => Some(0xD5),
        0x00D6 => Some(0xD6),
        0x00D7 => Some(0xD7),
        0x00D8 => Some(0xD8),
        0x00DA => Some(0xDA),
        0x00DB => Some(0xDB),
        0x00DC => Some(0xDC),
        0x00DF => Some(0xDF),
        0x00E1 => Some(0xE1),
        0x00E2 => Some(0xE2),
        0x00E3 => Some(0xE3),
        0x00E4 => Some(0xE4),
        0x00E5 => Some(0xE5),
        0x00E6 => Some(0xE6),
        0x00E9 => Some(0xE9),
        0x00EB => Some(0xEB),
        0x00ED => Some(0xED),
        0x00EE => Some(0xEE),
        0x00F4 => Some(0xF4),
        0x00F5 => Some(0xF5),
        0x00F6 => Some(0xF6),
        0x00F7 => Some(0xF7),
        0x00F8 => Some(0xF8),
        0x00FA => Some(0xFA),
        0x00FB => Some(0xFB),
        0x00FC => Some(0xFC),
        0x0100 => Some(0xC0),
        0x0101 => Some(0xE0),
        0x0104 => Some(0xA1),
        0x0105 => Some(0xB1),
        0x010C => Some(0xC8),
        0x010D => Some(0xE8),
        0x0110 => Some(0xD0),
        0x0111 => Some(0xF0),
        0x0112 => Some(0xAA),
        0x0113 => Some(0xBA),
        0x0116 => Some(0xCC),
        0x0117 => Some(0xEC),
        0x0118 => Some(0xCA),
        0x0119 => Some(0xEA),
        0x0122 => Some(0xAB),
        0x0123 => Some(0xBB),
        0x0128 => Some(0xA5),
        0x0129 => Some(0xB5),
        0x012A => Some(0xCF),
        0x012B => Some(0xEF),
        0x012E => Some(0xC7),
        0x012F => Some(0xE7),
        0x0136 => Some(0xD3),
        0x0137 => Some(0xF3),
        0x0138 => Some(0xA2),
        0x013B => Some(0xA6),
        0x013C => Some(0xB6),
        0x0145 => Some(0xD1),
        0x0146 => Some(0xF1),
        0x014A => Some(0xBD),
        0x014B => Some(0xBF),
        0x014C => Some(0xD2),
        0x014D => Some(0xF2),
        0x0156 => Some(0xA3),
        0x0157 => Some(0xB3),
        0x0160 => Some(0xA9),
        0x0161 => Some(0xB9),
        0x0166 => Some(0xAC),
        0x0167 => Some(0xBC),
        0x0168 => Some(0xDD),
        0x0169 => Some(0xFD),
        0x016A => Some(0xDE),
        0x016B => Some(0xFE),
        0x0172 => Some(0xD9),
        0x0173 => Some(0xF9),
        0x017D => Some(0xAE),
        0x017E => Some(0xBE),
        0x02C7 => Some(0xB7),
        0x02D9 => Some(0xFF),
        0x02DB => Some(0xB2),
        _ => None,
    }
}
fn encode_iso8859_5_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A7 => Some(0xFD),
        0x00AD => Some(0xAD),
        0x0401 => Some(0xA1),
        0x0402 => Some(0xA2),
        0x0403 => Some(0xA3),
        0x0404 => Some(0xA4),
        0x0405 => Some(0xA5),
        0x0406 => Some(0xA6),
        0x0407 => Some(0xA7),
        0x0408 => Some(0xA8),
        0x0409 => Some(0xA9),
        0x040A => Some(0xAA),
        0x040B => Some(0xAB),
        0x040C => Some(0xAC),
        0x040E => Some(0xAE),
        0x040F => Some(0xAF),
        0x0410 => Some(0xB0),
        0x0411 => Some(0xB1),
        0x0412 => Some(0xB2),
        0x0413 => Some(0xB3),
        0x0414 => Some(0xB4),
        0x0415 => Some(0xB5),
        0x0416 => Some(0xB6),
        0x0417 => Some(0xB7),
        0x0418 => Some(0xB8),
        0x0419 => Some(0xB9),
        0x041A => Some(0xBA),
        0x041B => Some(0xBB),
        0x041C => Some(0xBC),
        0x041D => Some(0xBD),
        0x041E => Some(0xBE),
        0x041F => Some(0xBF),
        0x0420 => Some(0xC0),
        0x0421 => Some(0xC1),
        0x0422 => Some(0xC2),
        0x0423 => Some(0xC3),
        0x0424 => Some(0xC4),
        0x0425 => Some(0xC5),
        0x0426 => Some(0xC6),
        0x0427 => Some(0xC7),
        0x0428 => Some(0xC8),
        0x0429 => Some(0xC9),
        0x042A => Some(0xCA),
        0x042B => Some(0xCB),
        0x042C => Some(0xCC),
        0x042D => Some(0xCD),
        0x042E => Some(0xCE),
        0x042F => Some(0xCF),
        0x0430 => Some(0xD0),
        0x0431 => Some(0xD1),
        0x0432 => Some(0xD2),
        0x0433 => Some(0xD3),
        0x0434 => Some(0xD4),
        0x0435 => Some(0xD5),
        0x0436 => Some(0xD6),
        0x0437 => Some(0xD7),
        0x0438 => Some(0xD8),
        0x0439 => Some(0xD9),
        0x043A => Some(0xDA),
        0x043B => Some(0xDB),
        0x043C => Some(0xDC),
        0x043D => Some(0xDD),
        0x043E => Some(0xDE),
        0x043F => Some(0xDF),
        0x0440 => Some(0xE0),
        0x0441 => Some(0xE1),
        0x0442 => Some(0xE2),
        0x0443 => Some(0xE3),
        0x0444 => Some(0xE4),
        0x0445 => Some(0xE5),
        0x0446 => Some(0xE6),
        0x0447 => Some(0xE7),
        0x0448 => Some(0xE8),
        0x0449 => Some(0xE9),
        0x044A => Some(0xEA),
        0x044B => Some(0xEB),
        0x044C => Some(0xEC),
        0x044D => Some(0xED),
        0x044E => Some(0xEE),
        0x044F => Some(0xEF),
        0x0451 => Some(0xF1),
        0x0452 => Some(0xF2),
        0x0453 => Some(0xF3),
        0x0454 => Some(0xF4),
        0x0455 => Some(0xF5),
        0x0456 => Some(0xF6),
        0x0457 => Some(0xF7),
        0x0458 => Some(0xF8),
        0x0459 => Some(0xF9),
        0x045A => Some(0xFA),
        0x045B => Some(0xFB),
        0x045C => Some(0xFC),
        0x045E => Some(0xFE),
        0x045F => Some(0xFF),
        0x2116 => Some(0xF0),
        _ => None,
    }
}

fn encode_iso8859_6_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A4 => Some(0xA4),
        0x00AD => Some(0xAD),
        0x060C => Some(0xAC),
        0x061B => Some(0xBB),
        0x061F => Some(0xBF),
        0x0621 => Some(0xC1),
        0x0622 => Some(0xC2),
        0x0623 => Some(0xC3),
        0x0624 => Some(0xC4),
        0x0625 => Some(0xC5),
        0x0626 => Some(0xC6),
        0x0627 => Some(0xC7),
        0x0628 => Some(0xC8),
        0x0629 => Some(0xC9),
        0x062A => Some(0xCA),
        0x062B => Some(0xCB),
        0x062C => Some(0xCC),
        0x062D => Some(0xCD),
        0x062E => Some(0xCE),
        0x062F => Some(0xCF),
        0x0630 => Some(0xD0),
        0x0631 => Some(0xD1),
        0x0632 => Some(0xD2),
        0x0633 => Some(0xD3),
        0x0634 => Some(0xD4),
        0x0635 => Some(0xD5),
        0x0636 => Some(0xD6),
        0x0637 => Some(0xD7),
        0x0638 => Some(0xD8),
        0x0639 => Some(0xD9),
        0x063A => Some(0xDA),
        0x0640 => Some(0xE0),
        0x0641 => Some(0xE1),
        0x0642 => Some(0xE2),
        0x0643 => Some(0xE3),
        0x0644 => Some(0xE4),
        0x0645 => Some(0xE5),
        0x0646 => Some(0xE6),
        0x0647 => Some(0xE7),
        0x0648 => Some(0xE8),
        0x0649 => Some(0xE9),
        0x064A => Some(0xEA),
        0x064B => Some(0xEB),
        0x064C => Some(0xEC),
        0x064D => Some(0xED),
        0x064E => Some(0xEE),
        0x064F => Some(0xEF),
        0x0650 => Some(0xF0),
        0x0651 => Some(0xF1),
        0x0652 => Some(0xF2),
        _ => None,
    }
}
fn encode_iso8859_7_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A3 => Some(0xA3),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00A9 => Some(0xA9),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B7 => Some(0xB7),
        0x00BB => Some(0xBB),
        0x00BD => Some(0xBD),
        0x037A => Some(0xAA),
        0x0384 => Some(0xB4),
        0x0385 => Some(0xB5),
        0x0386 => Some(0xB6),
        0x0388 => Some(0xB8),
        0x0389 => Some(0xB9),
        0x038A => Some(0xBA),
        0x038C => Some(0xBC),
        0x038E => Some(0xBE),
        0x038F => Some(0xBF),
        0x0390 => Some(0xC0),
        0x0391 => Some(0xC1),
        0x0392 => Some(0xC2),
        0x0393 => Some(0xC3),
        0x0394 => Some(0xC4),
        0x0395 => Some(0xC5),
        0x0396 => Some(0xC6),
        0x0397 => Some(0xC7),
        0x0398 => Some(0xC8),
        0x0399 => Some(0xC9),
        0x039A => Some(0xCA),
        0x039B => Some(0xCB),
        0x039C => Some(0xCC),
        0x039D => Some(0xCD),
        0x039E => Some(0xCE),
        0x039F => Some(0xCF),
        0x03A0 => Some(0xD0),
        0x03A1 => Some(0xD1),
        0x03A3 => Some(0xD3),
        0x03A4 => Some(0xD4),
        0x03A5 => Some(0xD5),
        0x03A6 => Some(0xD6),
        0x03A7 => Some(0xD7),
        0x03A8 => Some(0xD8),
        0x03A9 => Some(0xD9),
        0x03AA => Some(0xDA),
        0x03AB => Some(0xDB),
        0x03AC => Some(0xDC),
        0x03AD => Some(0xDD),
        0x03AE => Some(0xDE),
        0x03AF => Some(0xDF),
        0x03B0 => Some(0xE0),
        0x03B1 => Some(0xE1),
        0x03B2 => Some(0xE2),
        0x03B3 => Some(0xE3),
        0x03B4 => Some(0xE4),
        0x03B5 => Some(0xE5),
        0x03B6 => Some(0xE6),
        0x03B7 => Some(0xE7),
        0x03B8 => Some(0xE8),
        0x03B9 => Some(0xE9),
        0x03BA => Some(0xEA),
        0x03BB => Some(0xEB),
        0x03BC => Some(0xEC),
        0x03BD => Some(0xED),
        0x03BE => Some(0xEE),
        0x03BF => Some(0xEF),
        0x03C0 => Some(0xF0),
        0x03C1 => Some(0xF1),
        0x03C2 => Some(0xF2),
        0x03C3 => Some(0xF3),
        0x03C4 => Some(0xF4),
        0x03C5 => Some(0xF5),
        0x03C6 => Some(0xF6),
        0x03C7 => Some(0xF7),
        0x03C8 => Some(0xF8),
        0x03C9 => Some(0xF9),
        0x03CA => Some(0xFA),
        0x03CB => Some(0xFB),
        0x03CC => Some(0xFC),
        0x03CD => Some(0xFD),
        0x03CE => Some(0xFE),
        0x2015 => Some(0xAF),
        0x2018 => Some(0xA1),
        0x2019 => Some(0xA2),
        0x20AC => Some(0xA4),
        0x20AF => Some(0xA5),
        _ => None,
    }
}

fn encode_koi8_u_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0x9A),
        0x00A9 => Some(0xBF),
        0x00B0 => Some(0x9C),
        0x00B2 => Some(0x9D),
        0x00B7 => Some(0x9E),
        0x00F7 => Some(0x9F),
        0x0401 => Some(0xB3),
        0x0404 => Some(0xB4),
        0x0406 => Some(0xB6),
        0x0407 => Some(0xB7),
        0x0410 => Some(0xE1),
        0x0411 => Some(0xE2),
        0x0412 => Some(0xF7),
        0x0413 => Some(0xE7),
        0x0414 => Some(0xE4),
        0x0415 => Some(0xE5),
        0x0416 => Some(0xF6),
        0x0417 => Some(0xFA),
        0x0418 => Some(0xE9),
        0x0419 => Some(0xEA),
        0x041A => Some(0xEB),
        0x041B => Some(0xEC),
        0x041C => Some(0xED),
        0x041D => Some(0xEE),
        0x041E => Some(0xEF),
        0x041F => Some(0xF0),
        0x0420 => Some(0xF2),
        0x0421 => Some(0xF3),
        0x0422 => Some(0xF4),
        0x0423 => Some(0xF5),
        0x0424 => Some(0xE6),
        0x0425 => Some(0xE8),
        0x0426 => Some(0xE3),
        0x0427 => Some(0xFE),
        0x0428 => Some(0xFB),
        0x0429 => Some(0xFD),
        0x042A => Some(0xFF),
        0x042B => Some(0xF9),
        0x042C => Some(0xF8),
        0x042D => Some(0xFC),
        0x042E => Some(0xE0),
        0x042F => Some(0xF1),
        0x0430 => Some(0xC1),
        0x0431 => Some(0xC2),
        0x0432 => Some(0xD7),
        0x0433 => Some(0xC7),
        0x0434 => Some(0xC4),
        0x0435 => Some(0xC5),
        0x0436 => Some(0xD6),
        0x0437 => Some(0xDA),
        0x0438 => Some(0xC9),
        0x0439 => Some(0xCA),
        0x043A => Some(0xCB),
        0x043B => Some(0xCC),
        0x043C => Some(0xCD),
        0x043D => Some(0xCE),
        0x043E => Some(0xCF),
        0x043F => Some(0xD0),
        0x0440 => Some(0xD2),
        0x0441 => Some(0xD3),
        0x0442 => Some(0xD4),
        0x0443 => Some(0xD5),
        0x0444 => Some(0xC6),
        0x0445 => Some(0xC8),
        0x0446 => Some(0xC3),
        0x0447 => Some(0xDE),
        0x0448 => Some(0xDB),
        0x0449 => Some(0xDD),
        0x044A => Some(0xDF),
        0x044B => Some(0xD9),
        0x044C => Some(0xD8),
        0x044D => Some(0xDC),
        0x044E => Some(0xC0),
        0x044F => Some(0xD1),
        0x0451 => Some(0xA3),
        0x0454 => Some(0xA4),
        0x0456 => Some(0xA6),
        0x0457 => Some(0xA7),
        0x0490 => Some(0xBD),
        0x0491 => Some(0xAD),
        0x2219 => Some(0x95),
        0x221A => Some(0x96),
        0x2248 => Some(0x97),
        0x2264 => Some(0x98),
        0x2265 => Some(0x99),
        0x2320 => Some(0x93),
        0x2321 => Some(0x9B),
        0x2500 => Some(0x80),
        0x2502 => Some(0x81),
        0x250C => Some(0x82),
        0x2510 => Some(0x83),
        0x2514 => Some(0x84),
        0x2518 => Some(0x85),
        0x251C => Some(0x86),
        0x2524 => Some(0x87),
        0x252C => Some(0x88),
        0x2534 => Some(0x89),
        0x253C => Some(0x8A),
        0x2550 => Some(0xA0),
        0x2551 => Some(0xA1),
        0x2552 => Some(0xA2),
        0x2554 => Some(0xA5),
        0x2557 => Some(0xA8),
        0x2558 => Some(0xA9),
        0x2559 => Some(0xAA),
        0x255A => Some(0xAB),
        0x255B => Some(0xAC),
        0x255D => Some(0xAE),
        0x255E => Some(0xAF),
        0x255F => Some(0xB0),
        0x2560 => Some(0xB1),
        0x2561 => Some(0xB2),
        0x2563 => Some(0xB5),
        0x2566 => Some(0xB8),
        0x2567 => Some(0xB9),
        0x2568 => Some(0xBA),
        0x2569 => Some(0xBB),
        0x256A => Some(0xBC),
        0x256C => Some(0xBE),
        0x2580 => Some(0x8B),
        0x2584 => Some(0x8C),
        0x2588 => Some(0x8D),
        0x258C => Some(0x8E),
        0x2590 => Some(0x8F),
        0x2591 => Some(0x90),
        0x2592 => Some(0x91),
        0x2593 => Some(0x92),
        0x25A0 => Some(0x94),
        _ => None,
    }
}

fn encode_iso8859_8_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A2 => Some(0xA2),
        0x00A3 => Some(0xA3),
        0x00A4 => Some(0xA4),
        0x00A5 => Some(0xA5),
        0x00A6 => Some(0xA6),
        0x00A7 => Some(0xA7),
        0x00A8 => Some(0xA8),
        0x00A9 => Some(0xA9),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00AF => Some(0xAF),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B4 => Some(0xB4),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00B8 => Some(0xB8),
        0x00B9 => Some(0xB9),
        0x00BB => Some(0xBB),
        0x00BC => Some(0xBC),
        0x00BD => Some(0xBD),
        0x00BE => Some(0xBE),
        0x00D7 => Some(0xAA),
        0x00F7 => Some(0xBA),
        0x05D0 => Some(0xE0),
        0x05D1 => Some(0xE1),
        0x05D2 => Some(0xE2),
        0x05D3 => Some(0xE3),
        0x05D4 => Some(0xE4),
        0x05D5 => Some(0xE5),
        0x05D6 => Some(0xE6),
        0x05D7 => Some(0xE7),
        0x05D8 => Some(0xE8),
        0x05D9 => Some(0xE9),
        0x05DA => Some(0xEA),
        0x05DB => Some(0xEB),
        0x05DC => Some(0xEC),
        0x05DD => Some(0xED),
        0x05DE => Some(0xEE),
        0x05DF => Some(0xEF),
        0x05E0 => Some(0xF0),
        0x05E1 => Some(0xF1),
        0x05E2 => Some(0xF2),
        0x05E3 => Some(0xF3),
        0x05E4 => Some(0xF4),
        0x05E5 => Some(0xF5),
        0x05E6 => Some(0xF6),
        0x05E7 => Some(0xF7),
        0x05E8 => Some(0xF8),
        0x05E9 => Some(0xF9),
        0x05EA => Some(0xFA),
        0x200E => Some(0xFD),
        0x200F => Some(0xFE),
        0x2017 => Some(0xDF),
        _ => None,
    }
}
fn encode_iso8859_10_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A7 => Some(0xA7),
        0x00AD => Some(0xAD),
        0x00B0 => Some(0xB0),
        0x00B7 => Some(0xB7),
        0x00C1 => Some(0xC1),
        0x00C2 => Some(0xC2),
        0x00C3 => Some(0xC3),
        0x00C4 => Some(0xC4),
        0x00C5 => Some(0xC5),
        0x00C6 => Some(0xC6),
        0x00C9 => Some(0xC9),
        0x00CB => Some(0xCB),
        0x00CD => Some(0xCD),
        0x00CE => Some(0xCE),
        0x00CF => Some(0xCF),
        0x00D0 => Some(0xD0),
        0x00D3 => Some(0xD3),
        0x00D4 => Some(0xD4),
        0x00D5 => Some(0xD5),
        0x00D6 => Some(0xD6),
        0x00D8 => Some(0xD8),
        0x00DA => Some(0xDA),
        0x00DB => Some(0xDB),
        0x00DC => Some(0xDC),
        0x00DD => Some(0xDD),
        0x00DE => Some(0xDE),
        0x00DF => Some(0xDF),
        0x00E1 => Some(0xE1),
        0x00E2 => Some(0xE2),
        0x00E3 => Some(0xE3),
        0x00E4 => Some(0xE4),
        0x00E5 => Some(0xE5),
        0x00E6 => Some(0xE6),
        0x00E9 => Some(0xE9),
        0x00EB => Some(0xEB),
        0x00ED => Some(0xED),
        0x00EE => Some(0xEE),
        0x00EF => Some(0xEF),
        0x00F0 => Some(0xF0),
        0x00F3 => Some(0xF3),
        0x00F4 => Some(0xF4),
        0x00F5 => Some(0xF5),
        0x00F6 => Some(0xF6),
        0x00F8 => Some(0xF8),
        0x00FA => Some(0xFA),
        0x00FB => Some(0xFB),
        0x00FC => Some(0xFC),
        0x00FD => Some(0xFD),
        0x00FE => Some(0xFE),
        0x0100 => Some(0xC0),
        0x0101 => Some(0xE0),
        0x0104 => Some(0xA1),
        0x0105 => Some(0xB1),
        0x010C => Some(0xC8),
        0x010D => Some(0xE8),
        0x0110 => Some(0xA9),
        0x0111 => Some(0xB9),
        0x0112 => Some(0xA2),
        0x0113 => Some(0xB2),
        0x0116 => Some(0xCC),
        0x0117 => Some(0xEC),
        0x0118 => Some(0xCA),
        0x0119 => Some(0xEA),
        0x0122 => Some(0xA3),
        0x0123 => Some(0xB3),
        0x0128 => Some(0xA5),
        0x0129 => Some(0xB5),
        0x012A => Some(0xA4),
        0x012B => Some(0xB4),
        0x012E => Some(0xC7),
        0x012F => Some(0xE7),
        0x0136 => Some(0xA6),
        0x0137 => Some(0xB6),
        0x0138 => Some(0xFF),
        0x013B => Some(0xA8),
        0x013C => Some(0xB8),
        0x0145 => Some(0xD1),
        0x0146 => Some(0xF1),
        0x014A => Some(0xAF),
        0x014B => Some(0xBF),
        0x014C => Some(0xD2),
        0x014D => Some(0xF2),
        0x0160 => Some(0xAA),
        0x0161 => Some(0xBA),
        0x0166 => Some(0xAB),
        0x0167 => Some(0xBB),
        0x0168 => Some(0xD7),
        0x0169 => Some(0xF7),
        0x016A => Some(0xAE),
        0x016B => Some(0xBE),
        0x0172 => Some(0xD9),
        0x0173 => Some(0xF9),
        0x017D => Some(0xAC),
        0x017E => Some(0xBC),
        0x2015 => Some(0xBD),
        _ => None,
    }
}
fn encode_iso8859_15_byte(code: u32) -> Option<u8> {
    match code {
        0x0080 => Some(0x80),
        0x0081 => Some(0x81),
        0x0082 => Some(0x82),
        0x0083 => Some(0x83),
        0x0084 => Some(0x84),
        0x0085 => Some(0x85),
        0x0086 => Some(0x86),
        0x0087 => Some(0x87),
        0x0088 => Some(0x88),
        0x0089 => Some(0x89),
        0x008A => Some(0x8A),
        0x008B => Some(0x8B),
        0x008C => Some(0x8C),
        0x008D => Some(0x8D),
        0x008E => Some(0x8E),
        0x008F => Some(0x8F),
        0x0090 => Some(0x90),
        0x0091 => Some(0x91),
        0x0092 => Some(0x92),
        0x0093 => Some(0x93),
        0x0094 => Some(0x94),
        0x0095 => Some(0x95),
        0x0096 => Some(0x96),
        0x0097 => Some(0x97),
        0x0098 => Some(0x98),
        0x0099 => Some(0x99),
        0x009A => Some(0x9A),
        0x009B => Some(0x9B),
        0x009C => Some(0x9C),
        0x009D => Some(0x9D),
        0x009E => Some(0x9E),
        0x009F => Some(0x9F),
        0x00A0 => Some(0xA0),
        0x00A1 => Some(0xA1),
        0x00A2 => Some(0xA2),
        0x00A3 => Some(0xA3),
        0x00A5 => Some(0xA5),
        0x00A7 => Some(0xA7),
        0x00A9 => Some(0xA9),
        0x00AA => Some(0xAA),
        0x00AB => Some(0xAB),
        0x00AC => Some(0xAC),
        0x00AD => Some(0xAD),
        0x00AE => Some(0xAE),
        0x00AF => Some(0xAF),
        0x00B0 => Some(0xB0),
        0x00B1 => Some(0xB1),
        0x00B2 => Some(0xB2),
        0x00B3 => Some(0xB3),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xB6),
        0x00B7 => Some(0xB7),
        0x00B9 => Some(0xB9),
        0x00BA => Some(0xBA),
        0x00BB => Some(0xBB),
        0x00BF => Some(0xBF),
        0x00C0 => Some(0xC0),
        0x00C1 => Some(0xC1),
        0x00C2 => Some(0xC2),
        0x00C3 => Some(0xC3),
        0x00C4 => Some(0xC4),
        0x00C5 => Some(0xC5),
        0x00C6 => Some(0xC6),
        0x00C7 => Some(0xC7),
        0x00C8 => Some(0xC8),
        0x00C9 => Some(0xC9),
        0x00CA => Some(0xCA),
        0x00CB => Some(0xCB),
        0x00CC => Some(0xCC),
        0x00CD => Some(0xCD),
        0x00CE => Some(0xCE),
        0x00CF => Some(0xCF),
        0x00D0 => Some(0xD0),
        0x00D1 => Some(0xD1),
        0x00D2 => Some(0xD2),
        0x00D3 => Some(0xD3),
        0x00D4 => Some(0xD4),
        0x00D5 => Some(0xD5),
        0x00D6 => Some(0xD6),
        0x00D7 => Some(0xD7),
        0x00D8 => Some(0xD8),
        0x00D9 => Some(0xD9),
        0x00DA => Some(0xDA),
        0x00DB => Some(0xDB),
        0x00DC => Some(0xDC),
        0x00DD => Some(0xDD),
        0x00DE => Some(0xDE),
        0x00DF => Some(0xDF),
        0x00E0 => Some(0xE0),
        0x00E1 => Some(0xE1),
        0x00E2 => Some(0xE2),
        0x00E3 => Some(0xE3),
        0x00E4 => Some(0xE4),
        0x00E5 => Some(0xE5),
        0x00E6 => Some(0xE6),
        0x00E7 => Some(0xE7),
        0x00E8 => Some(0xE8),
        0x00E9 => Some(0xE9),
        0x00EA => Some(0xEA),
        0x00EB => Some(0xEB),
        0x00EC => Some(0xEC),
        0x00ED => Some(0xED),
        0x00EE => Some(0xEE),
        0x00EF => Some(0xEF),
        0x00F0 => Some(0xF0),
        0x00F1 => Some(0xF1),
        0x00F2 => Some(0xF2),
        0x00F3 => Some(0xF3),
        0x00F4 => Some(0xF4),
        0x00F5 => Some(0xF5),
        0x00F6 => Some(0xF6),
        0x00F7 => Some(0xF7),
        0x00F8 => Some(0xF8),
        0x00F9 => Some(0xF9),
        0x00FA => Some(0xFA),
        0x00FB => Some(0xFB),
        0x00FC => Some(0xFC),
        0x00FD => Some(0xFD),
        0x00FE => Some(0xFE),
        0x00FF => Some(0xFF),
        0x0152 => Some(0xBC),
        0x0153 => Some(0xBD),
        0x0160 => Some(0xA6),
        0x0161 => Some(0xA8),
        0x0178 => Some(0xBE),
        0x017D => Some(0xB4),
        0x017E => Some(0xB8),
        0x20AC => Some(0xA4),
        _ => None,
    }
}

fn encode_mac_roman_byte(code: u32) -> Option<u8> {
    match code {
        0x00A0 => Some(0xCA),
        0x00A1 => Some(0xC1),
        0x00A2 => Some(0xA2),
        0x00A3 => Some(0xA3),
        0x00A5 => Some(0xB4),
        0x00A7 => Some(0xA4),
        0x00A8 => Some(0xAC),
        0x00A9 => Some(0xA9),
        0x00AA => Some(0xBB),
        0x00AB => Some(0xC7),
        0x00AC => Some(0xC2),
        0x00AE => Some(0xA8),
        0x00AF => Some(0xF8),
        0x00B0 => Some(0xA1),
        0x00B1 => Some(0xB1),
        0x00B4 => Some(0xAB),
        0x00B5 => Some(0xB5),
        0x00B6 => Some(0xA6),
        0x00B7 => Some(0xE1),
        0x00B8 => Some(0xFC),
        0x00BA => Some(0xBC),
        0x00BB => Some(0xC8),
        0x00BF => Some(0xC0),
        0x00C0 => Some(0xCB),
        0x00C1 => Some(0xE7),
        0x00C2 => Some(0xE5),
        0x00C3 => Some(0xCC),
        0x00C4 => Some(0x80),
        0x00C5 => Some(0x81),
        0x00C6 => Some(0xAE),
        0x00C7 => Some(0x82),
        0x00C8 => Some(0xE9),
        0x00C9 => Some(0x83),
        0x00CA => Some(0xE6),
        0x00CB => Some(0xE8),
        0x00CC => Some(0xED),
        0x00CD => Some(0xEA),
        0x00CE => Some(0xEB),
        0x00CF => Some(0xEC),
        0x00D1 => Some(0x84),
        0x00D2 => Some(0xF1),
        0x00D3 => Some(0xEE),
        0x00D4 => Some(0xEF),
        0x00D5 => Some(0xCD),
        0x00D6 => Some(0x85),
        0x00D8 => Some(0xAF),
        0x00D9 => Some(0xF4),
        0x00DA => Some(0xF2),
        0x00DB => Some(0xF3),
        0x00DC => Some(0x86),
        0x00DF => Some(0xA7),
        0x00E0 => Some(0x88),
        0x00E1 => Some(0x87),
        0x00E2 => Some(0x89),
        0x00E3 => Some(0x8B),
        0x00E4 => Some(0x8A),
        0x00E5 => Some(0x8C),
        0x00E6 => Some(0xBE),
        0x00E7 => Some(0x8D),
        0x00E8 => Some(0x8F),
        0x00E9 => Some(0x8E),
        0x00EA => Some(0x90),
        0x00EB => Some(0x91),
        0x00EC => Some(0x93),
        0x00ED => Some(0x92),
        0x00EE => Some(0x94),
        0x00EF => Some(0x95),
        0x00F1 => Some(0x96),
        0x00F2 => Some(0x98),
        0x00F3 => Some(0x97),
        0x00F4 => Some(0x99),
        0x00F5 => Some(0x9B),
        0x00F6 => Some(0x9A),
        0x00F7 => Some(0xD6),
        0x00F8 => Some(0xBF),
        0x00F9 => Some(0x9D),
        0x00FA => Some(0x9C),
        0x00FB => Some(0x9E),
        0x00FC => Some(0x9F),
        0x00FF => Some(0xD8),
        0x0131 => Some(0xF5),
        0x0152 => Some(0xCE),
        0x0153 => Some(0xCF),
        0x0178 => Some(0xD9),
        0x0192 => Some(0xC4),
        0x02C6 => Some(0xF6),
        0x02C7 => Some(0xFF),
        0x02D8 => Some(0xF9),
        0x02D9 => Some(0xFA),
        0x02DA => Some(0xFB),
        0x02DB => Some(0xFE),
        0x02DC => Some(0xF7),
        0x02DD => Some(0xFD),
        0x03A9 => Some(0xBD),
        0x03C0 => Some(0xB9),
        0x2013 => Some(0xD0),
        0x2014 => Some(0xD1),
        0x2018 => Some(0xD4),
        0x2019 => Some(0xD5),
        0x201A => Some(0xE2),
        0x201C => Some(0xD2),
        0x201D => Some(0xD3),
        0x201E => Some(0xE3),
        0x2020 => Some(0xA0),
        0x2021 => Some(0xE0),
        0x2022 => Some(0xA5),
        0x2026 => Some(0xC9),
        0x2030 => Some(0xE4),
        0x2039 => Some(0xDC),
        0x203A => Some(0xDD),
        0x2044 => Some(0xDA),
        0x20AC => Some(0xDB),
        0x2122 => Some(0xAA),
        0x2202 => Some(0xB6),
        0x2206 => Some(0xC6),
        0x220F => Some(0xB8),
        0x2211 => Some(0xB7),
        0x221A => Some(0xC3),
        0x221E => Some(0xB0),
        0x222B => Some(0xBA),
        0x2248 => Some(0xC5),
        0x2260 => Some(0xAD),
        0x2264 => Some(0xB2),
        0x2265 => Some(0xB3),
        0x25CA => Some(0xD7),
        0xF8FF => Some(0xF0),
        0xFB01 => Some(0xDE),
        0xFB02 => Some(0xDF),
        _ => None,
    }
}

pub(crate) fn encode_string_with_errors(
    bytes: &[u8],
    encoding: &str,
    errors: Option<&str>,
) -> Result<Vec<u8>, EncodeError> {
    let Some(kind) = normalize_encoding(encoding) else {
        return Err(EncodeError::UnknownEncoding(encoding.to_string()));
    };
    let handler = errors.unwrap_or("strict");
    let mut unknown_handler: Option<String> = None;
    let handler = match handler {
        "surrogatepass" | "strict" | "surrogateescape" | "ignore" | "replace"
        | "backslashreplace" | "namereplace" | "xmlcharrefreplace" => handler,
        other => {
            unknown_handler = Some(other.to_string());
            "strict"
        }
    };
    let error_encoding = match kind {
        EncodingKind::Utf8Sig => "utf-8",
        EncodingKind::Cp1252
        | EncodingKind::Cp437
        | EncodingKind::Cp850
        | EncodingKind::Cp860
        | EncodingKind::Cp862
        | EncodingKind::Cp863
        | EncodingKind::Cp865
        | EncodingKind::Cp866
        | EncodingKind::Cp874
        | EncodingKind::Cp1250
        | EncodingKind::Cp1251
        | EncodingKind::Cp1253
        | EncodingKind::Cp1254
        | EncodingKind::Cp1255
        | EncodingKind::Cp1256
        | EncodingKind::Cp1257
        | EncodingKind::Koi8R
        | EncodingKind::Koi8U
        | EncodingKind::Iso8859_2
        | EncodingKind::Iso8859_3
        | EncodingKind::Iso8859_4
        | EncodingKind::Iso8859_5
        | EncodingKind::Iso8859_6
        | EncodingKind::Iso8859_7
        | EncodingKind::Iso8859_8
        | EncodingKind::Iso8859_10
        | EncodingKind::Iso8859_15
        | EncodingKind::MacRoman => "charmap",
        _ => kind.name(),
    };
    let invalid_char_err =
        |encoding: &'static str, code: u32, pos: usize, limit: u32| -> EncodeError {
            if let Some(name) = unknown_handler.as_ref() {
                EncodeError::UnknownErrorHandler(name.clone())
            } else {
                EncodeError::InvalidChar {
                    encoding,
                    code,
                    pos,
                    limit,
                }
            }
        };
    let encode_charmap = |map: fn(u32) -> Option<u8>| -> Result<Vec<u8>, EncodeError> {
        let mut out = Vec::new();
        for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
            let code = cp.to_u32();
            if code <= 0x7F {
                out.push(code as u8);
                continue;
            }
            if let Some(byte) = map(code) {
                out.push(byte);
                continue;
            }
            match handler {
                "ignore" => {}
                "replace" => out.push(b'?'),
                "backslashreplace" => {
                    out.extend_from_slice(unicode_escape_codepoint(code).as_bytes());
                }
                "namereplace" => {
                    out.extend_from_slice(unicode_name_escape(code).as_bytes());
                }
                "xmlcharrefreplace" => {
                    push_xmlcharref_ascii(&mut out, code);
                }
                "surrogateescape" => {
                    if (0xDC80..=0xDCFF).contains(&code) {
                        out.push((code - 0xDC00) as u8);
                    } else {
                        return Err(invalid_char_err(error_encoding, code, idx, 0));
                    }
                }
                "surrogatepass" | "strict" => {
                    return Err(invalid_char_err(error_encoding, code, idx, 0));
                }
                other => {
                    return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                }
            }
        }
        Ok(out)
    };
    let mut out = Vec::new();
    let encode_utf8 =
        |handler: &str, bytes: &[u8], out: &mut Vec<u8>| -> Result<Vec<u8>, EncodeError> {
            match handler {
                "surrogatepass" => Ok(bytes.to_vec()),
                "strict" => {
                    if !wtf8_has_surrogates(bytes) {
                        return Ok(bytes.to_vec());
                    }
                    for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                        let code = cp.to_u32();
                        if is_surrogate(code) {
                            return Err(invalid_char_err(error_encoding, code, idx, 0x110000));
                        }
                    }
                    Ok(bytes.to_vec())
                }
                "surrogateescape" => {
                    for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                        let code = cp.to_u32();
                        if (0xDC80..=0xDCFF).contains(&code) {
                            out.push((code - 0xDC00) as u8);
                        } else if is_surrogate(code) {
                            return Err(invalid_char_err(error_encoding, code, idx, 0x110000));
                        } else {
                            push_wtf8_codepoint(out, code);
                        }
                    }
                    Ok(std::mem::take(out))
                }
                "ignore" | "replace" | "backslashreplace" | "namereplace" | "xmlcharrefreplace" => {
                    for cp in wtf8_from_bytes(bytes).code_points() {
                        let code = cp.to_u32();
                        if is_surrogate(code) {
                            match handler {
                                "ignore" => {}
                                "replace" => out.push(b'?'),
                                "backslashreplace" => {
                                    out.extend_from_slice(unicode_escape_codepoint(code).as_bytes())
                                }
                                "namereplace" => {
                                    out.extend_from_slice(unicode_name_escape(code).as_bytes())
                                }
                                "xmlcharrefreplace" => {
                                    push_xmlcharref_ascii(out, code);
                                }
                                _ => {}
                            }
                            continue;
                        }
                        push_wtf8_codepoint(out, code);
                    }
                    Ok(std::mem::take(out))
                }
                other => Err(EncodeError::UnknownErrorHandler(other.to_string())),
            }
        };
    match kind {
        EncodingKind::Utf8 => encode_utf8(handler, bytes, &mut out),
        EncodingKind::Utf8Sig => {
            let encoded = encode_utf8(handler, bytes, &mut out)?;
            let mut with_bom = Vec::with_capacity(encoded.len() + 3);
            with_bom.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
            with_bom.extend_from_slice(&encoded);
            Ok(with_bom)
        }
        EncodingKind::Cp1252 => encode_charmap(encode_cp1252_byte),
        EncodingKind::Cp437 => encode_charmap(encode_cp437_byte),
        EncodingKind::Cp850 => encode_charmap(encode_cp850_byte),
        EncodingKind::Cp860 => encode_charmap(encode_cp860_byte),
        EncodingKind::Cp862 => encode_charmap(encode_cp862_byte),
        EncodingKind::Cp863 => encode_charmap(encode_cp863_byte),
        EncodingKind::Cp865 => encode_charmap(encode_cp865_byte),
        EncodingKind::Cp866 => encode_charmap(encode_cp866_byte),
        EncodingKind::Cp874 => encode_charmap(encode_cp874_byte),
        EncodingKind::Cp1250 => encode_charmap(encode_cp1250_byte),
        EncodingKind::Cp1251 => encode_charmap(encode_cp1251_byte),
        EncodingKind::Cp1253 => encode_charmap(encode_cp1253_byte),
        EncodingKind::Cp1254 => encode_charmap(encode_cp1254_byte),
        EncodingKind::Cp1255 => encode_charmap(encode_cp1255_byte),
        EncodingKind::Cp1256 => encode_charmap(encode_cp1256_byte),
        EncodingKind::Cp1257 => encode_charmap(encode_cp1257_byte),
        EncodingKind::Koi8R => encode_charmap(encode_koi8_r_byte),
        EncodingKind::Koi8U => encode_charmap(encode_koi8_u_byte),
        EncodingKind::Iso8859_2 => encode_charmap(encode_iso8859_2_byte),
        EncodingKind::Iso8859_3 => encode_charmap(encode_iso8859_3_byte),
        EncodingKind::Iso8859_4 => encode_charmap(encode_iso8859_4_byte),
        EncodingKind::Iso8859_5 => encode_charmap(encode_iso8859_5_byte),
        EncodingKind::Iso8859_6 => encode_charmap(encode_iso8859_6_byte),
        EncodingKind::Iso8859_7 => encode_charmap(encode_iso8859_7_byte),
        EncodingKind::Iso8859_8 => encode_charmap(encode_iso8859_8_byte),
        EncodingKind::Iso8859_10 => encode_charmap(encode_iso8859_10_byte),
        EncodingKind::Iso8859_15 => encode_charmap(encode_iso8859_15_byte),
        EncodingKind::MacRoman => encode_charmap(encode_mac_roman_byte),
        EncodingKind::Latin1 | EncodingKind::Ascii => {
            let limit = kind.ordinal_limit();
            for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                let code = cp.to_u32();
                if code < limit {
                    out.push(code as u8);
                    continue;
                }
                match handler {
                    "ignore" => {}
                    "replace" => out.push(b'?'),
                    "backslashreplace" => {
                        out.extend_from_slice(unicode_escape_codepoint(code).as_bytes());
                    }
                    "namereplace" => {
                        out.extend_from_slice(unicode_name_escape(code).as_bytes());
                    }
                    "xmlcharrefreplace" => {
                        push_xmlcharref_ascii(&mut out, code);
                    }
                    "surrogateescape" => {
                        if (0xDC80..=0xDCFF).contains(&code) {
                            out.push((code - 0xDC00) as u8);
                        } else {
                            return Err(invalid_char_err(error_encoding, code, idx, limit));
                        }
                    }
                    "surrogatepass" | "strict" => {
                        return Err(invalid_char_err(error_encoding, code, idx, limit));
                    }
                    other => {
                        return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                    }
                }
            }
            Ok(out)
        }
        EncodingKind::UnicodeEscape => {
            for cp in wtf8_from_bytes(bytes).code_points() {
                let code = cp.to_u32();
                match code {
                    0x5C => out.extend_from_slice(b"\\\\"),
                    0x09 => out.extend_from_slice(b"\\t"),
                    0x0A => out.extend_from_slice(b"\\n"),
                    0x0D => out.extend_from_slice(b"\\r"),
                    0x20..=0x7E => out.push(code as u8),
                    _ if code <= 0xFF => push_hex_escape(&mut out, b'x', code, 2),
                    _ if code <= 0xFFFF => push_hex_escape(&mut out, b'u', code, 4),
                    _ => push_hex_escape(&mut out, b'U', code, 8),
                }
            }
            Ok(out)
        }
        EncodingKind::Utf16 | EncodingKind::Utf16LE | EncodingKind::Utf16BE => {
            let (endian, with_bom) = match kind {
                EncodingKind::Utf16 => (native_endian(), true),
                EncodingKind::Utf16LE => (Endian::Little, false),
                EncodingKind::Utf16BE => (Endian::Big, false),
                _ => (native_endian(), false),
            };
            if with_bom {
                push_u16(&mut out, 0xFEFF, endian);
            }
            for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                let code = cp.to_u32();
                if is_surrogate(code) {
                    match handler {
                        "surrogatepass" | "surrogateescape" => {
                            push_u16(&mut out, code as u16, endian);
                            continue;
                        }
                        "ignore" => continue,
                        "replace" => {
                            push_u16(&mut out, 0xFFFD, endian);
                            continue;
                        }
                        "backslashreplace" => {
                            for ch in unicode_escape_codepoint(code).chars() {
                                push_u16(&mut out, ch as u16, endian);
                            }
                            continue;
                        }
                        "namereplace" => {
                            for ch in unicode_name_escape(code).chars() {
                                push_u16(&mut out, ch as u16, endian);
                            }
                            continue;
                        }
                        "xmlcharrefreplace" => {
                            push_xmlcharref_utf16(&mut out, code, endian);
                            continue;
                        }
                        "strict" => {
                            return Err(invalid_char_err(error_encoding, code, idx, 0x110000));
                        }
                        other => {
                            return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                        }
                    }
                }
                if code <= 0xFFFF {
                    push_u16(&mut out, code as u16, endian);
                } else {
                    let val = code - 0x10000;
                    let high = 0xD800 | ((val >> 10) as u16);
                    let low = 0xDC00 | ((val & 0x3FF) as u16);
                    push_u16(&mut out, high, endian);
                    push_u16(&mut out, low, endian);
                }
            }
            Ok(out)
        }
        EncodingKind::Utf32 | EncodingKind::Utf32LE | EncodingKind::Utf32BE => {
            let (endian, with_bom) = match kind {
                EncodingKind::Utf32 => (native_endian(), true),
                EncodingKind::Utf32LE => (Endian::Little, false),
                EncodingKind::Utf32BE => (Endian::Big, false),
                _ => (native_endian(), false),
            };
            if with_bom {
                push_u32(&mut out, 0x0000_FEFF, endian);
            }
            for (idx, cp) in wtf8_from_bytes(bytes).code_points().enumerate() {
                let code = cp.to_u32();
                if is_surrogate(code) {
                    match handler {
                        "surrogatepass" | "surrogateescape" => {
                            push_u32(&mut out, code, endian);
                            continue;
                        }
                        "ignore" => continue,
                        "replace" => {
                            push_u32(&mut out, 0xFFFD, endian);
                            continue;
                        }
                        "backslashreplace" => {
                            for ch in unicode_escape_codepoint(code).chars() {
                                push_u32(&mut out, ch as u32, endian);
                            }
                            continue;
                        }
                        "namereplace" => {
                            for ch in unicode_name_escape(code).chars() {
                                push_u32(&mut out, ch as u32, endian);
                            }
                            continue;
                        }
                        "xmlcharrefreplace" => {
                            push_xmlcharref_utf32(&mut out, code, endian);
                            continue;
                        }
                        "strict" => {
                            return Err(invalid_char_err(kind.name(), code, idx, 0x110000));
                        }
                        other => {
                            return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                        }
                    }
                }
                push_u32(&mut out, code, endian);
            }
            Ok(out)
        }
    }
}

pub(crate) fn decode_error_byte(label: &str, byte: u8, pos: usize, message: &str) -> String {
    format!("'{label}' codec can't decode byte 0x{byte:02x} in position {pos}: {message}")
}

pub(crate) fn decode_error_range(label: &str, start: usize, end: usize, message: &str) -> String {
    format!("'{label}' codec can't decode bytes in position {start}-{end}: {message}")
}

fn read_u16(bytes: &[u8], idx: usize, endian: Endian) -> u16 {
    match endian {
        Endian::Little => u16::from_le_bytes([bytes[idx], bytes[idx + 1]]),
        Endian::Big => u16::from_be_bytes([bytes[idx], bytes[idx + 1]]),
    }
}

fn read_u32(bytes: &[u8], idx: usize, endian: Endian) -> u32 {
    match endian {
        Endian::Little => {
            u32::from_le_bytes([bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]])
        }
        Endian::Big => {
            u32::from_be_bytes([bytes[idx], bytes[idx + 1], bytes[idx + 2], bytes[idx + 3]])
        }
    }
}

fn decode_ascii_with_errors(bytes: &[u8], errors: &str) -> Result<Vec<u8>, DecodeFailure> {
    let mut out = Vec::with_capacity(bytes.len());
    for (idx, &byte) in bytes.iter().enumerate() {
        if byte <= 0x7f {
            out.push(byte);
            continue;
        }
        match errors {
            "ignore" => {}
            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
            "backslashreplace" => push_backslash_bytes_vec(&mut out, &[byte]),
            "surrogateescape" => push_wtf8_codepoint(&mut out, 0xDC00 + byte as u32),
            "strict" | "surrogatepass" => {
                return Err(DecodeFailure::Byte {
                    pos: idx,
                    byte,
                    message: "ordinal not in range(128)",
                });
            }
            other => {
                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
            }
        }
    }
    Ok(out)
}

const CP1252_DECODE_TABLE: [u16; 32] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0xFFFF, 0x017D, 0xFFFF, 0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFF, 0x017E, 0x0178,
];

const CP437_DECODE_TABLE: [u16; 128] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7, 0x00EA, 0x00EB, 0x00E8, 0x00EF,
    0x00EE, 0x00EC, 0x00C4, 0x00C5, 0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00A2, 0x00A3, 0x00A5, 0x20A7, 0x0192, 0x00E1, 0x00ED, 0x00F3, 0x00FA,
    0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255D, 0x255C, 0x255B, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4, 0x03A6, 0x0398, 0x03A9, 0x03B4,
    0x221E, 0x03C6, 0x03B5, 0x2229, 0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];

const CP850_DECODE_TABLE: [u16; 128] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7, 0x00EA, 0x00EB, 0x00E8, 0x00EF,
    0x00EE, 0x00EC, 0x00C4, 0x00C5, 0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00F8, 0x00A3, 0x00D8, 0x00D7, 0x0192, 0x00E1, 0x00ED, 0x00F3, 0x00FA,
    0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x00AE, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x00C1, 0x00C2, 0x00C0, 0x00A9, 0x2563, 0x2551, 0x2557,
    0x255D, 0x00A2, 0x00A5, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x00E3, 0x00C3,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x00A4, 0x00F0, 0x00D0, 0x00CA, 0x00CB,
    0x00C8, 0x0131, 0x00CD, 0x00CE, 0x00CF, 0x2518, 0x250C, 0x2588, 0x2584, 0x00A6, 0x00CC, 0x2580,
    0x00D3, 0x00DF, 0x00D4, 0x00D2, 0x00F5, 0x00D5, 0x00B5, 0x00FE, 0x00DE, 0x00DA, 0x00DB, 0x00D9,
    0x00FD, 0x00DD, 0x00AF, 0x00B4, 0x00AD, 0x00B1, 0x2017, 0x00BE, 0x00B6, 0x00A7, 0x00F7, 0x00B8,
    0x00B0, 0x00A8, 0x00B7, 0x00B9, 0x00B3, 0x00B2, 0x25A0, 0x00A0,
];

const CP865_DECODE_TABLE: [u16; 128] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E4, 0x00E0, 0x00E5, 0x00E7, 0x00EA, 0x00EB, 0x00E8, 0x00EF,
    0x00EE, 0x00EC, 0x00C4, 0x00C5, 0x00C9, 0x00E6, 0x00C6, 0x00F4, 0x00F6, 0x00F2, 0x00FB, 0x00F9,
    0x00FF, 0x00D6, 0x00DC, 0x00F8, 0x00A3, 0x00D8, 0x20A7, 0x0192, 0x00E1, 0x00ED, 0x00F3, 0x00FA,
    0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00A4,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255D, 0x255C, 0x255B, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4, 0x03A6, 0x0398, 0x03A9, 0x03B4,
    0x221E, 0x03C6, 0x03B5, 0x2229, 0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];

const CP866_DECODE_TABLE: [u16; 128] = [
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417, 0x0418, 0x0419, 0x041A, 0x041B,
    0x041C, 0x041D, 0x041E, 0x041F, 0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427,
    0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F, 0x0430, 0x0431, 0x0432, 0x0433,
    0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255D, 0x255C, 0x255B, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447, 0x0448, 0x0449, 0x044A, 0x044B,
    0x044C, 0x044D, 0x044E, 0x044F, 0x0401, 0x0451, 0x0404, 0x0454, 0x0407, 0x0457, 0x040E, 0x045E,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x2116, 0x00A4, 0x25A0, 0x00A0,
];

const CP874_DECODE_TABLE: [u16; 128] = [
    0x20AC, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x2026, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x00A0, 0x0E01, 0x0E02, 0x0E03,
    0x0E04, 0x0E05, 0x0E06, 0x0E07, 0x0E08, 0x0E09, 0x0E0A, 0x0E0B, 0x0E0C, 0x0E0D, 0x0E0E, 0x0E0F,
    0x0E10, 0x0E11, 0x0E12, 0x0E13, 0x0E14, 0x0E15, 0x0E16, 0x0E17, 0x0E18, 0x0E19, 0x0E1A, 0x0E1B,
    0x0E1C, 0x0E1D, 0x0E1E, 0x0E1F, 0x0E20, 0x0E21, 0x0E22, 0x0E23, 0x0E24, 0x0E25, 0x0E26, 0x0E27,
    0x0E28, 0x0E29, 0x0E2A, 0x0E2B, 0x0E2C, 0x0E2D, 0x0E2E, 0x0E2F, 0x0E30, 0x0E31, 0x0E32, 0x0E33,
    0x0E34, 0x0E35, 0x0E36, 0x0E37, 0x0E38, 0x0E39, 0x0E3A, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x0E3F,
    0x0E40, 0x0E41, 0x0E42, 0x0E43, 0x0E44, 0x0E45, 0x0E46, 0x0E47, 0x0E48, 0x0E49, 0x0E4A, 0x0E4B,
    0x0E4C, 0x0E4D, 0x0E4E, 0x0E4F, 0x0E50, 0x0E51, 0x0E52, 0x0E53, 0x0E54, 0x0E55, 0x0E56, 0x0E57,
    0x0E58, 0x0E59, 0x0E5A, 0x0E5B, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
];

const CP1250_DECODE_TABLE: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0xFFFF, 0x201E, 0x2026, 0x2020, 0x2021, 0xFFFF, 0x2030, 0x0160, 0x2039,
    0x015A, 0x0164, 0x017D, 0x0179, 0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0x2122, 0x0161, 0x203A, 0x015B, 0x0165, 0x017E, 0x017A, 0x00A0, 0x02C7, 0x02D8, 0x0141,
    0x00A4, 0x0104, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x015E, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x017B,
    0x00B0, 0x00B1, 0x02DB, 0x0142, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x0105, 0x015F, 0x00BB,
    0x013D, 0x02DD, 0x013E, 0x017C, 0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E, 0x0110, 0x0143, 0x0147, 0x00D3,
    0x00D4, 0x0150, 0x00D6, 0x00D7, 0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7, 0x010D, 0x00E9, 0x0119, 0x00EB,
    0x011B, 0x00ED, 0x00EE, 0x010F, 0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

const CP1251_DECODE_TABLE: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039,
    0x040A, 0x040C, 0x040B, 0x040F, 0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F, 0x00A0, 0x040E, 0x045E, 0x0408,
    0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB,
    0x0458, 0x0405, 0x0455, 0x0457, 0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417,
    0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F, 0x0420, 0x0421, 0x0422, 0x0423,
    0x0424, 0x0425, 0x0426, 0x0427, 0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B,
    0x043C, 0x043D, 0x043E, 0x043F, 0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447,
    0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
];

const CP860_DECODE_TABLE: [u16; 128] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00E3, 0x00E0, 0x00C1, 0x00E7, 0x00EA, 0x00CA, 0x00E8, 0x00CD,
    0x00D4, 0x00EC, 0x00C3, 0x00C2, 0x00C9, 0x00C0, 0x00C8, 0x00F4, 0x00F5, 0x00F2, 0x00DA, 0x00F9,
    0x00CC, 0x00D5, 0x00DC, 0x00A2, 0x00A3, 0x00D9, 0x20A7, 0x00D3, 0x00E1, 0x00ED, 0x00F3, 0x00FA,
    0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x00D2, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255D, 0x255C, 0x255B, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4, 0x03A6, 0x0398, 0x03A9, 0x03B4,
    0x221E, 0x03C6, 0x03B5, 0x2229, 0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];
const CP862_DECODE_TABLE: [u16; 128] = [
    0x05D0, 0x05D1, 0x05D2, 0x05D3, 0x05D4, 0x05D5, 0x05D6, 0x05D7, 0x05D8, 0x05D9, 0x05DA, 0x05DB,
    0x05DC, 0x05DD, 0x05DE, 0x05DF, 0x05E0, 0x05E1, 0x05E2, 0x05E3, 0x05E4, 0x05E5, 0x05E6, 0x05E7,
    0x05E8, 0x05E9, 0x05EA, 0x00A2, 0x00A3, 0x00A5, 0x20A7, 0x0192, 0x00E1, 0x00ED, 0x00F3, 0x00FA,
    0x00F1, 0x00D1, 0x00AA, 0x00BA, 0x00BF, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00A1, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255D, 0x255C, 0x255B, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4, 0x03A6, 0x0398, 0x03A9, 0x03B4,
    0x221E, 0x03C6, 0x03B5, 0x2229, 0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];
const CP863_DECODE_TABLE: [u16; 128] = [
    0x00C7, 0x00FC, 0x00E9, 0x00E2, 0x00C2, 0x00E0, 0x00B6, 0x00E7, 0x00EA, 0x00EB, 0x00E8, 0x00EF,
    0x00EE, 0x2017, 0x00C0, 0x00A7, 0x00C9, 0x00C8, 0x00CA, 0x00F4, 0x00CB, 0x00CF, 0x00FB, 0x00F9,
    0x00A4, 0x00D4, 0x00DC, 0x00A2, 0x00A3, 0x00D9, 0x00DB, 0x0192, 0x00A6, 0x00B4, 0x00F3, 0x00FA,
    0x00A8, 0x00B8, 0x00B3, 0x00AF, 0x00CE, 0x2310, 0x00AC, 0x00BD, 0x00BC, 0x00BE, 0x00AB, 0x00BB,
    0x2591, 0x2592, 0x2593, 0x2502, 0x2524, 0x2561, 0x2562, 0x2556, 0x2555, 0x2563, 0x2551, 0x2557,
    0x255D, 0x255C, 0x255B, 0x2510, 0x2514, 0x2534, 0x252C, 0x251C, 0x2500, 0x253C, 0x255E, 0x255F,
    0x255A, 0x2554, 0x2569, 0x2566, 0x2560, 0x2550, 0x256C, 0x2567, 0x2568, 0x2564, 0x2565, 0x2559,
    0x2558, 0x2552, 0x2553, 0x256B, 0x256A, 0x2518, 0x250C, 0x2588, 0x2584, 0x258C, 0x2590, 0x2580,
    0x03B1, 0x00DF, 0x0393, 0x03C0, 0x03A3, 0x03C3, 0x00B5, 0x03C4, 0x03A6, 0x0398, 0x03A9, 0x03B4,
    0x221E, 0x03C6, 0x03B5, 0x2229, 0x2261, 0x00B1, 0x2265, 0x2264, 0x2320, 0x2321, 0x00F7, 0x2248,
    0x00B0, 0x2219, 0x00B7, 0x221A, 0x207F, 0x00B2, 0x25A0, 0x00A0,
];
const CP1253_DECODE_TABLE: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0xFFFF, 0x2030, 0xFFFF, 0x2039,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0x2122, 0xFFFF, 0x203A, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x00A0, 0x0385, 0x0386, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0xFFFF, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x2015,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x0384, 0x00B5, 0x00B6, 0x00B7, 0x0388, 0x0389, 0x038A, 0x00BB,
    0x038C, 0x00BD, 0x038E, 0x038F, 0x0390, 0x0391, 0x0392, 0x0393, 0x0394, 0x0395, 0x0396, 0x0397,
    0x0398, 0x0399, 0x039A, 0x039B, 0x039C, 0x039D, 0x039E, 0x039F, 0x03A0, 0x03A1, 0xFFFF, 0x03A3,
    0x03A4, 0x03A5, 0x03A6, 0x03A7, 0x03A8, 0x03A9, 0x03AA, 0x03AB, 0x03AC, 0x03AD, 0x03AE, 0x03AF,
    0x03B0, 0x03B1, 0x03B2, 0x03B3, 0x03B4, 0x03B5, 0x03B6, 0x03B7, 0x03B8, 0x03B9, 0x03BA, 0x03BB,
    0x03BC, 0x03BD, 0x03BE, 0x03BF, 0x03C0, 0x03C1, 0x03C2, 0x03C3, 0x03C4, 0x03C5, 0x03C6, 0x03C7,
    0x03C8, 0x03C9, 0x03CA, 0x03CB, 0x03CC, 0x03CD, 0x03CE, 0xFFFF,
];
const CP1254_DECODE_TABLE: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFF, 0xFFFF, 0x0178, 0x00A0, 0x00A1, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00BA, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00BF, 0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF, 0x011E, 0x00D1, 0x00D2, 0x00D3,
    0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x0130, 0x015E, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x00EC, 0x00ED, 0x00EE, 0x00EF, 0x011F, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x0131, 0x015F, 0x00FF,
];
const CP1255_DECODE_TABLE: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0xFFFF, 0x2039,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0xFFFF, 0x203A, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x00A0, 0x00A1, 0x00A2, 0x00A3,
    0x20AA, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00D7, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00F7, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00BF, 0x05B0, 0x05B1, 0x05B2, 0x05B3, 0x05B4, 0x05B5, 0x05B6, 0x05B7,
    0x05B8, 0x05B9, 0xFFFF, 0x05BB, 0x05BC, 0x05BD, 0x05BE, 0x05BF, 0x05C0, 0x05C1, 0x05C2, 0x05C3,
    0x05F0, 0x05F1, 0x05F2, 0x05F3, 0x05F4, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0x05D0, 0x05D1, 0x05D2, 0x05D3, 0x05D4, 0x05D5, 0x05D6, 0x05D7, 0x05D8, 0x05D9, 0x05DA, 0x05DB,
    0x05DC, 0x05DD, 0x05DE, 0x05DF, 0x05E0, 0x05E1, 0x05E2, 0x05E3, 0x05E4, 0x05E5, 0x05E6, 0x05E7,
    0x05E8, 0x05E9, 0x05EA, 0xFFFF, 0xFFFF, 0x200E, 0x200F, 0xFFFF,
];
const CP1256_DECODE_TABLE: [u16; 128] = [
    0x20AC, 0x067E, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0679, 0x2039,
    0x0152, 0x0686, 0x0698, 0x0688, 0x06AF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x06A9, 0x2122, 0x0691, 0x203A, 0x0153, 0x200C, 0x200D, 0x06BA, 0x00A0, 0x060C, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x06BE, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x061B, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x061F, 0x06C1, 0x0621, 0x0622, 0x0623, 0x0624, 0x0625, 0x0626, 0x0627,
    0x0628, 0x0629, 0x062A, 0x062B, 0x062C, 0x062D, 0x062E, 0x062F, 0x0630, 0x0631, 0x0632, 0x0633,
    0x0634, 0x0635, 0x0636, 0x00D7, 0x0637, 0x0638, 0x0639, 0x063A, 0x0640, 0x0641, 0x0642, 0x0643,
    0x00E0, 0x0644, 0x00E2, 0x0645, 0x0646, 0x0647, 0x0648, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x0649, 0x064A, 0x00EE, 0x00EF, 0x064B, 0x064C, 0x064D, 0x064E, 0x00F4, 0x064F, 0x0650, 0x00F7,
    0x0651, 0x00F9, 0x0652, 0x00FB, 0x00FC, 0x200E, 0x200F, 0x06D2,
];
const CP1257_DECODE_TABLE: [u16; 128] = [
    0x20AC, 0xFFFF, 0x201A, 0xFFFF, 0x201E, 0x2026, 0x2020, 0x2021, 0xFFFF, 0x2030, 0xFFFF, 0x2039,
    0xFFFF, 0x00A8, 0x02C7, 0x00B8, 0xFFFF, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFF, 0x2122, 0xFFFF, 0x203A, 0xFFFF, 0x00AF, 0x02DB, 0xFFFF, 0x00A0, 0xFFFF, 0x00A2, 0x00A3,
    0x00A4, 0xFFFF, 0x00A6, 0x00A7, 0x00D8, 0x00A9, 0x0156, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00C6,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00F8, 0x00B9, 0x0157, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0x00E6, 0x0104, 0x012E, 0x0100, 0x0106, 0x00C4, 0x00C5, 0x0118, 0x0112,
    0x010C, 0x00C9, 0x0179, 0x0116, 0x0122, 0x0136, 0x012A, 0x013B, 0x0160, 0x0143, 0x0145, 0x00D3,
    0x014C, 0x00D5, 0x00D6, 0x00D7, 0x0172, 0x0141, 0x015A, 0x016A, 0x00DC, 0x017B, 0x017D, 0x00DF,
    0x0105, 0x012F, 0x0101, 0x0107, 0x00E4, 0x00E5, 0x0119, 0x0113, 0x010D, 0x00E9, 0x017A, 0x0117,
    0x0123, 0x0137, 0x012B, 0x013C, 0x0161, 0x0144, 0x0146, 0x00F3, 0x014D, 0x00F5, 0x00F6, 0x00F7,
    0x0173, 0x0142, 0x015B, 0x016B, 0x00FC, 0x017C, 0x017E, 0x02D9,
];
const KOI8_R_DECODE_TABLE: [u16; 128] = [
    0x2500, 0x2502, 0x250C, 0x2510, 0x2514, 0x2518, 0x251C, 0x2524, 0x252C, 0x2534, 0x253C, 0x2580,
    0x2584, 0x2588, 0x258C, 0x2590, 0x2591, 0x2592, 0x2593, 0x2320, 0x25A0, 0x2219, 0x221A, 0x2248,
    0x2264, 0x2265, 0x00A0, 0x2321, 0x00B0, 0x00B2, 0x00B7, 0x00F7, 0x2550, 0x2551, 0x2552, 0x0451,
    0x2553, 0x2554, 0x2555, 0x2556, 0x2557, 0x2558, 0x2559, 0x255A, 0x255B, 0x255C, 0x255D, 0x255E,
    0x255F, 0x2560, 0x2561, 0x0401, 0x2562, 0x2563, 0x2564, 0x2565, 0x2566, 0x2567, 0x2568, 0x2569,
    0x256A, 0x256B, 0x256C, 0x00A9, 0x044E, 0x0430, 0x0431, 0x0446, 0x0434, 0x0435, 0x0444, 0x0433,
    0x0445, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F, 0x044F, 0x0440, 0x0441,
    0x0442, 0x0443, 0x0436, 0x0432, 0x044C, 0x044B, 0x0437, 0x0448, 0x044D, 0x0449, 0x0447, 0x044A,
    0x042E, 0x0410, 0x0411, 0x0426, 0x0414, 0x0415, 0x0424, 0x0413, 0x0425, 0x0418, 0x0419, 0x041A,
    0x041B, 0x041C, 0x041D, 0x041E, 0x041F, 0x042F, 0x0420, 0x0421, 0x0422, 0x0423, 0x0416, 0x0412,
    0x042C, 0x042B, 0x0417, 0x0428, 0x042D, 0x0429, 0x0427, 0x042A,
];

const KOI8_U_DECODE_TABLE: [u16; 128] = [
    0x2500, 0x2502, 0x250C, 0x2510, 0x2514, 0x2518, 0x251C, 0x2524, 0x252C, 0x2534, 0x253C, 0x2580,
    0x2584, 0x2588, 0x258C, 0x2590, 0x2591, 0x2592, 0x2593, 0x2320, 0x25A0, 0x2219, 0x221A, 0x2248,
    0x2264, 0x2265, 0x00A0, 0x2321, 0x00B0, 0x00B2, 0x00B7, 0x00F7, 0x2550, 0x2551, 0x2552, 0x0451,
    0x0454, 0x2554, 0x0456, 0x0457, 0x2557, 0x2558, 0x2559, 0x255A, 0x255B, 0x0491, 0x255D, 0x255E,
    0x255F, 0x2560, 0x2561, 0x0401, 0x0404, 0x2563, 0x0406, 0x0407, 0x2566, 0x2567, 0x2568, 0x2569,
    0x256A, 0x0490, 0x256C, 0x00A9, 0x044E, 0x0430, 0x0431, 0x0446, 0x0434, 0x0435, 0x0444, 0x0433,
    0x0445, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F, 0x044F, 0x0440, 0x0441,
    0x0442, 0x0443, 0x0436, 0x0432, 0x044C, 0x044B, 0x0437, 0x0448, 0x044D, 0x0449, 0x0447, 0x044A,
    0x042E, 0x0410, 0x0411, 0x0426, 0x0414, 0x0415, 0x0424, 0x0413, 0x0425, 0x0418, 0x0419, 0x041A,
    0x041B, 0x041C, 0x041D, 0x041E, 0x041F, 0x042F, 0x0420, 0x0421, 0x0422, 0x0423, 0x0416, 0x0412,
    0x042C, 0x042B, 0x0417, 0x0428, 0x042D, 0x0429, 0x0427, 0x042A,
];

const ISO8859_2_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0x0104, 0x02D8, 0x0141,
    0x00A4, 0x013D, 0x015A, 0x00A7, 0x00A8, 0x0160, 0x015E, 0x0164, 0x0179, 0x00AD, 0x017D, 0x017B,
    0x00B0, 0x0105, 0x02DB, 0x0142, 0x00B4, 0x013E, 0x015B, 0x02C7, 0x00B8, 0x0161, 0x015F, 0x0165,
    0x017A, 0x02DD, 0x017E, 0x017C, 0x0154, 0x00C1, 0x00C2, 0x0102, 0x00C4, 0x0139, 0x0106, 0x00C7,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x011A, 0x00CD, 0x00CE, 0x010E, 0x0110, 0x0143, 0x0147, 0x00D3,
    0x00D4, 0x0150, 0x00D6, 0x00D7, 0x0158, 0x016E, 0x00DA, 0x0170, 0x00DC, 0x00DD, 0x0162, 0x00DF,
    0x0155, 0x00E1, 0x00E2, 0x0103, 0x00E4, 0x013A, 0x0107, 0x00E7, 0x010D, 0x00E9, 0x0119, 0x00EB,
    0x011B, 0x00ED, 0x00EE, 0x010F, 0x0111, 0x0144, 0x0148, 0x00F3, 0x00F4, 0x0151, 0x00F6, 0x00F7,
    0x0159, 0x016F, 0x00FA, 0x0171, 0x00FC, 0x00FD, 0x0163, 0x02D9,
];

const ISO8859_3_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0x0126, 0x02D8, 0x00A3,
    0x00A4, 0xFFFF, 0x0124, 0x00A7, 0x00A8, 0x0130, 0x015E, 0x011E, 0x0134, 0x00AD, 0xFFFF, 0x017B,
    0x00B0, 0x0127, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x0125, 0x00B7, 0x00B8, 0x0131, 0x015F, 0x011F,
    0x0135, 0x00BD, 0xFFFF, 0x017C, 0x00C0, 0x00C1, 0x00C2, 0xFFFF, 0x00C4, 0x010A, 0x0108, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF, 0xFFFF, 0x00D1, 0x00D2, 0x00D3,
    0x00D4, 0x0120, 0x00D6, 0x00D7, 0x011C, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x016C, 0x015C, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0xFFFF, 0x00E4, 0x010B, 0x0109, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x00EC, 0x00ED, 0x00EE, 0x00EF, 0xFFFF, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x0121, 0x00F6, 0x00F7,
    0x011D, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x016D, 0x015D, 0x02D9,
];
const ISO8859_4_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0x0104, 0x0138, 0x0156,
    0x00A4, 0x0128, 0x013B, 0x00A7, 0x00A8, 0x0160, 0x0112, 0x0122, 0x0166, 0x00AD, 0x017D, 0x00AF,
    0x00B0, 0x0105, 0x02DB, 0x0157, 0x00B4, 0x0129, 0x013C, 0x02C7, 0x00B8, 0x0161, 0x0113, 0x0123,
    0x0167, 0x014A, 0x017E, 0x014B, 0x0100, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x012E,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x0116, 0x00CD, 0x00CE, 0x012A, 0x0110, 0x0145, 0x014C, 0x0136,
    0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x0172, 0x00DA, 0x00DB, 0x00DC, 0x0168, 0x016A, 0x00DF,
    0x0101, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x012F, 0x010D, 0x00E9, 0x0119, 0x00EB,
    0x0117, 0x00ED, 0x00EE, 0x012B, 0x0111, 0x0146, 0x014D, 0x0137, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x0173, 0x00FA, 0x00FB, 0x00FC, 0x0169, 0x016B, 0x02D9,
];
const ISO8859_5_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0x0401, 0x0402, 0x0403,
    0x0404, 0x0405, 0x0406, 0x0407, 0x0408, 0x0409, 0x040A, 0x040B, 0x040C, 0x00AD, 0x040E, 0x040F,
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417, 0x0418, 0x0419, 0x041A, 0x041B,
    0x041C, 0x041D, 0x041E, 0x041F, 0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427,
    0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F, 0x0430, 0x0431, 0x0432, 0x0433,
    0x0434, 0x0435, 0x0436, 0x0437, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447, 0x0448, 0x0449, 0x044A, 0x044B,
    0x044C, 0x044D, 0x044E, 0x044F, 0x2116, 0x0451, 0x0452, 0x0453, 0x0454, 0x0455, 0x0456, 0x0457,
    0x0458, 0x0459, 0x045A, 0x045B, 0x045C, 0x00A7, 0x045E, 0x045F,
];

const ISO8859_6_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0xFFFF, 0xFFFF, 0xFFFF,
    0x00A4, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x060C, 0x00AD, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x061B,
    0xFFFF, 0xFFFF, 0xFFFF, 0x061F, 0xFFFF, 0x0621, 0x0622, 0x0623, 0x0624, 0x0625, 0x0626, 0x0627,
    0x0628, 0x0629, 0x062A, 0x062B, 0x062C, 0x062D, 0x062E, 0x062F, 0x0630, 0x0631, 0x0632, 0x0633,
    0x0634, 0x0635, 0x0636, 0x0637, 0x0638, 0x0639, 0x063A, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0x0640, 0x0641, 0x0642, 0x0643, 0x0644, 0x0645, 0x0646, 0x0647, 0x0648, 0x0649, 0x064A, 0x064B,
    0x064C, 0x064D, 0x064E, 0x064F, 0x0650, 0x0651, 0x0652, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
];
const ISO8859_7_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0x2018, 0x2019, 0x00A3,
    0x20AC, 0x20AF, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x037A, 0x00AB, 0x00AC, 0x00AD, 0xFFFF, 0x2015,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x0384, 0x0385, 0x0386, 0x00B7, 0x0388, 0x0389, 0x038A, 0x00BB,
    0x038C, 0x00BD, 0x038E, 0x038F, 0x0390, 0x0391, 0x0392, 0x0393, 0x0394, 0x0395, 0x0396, 0x0397,
    0x0398, 0x0399, 0x039A, 0x039B, 0x039C, 0x039D, 0x039E, 0x039F, 0x03A0, 0x03A1, 0xFFFF, 0x03A3,
    0x03A4, 0x03A5, 0x03A6, 0x03A7, 0x03A8, 0x03A9, 0x03AA, 0x03AB, 0x03AC, 0x03AD, 0x03AE, 0x03AF,
    0x03B0, 0x03B1, 0x03B2, 0x03B3, 0x03B4, 0x03B5, 0x03B6, 0x03B7, 0x03B8, 0x03B9, 0x03BA, 0x03BB,
    0x03BC, 0x03BD, 0x03BE, 0x03BF, 0x03C0, 0x03C1, 0x03C2, 0x03C3, 0x03C4, 0x03C5, 0x03C6, 0x03C7,
    0x03C8, 0x03C9, 0x03CA, 0x03CB, 0x03CC, 0x03CD, 0x03CE, 0xFFFF,
];

const ISO8859_8_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0xFFFF, 0x00A2, 0x00A3,
    0x00A4, 0x00A5, 0x00A6, 0x00A7, 0x00A8, 0x00A9, 0x00D7, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x00B4, 0x00B5, 0x00B6, 0x00B7, 0x00B8, 0x00B9, 0x00F7, 0x00BB,
    0x00BC, 0x00BD, 0x00BE, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF,
    0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0xFFFF, 0x2017,
    0x05D0, 0x05D1, 0x05D2, 0x05D3, 0x05D4, 0x05D5, 0x05D6, 0x05D7, 0x05D8, 0x05D9, 0x05DA, 0x05DB,
    0x05DC, 0x05DD, 0x05DE, 0x05DF, 0x05E0, 0x05E1, 0x05E2, 0x05E3, 0x05E4, 0x05E5, 0x05E6, 0x05E7,
    0x05E8, 0x05E9, 0x05EA, 0xFFFF, 0xFFFF, 0x200E, 0x200F, 0xFFFF,
];
const ISO8859_10_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0x0104, 0x0112, 0x0122,
    0x012A, 0x0128, 0x0136, 0x00A7, 0x013B, 0x0110, 0x0160, 0x0166, 0x017D, 0x00AD, 0x016A, 0x014A,
    0x00B0, 0x0105, 0x0113, 0x0123, 0x012B, 0x0129, 0x0137, 0x00B7, 0x013C, 0x0111, 0x0161, 0x0167,
    0x017E, 0x2015, 0x016B, 0x014B, 0x0100, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x012E,
    0x010C, 0x00C9, 0x0118, 0x00CB, 0x0116, 0x00CD, 0x00CE, 0x00CF, 0x00D0, 0x0145, 0x014C, 0x00D3,
    0x00D4, 0x00D5, 0x00D6, 0x0168, 0x00D8, 0x0172, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x0101, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x012F, 0x010D, 0x00E9, 0x0119, 0x00EB,
    0x0117, 0x00ED, 0x00EE, 0x00EF, 0x00F0, 0x0146, 0x014D, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x0169,
    0x00F8, 0x0173, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x0138,
];
const ISO8859_15_DECODE_TABLE: [u16; 128] = [
    0x0080, 0x0081, 0x0082, 0x0083, 0x0084, 0x0085, 0x0086, 0x0087, 0x0088, 0x0089, 0x008A, 0x008B,
    0x008C, 0x008D, 0x008E, 0x008F, 0x0090, 0x0091, 0x0092, 0x0093, 0x0094, 0x0095, 0x0096, 0x0097,
    0x0098, 0x0099, 0x009A, 0x009B, 0x009C, 0x009D, 0x009E, 0x009F, 0x00A0, 0x00A1, 0x00A2, 0x00A3,
    0x20AC, 0x00A5, 0x0160, 0x00A7, 0x0161, 0x00A9, 0x00AA, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x00AF,
    0x00B0, 0x00B1, 0x00B2, 0x00B3, 0x017D, 0x00B5, 0x00B6, 0x00B7, 0x017E, 0x00B9, 0x00BA, 0x00BB,
    0x0152, 0x0153, 0x0178, 0x00BF, 0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00C7,
    0x00C8, 0x00C9, 0x00CA, 0x00CB, 0x00CC, 0x00CD, 0x00CE, 0x00CF, 0x00D0, 0x00D1, 0x00D2, 0x00D3,
    0x00D4, 0x00D5, 0x00D6, 0x00D7, 0x00D8, 0x00D9, 0x00DA, 0x00DB, 0x00DC, 0x00DD, 0x00DE, 0x00DF,
    0x00E0, 0x00E1, 0x00E2, 0x00E3, 0x00E4, 0x00E5, 0x00E6, 0x00E7, 0x00E8, 0x00E9, 0x00EA, 0x00EB,
    0x00EC, 0x00ED, 0x00EE, 0x00EF, 0x00F0, 0x00F1, 0x00F2, 0x00F3, 0x00F4, 0x00F5, 0x00F6, 0x00F7,
    0x00F8, 0x00F9, 0x00FA, 0x00FB, 0x00FC, 0x00FD, 0x00FE, 0x00FF,
];

const MAC_ROMAN_DECODE_TABLE: [u16; 128] = [
    0x00C4, 0x00C5, 0x00C7, 0x00C9, 0x00D1, 0x00D6, 0x00DC, 0x00E1, 0x00E0, 0x00E2, 0x00E4, 0x00E3,
    0x00E5, 0x00E7, 0x00E9, 0x00E8, 0x00EA, 0x00EB, 0x00ED, 0x00EC, 0x00EE, 0x00EF, 0x00F1, 0x00F3,
    0x00F2, 0x00F4, 0x00F6, 0x00F5, 0x00FA, 0x00F9, 0x00FB, 0x00FC, 0x2020, 0x00B0, 0x00A2, 0x00A3,
    0x00A7, 0x2022, 0x00B6, 0x00DF, 0x00AE, 0x00A9, 0x2122, 0x00B4, 0x00A8, 0x2260, 0x00C6, 0x00D8,
    0x221E, 0x00B1, 0x2264, 0x2265, 0x00A5, 0x00B5, 0x2202, 0x2211, 0x220F, 0x03C0, 0x222B, 0x00AA,
    0x00BA, 0x03A9, 0x00E6, 0x00F8, 0x00BF, 0x00A1, 0x00AC, 0x221A, 0x0192, 0x2248, 0x2206, 0x00AB,
    0x00BB, 0x2026, 0x00A0, 0x00C0, 0x00C3, 0x00D5, 0x0152, 0x0153, 0x2013, 0x2014, 0x201C, 0x201D,
    0x2018, 0x2019, 0x00F7, 0x25CA, 0x00FF, 0x0178, 0x2044, 0x20AC, 0x2039, 0x203A, 0xFB01, 0xFB02,
    0x2021, 0x00B7, 0x201A, 0x201E, 0x2030, 0x00C2, 0x00CA, 0x00C1, 0x00CB, 0x00C8, 0x00CD, 0x00CE,
    0x00CF, 0x00CC, 0x00D3, 0x00D4, 0xF8FF, 0x00D2, 0x00DA, 0x00DB, 0x00D9, 0x0131, 0x02C6, 0x02DC,
    0x00AF, 0x02D8, 0x02D9, 0x02DA, 0x00B8, 0x02DD, 0x02DB, 0x02C7,
];

fn cp1252_decode_byte(byte: u8) -> Option<u32> {
    if byte <= 0x7F || byte >= 0xA0 {
        return Some(byte as u32);
    }
    let idx = (byte - 0x80) as usize;
    let code = CP1252_DECODE_TABLE[idx];
    if code == 0xFFFF {
        None
    } else {
        Some(code as u32)
    }
}

fn decode_cp1252_with_errors(bytes: &[u8], errors: &str) -> Result<Vec<u8>, DecodeFailure> {
    let mut out = Vec::with_capacity(bytes.len());
    for (idx, &byte) in bytes.iter().enumerate() {
        if let Some(code) = cp1252_decode_byte(byte) {
            push_wtf8_codepoint(&mut out, code);
            continue;
        }
        match errors {
            "ignore" => {}
            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
            "backslashreplace" => push_backslash_bytes_vec(&mut out, &[byte]),
            "surrogateescape" => push_wtf8_codepoint(&mut out, 0xDC00 + byte as u32),
            "strict" | "surrogatepass" => {
                return Err(DecodeFailure::Byte {
                    pos: idx,
                    byte,
                    message: "character maps to <undefined>",
                });
            }
            other => {
                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
            }
        }
    }
    Ok(out)
}

fn decode_charmap_with_errors(
    bytes: &[u8],
    errors: &str,
    table: &[u16; 128],
) -> Result<Vec<u8>, DecodeFailure> {
    let mut out = Vec::with_capacity(bytes.len());
    for (idx, &byte) in bytes.iter().enumerate() {
        if byte <= 0x7F {
            out.push(byte);
            continue;
        }
        let code = table[(byte - 0x80) as usize];
        if code != 0xFFFF {
            push_wtf8_codepoint(&mut out, code as u32);
            continue;
        }
        match errors {
            "ignore" => {}
            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
            "backslashreplace" => push_backslash_bytes_vec(&mut out, &[byte]),
            "surrogateescape" => push_wtf8_codepoint(&mut out, 0xDC00 + byte as u32),
            "strict" | "surrogatepass" => {
                return Err(DecodeFailure::Byte {
                    pos: idx,
                    byte,
                    message: "character maps to <undefined>",
                });
            }
            other => {
                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
            }
        }
    }
    Ok(out)
}

fn decode_utf8_bytes_with_errors(bytes: &[u8], errors: &str) -> Result<Vec<u8>, DecodeFailure> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    let allow_surrogates = errors == "surrogatepass";
    while idx < bytes.len() {
        let first = bytes[idx];
        if first < 0x80 {
            out.push(first);
            idx += 1;
            continue;
        }
        if first < 0xC0 {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        }
        let (needed, min_code) = if first < 0xE0 {
            (1usize, 0x80u32)
        } else if first < 0xF0 {
            (2usize, 0x800u32)
        } else if first < 0xF8 {
            (3usize, 0x10000u32)
        } else {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        };
        if idx + needed >= bytes.len() {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        }
        let mut code: u32 = (first & (0x7F >> needed)) as u32;
        let mut ok = true;
        for off in 1..=needed {
            let byte = bytes[idx + off];
            if (byte & 0xC0) != 0x80 {
                ok = false;
                break;
            }
            code = (code << 6) | (byte & 0x3F) as u32;
        }
        if !ok || code < min_code || code > 0x10FFFF {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        }
        if is_surrogate(code) && !allow_surrogates {
            decode_utf8_invalid_byte(errors, &mut out, idx, first)?;
            idx += 1;
            continue;
        }
        push_wtf8_codepoint(&mut out, code);
        idx += needed + 1;
    }
    Ok(out)
}

fn decode_utf8_invalid_byte(
    errors: &str,
    out: &mut Vec<u8>,
    pos: usize,
    byte: u8,
) -> Result<(), DecodeFailure> {
    match errors {
        "ignore" => Ok(()),
        "replace" => {
            push_wtf8_codepoint(out, 0xFFFD);
            Ok(())
        }
        "backslashreplace" => {
            push_backslash_bytes_vec(out, &[byte]);
            Ok(())
        }
        "surrogateescape" => {
            push_wtf8_codepoint(out, 0xDC00 + byte as u32);
            Ok(())
        }
        "strict" | "surrogatepass" => Err(DecodeFailure::Byte {
            pos,
            byte,
            message: "invalid start byte",
        }),
        other => Err(DecodeFailure::UnknownErrorHandler(other.to_string())),
    }
}

fn hex_value(byte: u8) -> Option<u32> {
    match byte {
        b'0'..=b'9' => Some((byte - b'0') as u32),
        b'a'..=b'f' => Some((byte - b'a' + 10) as u32),
        b'A'..=b'F' => Some((byte - b'A' + 10) as u32),
        _ => None,
    }
}

fn parse_hex_prefix(bytes: &[u8], max: usize) -> (u32, usize) {
    let mut value = 0u32;
    let mut count = 0usize;
    for &byte in bytes.iter().take(max) {
        let Some(digit) = hex_value(byte) else {
            break;
        };
        value = (value << 4) | digit;
        count += 1;
    }
    (value, count)
}

fn parse_octal_prefix(bytes: &[u8], max: usize) -> (u32, usize) {
    let mut value = 0u32;
    let mut count = 0usize;
    for &byte in bytes.iter().take(max) {
        if !(b'0'..=b'7').contains(&byte) {
            break;
        }
        value = (value << 3) | (byte - b'0') as u32;
        count += 1;
    }
    (value, count)
}

fn handle_unicode_escape_failure(
    errors: &str,
    out: &mut Vec<u8>,
    bytes: &[u8],
    start: usize,
    end: usize,
    failure: DecodeFailure,
) -> Result<usize, DecodeFailure> {
    match errors {
        "ignore" => Ok(end + 1),
        "replace" => {
            push_wtf8_codepoint(out, 0xFFFD);
            Ok(end + 1)
        }
        "backslashreplace" => {
            if start <= end && end < bytes.len() {
                push_backslash_bytes_vec(out, &bytes[start..=end]);
            }
            Ok(end + 1)
        }
        "strict" | "surrogatepass" | "surrogateescape" => Err(failure),
        other => Err(DecodeFailure::UnknownErrorHandler(other.to_string())),
    }
}

fn decode_unicode_escape_with_errors(bytes: &[u8], errors: &str) -> Result<Vec<u8>, DecodeFailure> {
    const TRUNC_X: &str = "truncated \\xXX escape";
    const TRUNC_U: &str = "truncated \\uXXXX escape";
    const TRUNC_U8: &str = "truncated \\UXXXXXXXX escape";
    const MALFORMED_N: &str = "malformed \\N character escape";
    const UNKNOWN_NAME: &str = "unknown Unicode character name";
    const ILLEGAL_UNICODE: &str = "illegal Unicode character";
    const TRAILING_SLASH: &str = "\\ at end of string";

    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0usize;
    while idx < bytes.len() {
        let byte = bytes[idx];
        if byte != b'\\' {
            push_wtf8_codepoint(&mut out, byte as u32);
            idx += 1;
            continue;
        }
        if idx + 1 >= bytes.len() {
            let failure = DecodeFailure::Byte {
                pos: idx,
                byte,
                message: TRAILING_SLASH,
            };
            idx = handle_unicode_escape_failure(errors, &mut out, bytes, idx, idx, failure)?;
            continue;
        }
        let esc = bytes[idx + 1];
        match esc {
            b'\\' => {
                push_wtf8_codepoint(&mut out, b'\\' as u32);
                idx += 2;
            }
            b'\'' => {
                push_wtf8_codepoint(&mut out, b'\'' as u32);
                idx += 2;
            }
            b'"' => {
                push_wtf8_codepoint(&mut out, b'"' as u32);
                idx += 2;
            }
            b'a' => {
                push_wtf8_codepoint(&mut out, 0x07);
                idx += 2;
            }
            b'b' => {
                push_wtf8_codepoint(&mut out, 0x08);
                idx += 2;
            }
            b't' => {
                push_wtf8_codepoint(&mut out, 0x09);
                idx += 2;
            }
            b'n' => {
                push_wtf8_codepoint(&mut out, 0x0A);
                idx += 2;
            }
            b'v' => {
                push_wtf8_codepoint(&mut out, 0x0B);
                idx += 2;
            }
            b'f' => {
                push_wtf8_codepoint(&mut out, 0x0C);
                idx += 2;
            }
            b'r' => {
                push_wtf8_codepoint(&mut out, 0x0D);
                idx += 2;
            }
            b'\n' => {
                idx += 2;
            }
            b'x' => {
                let (value, count) = parse_hex_prefix(&bytes[idx + 2..], 2);
                if count < 2 {
                    let end = idx + 1 + count;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: TRUNC_X,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                push_wtf8_codepoint(&mut out, value);
                idx += 4;
            }
            b'u' => {
                let (value, count) = parse_hex_prefix(&bytes[idx + 2..], 4);
                if count < 4 {
                    let end = idx + 1 + count;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: TRUNC_U,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                push_wtf8_codepoint(&mut out, value);
                idx += 6;
            }
            b'U' => {
                let (value, count) = parse_hex_prefix(&bytes[idx + 2..], 8);
                if count < 8 {
                    let end = idx + 1 + count;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: TRUNC_U8,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                if value > 0x10FFFF {
                    let end = idx + 9;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: ILLEGAL_UNICODE,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                push_wtf8_codepoint(&mut out, value);
                idx += 10;
            }
            b'N' => {
                if idx + 2 >= bytes.len() || bytes[idx + 2] != b'{' {
                    let end = usize::min(idx + 1, bytes.len() - 1);
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: MALFORMED_N,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                }
                let close = bytes[idx + 3..]
                    .iter()
                    .position(|&ch| ch == b'}')
                    .map(|offset| idx + 3 + offset);
                let Some(close_idx) = close else {
                    let end = bytes.len() - 1;
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end,
                        message: MALFORMED_N,
                    };
                    idx =
                        handle_unicode_escape_failure(errors, &mut out, bytes, idx, end, failure)?;
                    continue;
                };
                let name_bytes = &bytes[idx + 3..close_idx];
                let name = std::str::from_utf8(name_bytes).unwrap_or("");
                #[cfg(feature = "stdlib_unicode_names")]
                let resolved = unicode_names2::character(name);
                #[cfg(not(feature = "stdlib_unicode_names"))]
                let resolved: Option<char> = None;
                if let Some(ch) = resolved {
                    push_wtf8_codepoint(&mut out, ch as u32);
                    idx = close_idx + 1;
                } else {
                    let failure = DecodeFailure::Range {
                        start: idx,
                        end: close_idx,
                        message: UNKNOWN_NAME,
                    };
                    idx = handle_unicode_escape_failure(
                        errors, &mut out, bytes, idx, close_idx, failure,
                    )?;
                }
            }
            b'0'..=b'7' => {
                let (value, count) = parse_octal_prefix(&bytes[idx + 1..], 3);
                push_wtf8_codepoint(&mut out, value);
                idx += 1 + count;
            }
            _ => {
                push_wtf8_codepoint(&mut out, b'\\' as u32);
                push_wtf8_codepoint(&mut out, esc as u32);
                idx += 2;
            }
        }
    }
    Ok(out)
}

fn utf16_decode_config(bytes: &[u8], kind: EncodingKind) -> (Endian, String, usize) {
    match kind {
        EncodingKind::Utf16 => {
            if bytes.len() >= 2 {
                if bytes[0] == 0xFF && bytes[1] == 0xFE {
                    return (Endian::Little, "utf-16-le".to_string(), 2);
                }
                if bytes[0] == 0xFE && bytes[1] == 0xFF {
                    return (Endian::Big, "utf-16-be".to_string(), 2);
                }
            }
            let endian = native_endian();
            let label = match endian {
                Endian::Little => "utf-16-le".to_string(),
                Endian::Big => "utf-16-be".to_string(),
            };
            (endian, label, 0)
        }
        EncodingKind::Utf16LE => (Endian::Little, "utf-16-le".to_string(), 0),
        EncodingKind::Utf16BE => (Endian::Big, "utf-16-be".to_string(), 0),
        _ => (native_endian(), "utf-16-le".to_string(), 0),
    }
}

fn decode_utf16_with_errors(
    bytes: &[u8],
    errors: &str,
    endian: Endian,
    offset: usize,
) -> Result<Vec<u8>, DecodeFailure> {
    let data = if offset > 0 { &bytes[offset..] } else { bytes };
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx + 1 < data.len() {
        let unit = read_u16(data, idx, endian);
        if (0xD800..=0xDBFF).contains(&unit) {
            if idx + 3 >= data.len() {
                match errors {
                    "surrogatepass" | "surrogateescape" => {
                        push_wtf8_codepoint(&mut out, unit as u32);
                    }
                    "ignore" => {}
                    "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                    "backslashreplace" => {
                        push_backslash_bytes_vec(&mut out, &data[idx..]);
                    }
                    "strict" => {
                        return Err(DecodeFailure::Range {
                            start: offset + idx,
                            end: offset + data.len() - 1,
                            message: "unexpected end of data",
                        });
                    }
                    other => {
                        return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                    }
                }
                // Avoid double-applying trailing bytes in the post-loop remainder handler.
                idx = data.len();
                break;
            }
            let next = read_u16(data, idx + 2, endian);
            if (0xDC00..=0xDFFF).contains(&next) {
                let high = (unit as u32) - 0xD800;
                let low = (next as u32) - 0xDC00;
                let code = 0x10000 + ((high << 10) | low);
                push_wtf8_codepoint(&mut out, code);
                idx += 4;
                continue;
            }
            match errors {
                "surrogatepass" | "surrogateescape" => {
                    push_wtf8_codepoint(&mut out, unit as u32);
                }
                "ignore" => {}
                "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                "backslashreplace" => {
                    push_backslash_bytes_vec(&mut out, &data[idx..idx + 2]);
                }
                "strict" => {
                    return Err(DecodeFailure::Range {
                        start: offset + idx,
                        end: offset + idx + 1,
                        message: "illegal UTF-16 surrogate",
                    });
                }
                other => {
                    return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                }
            }
            idx += 2;
            continue;
        }
        if (0xDC00..=0xDFFF).contains(&unit) {
            match errors {
                "surrogatepass" | "surrogateescape" => {
                    push_wtf8_codepoint(&mut out, unit as u32);
                }
                "ignore" => {}
                "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                "backslashreplace" => {
                    push_backslash_bytes_vec(&mut out, &data[idx..idx + 2]);
                }
                "strict" => {
                    return Err(DecodeFailure::Range {
                        start: offset + idx,
                        end: offset + idx + 1,
                        message: "illegal encoding",
                    });
                }
                other => {
                    return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                }
            }
            idx += 2;
            continue;
        }
        push_wtf8_codepoint(&mut out, unit as u32);
        idx += 2;
    }
    if idx < data.len() {
        match errors {
            "surrogatepass" | "surrogateescape" => {
                push_wtf8_codepoint(&mut out, data[idx] as u32);
            }
            "ignore" => {}
            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
            "backslashreplace" => {
                push_backslash_bytes_vec(&mut out, &data[idx..]);
            }
            "strict" => {
                let pos = offset + data.len() - 1;
                let byte = data[data.len() - 1];
                return Err(DecodeFailure::Byte {
                    pos,
                    byte,
                    message: "truncated data",
                });
            }
            other => {
                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
            }
        }
    }
    Ok(out)
}

fn utf32_decode_config(bytes: &[u8], kind: EncodingKind) -> (Endian, String, usize) {
    match kind {
        EncodingKind::Utf32 => {
            if bytes.len() >= 4 {
                if bytes[0] == 0xFF && bytes[1] == 0xFE && bytes[2] == 0x00 && bytes[3] == 0x00 {
                    return (Endian::Little, "utf-32-le".to_string(), 4);
                }
                if bytes[0] == 0x00 && bytes[1] == 0x00 && bytes[2] == 0xFE && bytes[3] == 0xFF {
                    return (Endian::Big, "utf-32-be".to_string(), 4);
                }
            }
            let endian = native_endian();
            let label = match endian {
                Endian::Little => "utf-32-le".to_string(),
                Endian::Big => "utf-32-be".to_string(),
            };
            (endian, label, 0)
        }
        EncodingKind::Utf32LE => (Endian::Little, "utf-32-le".to_string(), 0),
        EncodingKind::Utf32BE => (Endian::Big, "utf-32-be".to_string(), 0),
        _ => (native_endian(), "utf-32-le".to_string(), 0),
    }
}

fn decode_utf32_with_errors(
    bytes: &[u8],
    errors: &str,
    endian: Endian,
    offset: usize,
) -> Result<Vec<u8>, DecodeFailure> {
    let data = if offset > 0 { &bytes[offset..] } else { bytes };
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx + 3 < data.len() {
        let code = read_u32(data, idx, endian);
        if is_surrogate(code) {
            match errors {
                "surrogatepass" | "surrogateescape" => {
                    push_wtf8_codepoint(&mut out, code);
                }
                "ignore" => {}
                "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                "backslashreplace" => {
                    push_backslash_bytes_vec(&mut out, &data[idx..idx + 4]);
                }
                "strict" => {
                    return Err(DecodeFailure::Range {
                        start: offset + idx,
                        end: offset + idx + 3,
                        message: "code point in surrogate code point range(0xd800, 0xe000)",
                    });
                }
                other => {
                    return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                }
            }
            idx += 4;
            continue;
        }
        if code > 0x10FFFF {
            match errors {
                "ignore" => {}
                "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
                "backslashreplace" => {
                    push_backslash_bytes_vec(&mut out, &data[idx..idx + 4]);
                }
                "strict" | "surrogatepass" | "surrogateescape" => {
                    return Err(DecodeFailure::Range {
                        start: offset + idx,
                        end: offset + idx + 3,
                        message: "code point not in range(0x110000)",
                    });
                }
                other => {
                    return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
                }
            }
            idx += 4;
            continue;
        }
        push_wtf8_codepoint(&mut out, code);
        idx += 4;
    }
    if idx < data.len() {
        match errors {
            "surrogatepass" | "surrogateescape" => {
                for &byte in &data[idx..] {
                    push_wtf8_codepoint(&mut out, 0xDC00 + byte as u32);
                }
            }
            "ignore" => {}
            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),
            "backslashreplace" => {
                push_backslash_bytes_vec(&mut out, &data[idx..]);
            }
            "strict" => {
                return Err(DecodeFailure::Range {
                    start: offset + idx,
                    end: offset + data.len() - 1,
                    message: "truncated data",
                });
            }
            other => {
                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));
            }
        }
    }
    Ok(out)
}

fn decode_bytes_with_errors(
    bytes: &[u8],
    kind: EncodingKind,
    errors: &str,
) -> Result<(Vec<u8>, String), (DecodeFailure, String)> {
    match kind {
        EncodingKind::Utf8 => match decode_utf8_bytes_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "utf-8".to_string())),
            Err(err) => Err((err, "utf-8".to_string())),
        },
        EncodingKind::Utf8Sig => {
            let data =
                if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
                    &bytes[3..]
                } else {
                    bytes
                };
            match decode_utf8_bytes_with_errors(data, errors) {
                Ok(text) => Ok((text, "utf-8".to_string())),
                Err(err) => Err((err, "utf-8".to_string())),
            }
        }
        EncodingKind::Cp1252 => match decode_cp1252_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp437 => match decode_charmap_with_errors(bytes, errors, &CP437_DECODE_TABLE)
        {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp850 => match decode_charmap_with_errors(bytes, errors, &CP850_DECODE_TABLE)
        {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp860 => match decode_charmap_with_errors(bytes, errors, &CP860_DECODE_TABLE)
        {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp862 => match decode_charmap_with_errors(bytes, errors, &CP862_DECODE_TABLE)
        {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp863 => match decode_charmap_with_errors(bytes, errors, &CP863_DECODE_TABLE)
        {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp865 => match decode_charmap_with_errors(bytes, errors, &CP865_DECODE_TABLE)
        {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp866 => match decode_charmap_with_errors(bytes, errors, &CP866_DECODE_TABLE)
        {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp874 => match decode_charmap_with_errors(bytes, errors, &CP874_DECODE_TABLE)
        {
            Ok(text) => Ok((text, "charmap".to_string())),
            Err(err) => Err((err, "charmap".to_string())),
        },
        EncodingKind::Cp1250 => {
            match decode_charmap_with_errors(bytes, errors, &CP1250_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Cp1251 => {
            match decode_charmap_with_errors(bytes, errors, &CP1251_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Cp1253 => {
            match decode_charmap_with_errors(bytes, errors, &CP1253_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Cp1254 => {
            match decode_charmap_with_errors(bytes, errors, &CP1254_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Cp1255 => {
            match decode_charmap_with_errors(bytes, errors, &CP1255_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Cp1256 => {
            match decode_charmap_with_errors(bytes, errors, &CP1256_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Cp1257 => {
            match decode_charmap_with_errors(bytes, errors, &CP1257_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Koi8R => {
            match decode_charmap_with_errors(bytes, errors, &KOI8_R_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Koi8U => {
            match decode_charmap_with_errors(bytes, errors, &KOI8_U_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_2 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_2_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_3 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_3_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_4 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_4_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_5 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_5_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_6 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_6_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_7 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_7_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_8 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_8_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_10 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_10_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Iso8859_15 => {
            match decode_charmap_with_errors(bytes, errors, &ISO8859_15_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::MacRoman => {
            match decode_charmap_with_errors(bytes, errors, &MAC_ROMAN_DECODE_TABLE) {
                Ok(text) => Ok((text, "charmap".to_string())),
                Err(err) => Err((err, "charmap".to_string())),
            }
        }
        EncodingKind::Ascii => match decode_ascii_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "ascii".to_string())),
            Err(err) => Err((err, "ascii".to_string())),
        },
        EncodingKind::Latin1 => {
            let mut out = Vec::with_capacity(bytes.len());
            for &byte in bytes {
                push_wtf8_codepoint(&mut out, byte as u32);
            }
            Ok((out, "latin-1".to_string()))
        }
        EncodingKind::UnicodeEscape => match decode_unicode_escape_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "unicodeescape".to_string())),
            Err(err) => Err((err, "unicodeescape".to_string())),
        },
        EncodingKind::Utf16 | EncodingKind::Utf16LE | EncodingKind::Utf16BE => {
            let (endian, label, offset) = utf16_decode_config(bytes, kind);
            match decode_utf16_with_errors(bytes, errors, endian, offset) {
                Ok(text) => Ok((text, label)),
                Err(err) => Err((err, label)),
            }
        }
        EncodingKind::Utf32 | EncodingKind::Utf32LE | EncodingKind::Utf32BE => {
            let (endian, label, offset) = utf32_decode_config(bytes, kind);
            match decode_utf32_with_errors(bytes, errors, endian, offset) {
                Ok(text) => Ok((text, label)),
                Err(err) => Err((err, label)),
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum DecodeTextError {
    UnknownEncoding(String),
    UnknownErrorHandler(String),
    Failure(DecodeFailure, String),
}

pub(crate) fn decode_bytes_text(
    encoding: &str,
    errors: &str,
    bytes: &[u8],
) -> Result<(Vec<u8>, String), DecodeTextError> {
    let Some(kind) = normalize_encoding(encoding) else {
        return Err(DecodeTextError::UnknownEncoding(encoding.to_string()));
    };
    let errors_known = matches!(
        errors,
        "strict" | "ignore" | "replace" | "backslashreplace" | "surrogateescape" | "surrogatepass"
    );
    let result = if errors_known {
        decode_bytes_with_errors(bytes, kind, errors)
    } else {
        match decode_bytes_with_errors(bytes, kind, "strict") {
            Ok((text, label)) => return Ok((text, label)),
            Err((_failure, _label)) => {
                return Err(DecodeTextError::UnknownErrorHandler(errors.to_string()));
            }
        }
    };
    match result {
        Ok((text, label)) => Ok((text, label)),
        Err((failure, label)) => Err(DecodeTextError::Failure(failure, label)),
    }
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
pub extern "C" fn molt_list_append(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(list_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_LIST {
                    let elems = seq_vec(ptr);
                    elems.push(val_bits);
                    inc_ref_bits(_py, val_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_pop(list_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(list_bits);
        let index_obj = obj_from_bits(index_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_LIST {
                    let len = list_len(ptr) as i64;
                    if len == 0 {
                        return raise_exception::<_>(_py, "IndexError", "pop from empty list");
                    }
                    let mut idx = if index_obj.is_none() {
                        len - 1
                    } else {
                        index_i64_from_obj(
                            _py,
                            index_bits,
                            "list indices must be integers or have an __index__ method",
                        )
                    };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if idx < 0 {
                        idx += len;
                    }
                    if idx < 0 || idx >= len {
                        return raise_exception::<_>(_py, "IndexError", "pop index out of range");
                    }
                    let elems = seq_vec(ptr);
                    let idx_usize = idx as usize;
                    let value = elems.remove(idx_usize);
                    inc_ref_bits(_py, value);
                    dec_ref_bits(_py, value);
                    return value;
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_extend(list_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(list_ptr) != TYPE_ID_LIST {
                    return MoltObject::none().bits();
                }
                let list_elems = seq_vec(list_ptr);
                let other_obj = obj_from_bits(other_bits);
                if let Some(other_ptr) = other_obj.as_ptr() {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_LIST || other_type == TYPE_ID_TUPLE {
                        if other_ptr == list_ptr {
                            let snapshot = seq_vec_ref(other_ptr).clone();
                            for item in snapshot {
                                list_elems.push(item);
                                inc_ref_bits(_py, item);
                            }
                        } else {
                            let src = seq_vec_ref(other_ptr);
                            for &item in src.iter() {
                                list_elems.push(item);
                                inc_ref_bits(_py, item);
                            }
                        }
                        return MoltObject::none().bits();
                    }
                    if other_type == TYPE_ID_DICT {
                        let order = dict_order(other_ptr);
                        for idx in (0..order.len()).step_by(2) {
                            let key_bits = order[idx];
                            list_elems.push(key_bits);
                            inc_ref_bits(_py, key_bits);
                        }
                        return MoltObject::none().bits();
                    }
                    if other_type == TYPE_ID_DICT_KEYS_VIEW
                        || other_type == TYPE_ID_DICT_VALUES_VIEW
                        || other_type == TYPE_ID_DICT_ITEMS_VIEW
                    {
                        let len = dict_view_len(other_ptr);
                        for idx in 0..len {
                            if let Some((key_bits, val_bits)) = dict_view_entry(other_ptr, idx) {
                                if other_type == TYPE_ID_DICT_ITEMS_VIEW {
                                    let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                                    if tuple_ptr.is_null() {
                                        return MoltObject::none().bits();
                                    }
                                    list_elems.push(MoltObject::from_ptr(tuple_ptr).bits());
                                } else {
                                    let item = if other_type == TYPE_ID_DICT_KEYS_VIEW {
                                        key_bits
                                    } else {
                                        val_bits
                                    };
                                    list_elems.push(item);
                                    inc_ref_bits(_py, item);
                                }
                            }
                        }
                        return MoltObject::none().bits();
                    }
                }
                let iter_bits = molt_iter(other_bits);
                if obj_from_bits(iter_bits).is_none() {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_not_iterable(_py, other_bits);
                }
                loop {
                    let pair_bits = molt_iter_next(iter_bits);
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        return MoltObject::none().bits();
                    }
                    let pair_elems = seq_vec_ref(pair_ptr);
                    if pair_elems.len() < 2 {
                        return MoltObject::none().bits();
                    }
                    let done_bits = pair_elems[1];
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        break;
                    }
                    let val_bits = pair_elems[0];
                    list_elems.push(val_bits);
                    inc_ref_bits(_py, val_bits);
                }
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_insert(list_bits: u64, index_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    let len = list_len(list_ptr) as i64;
                    let mut idx = index_i64_from_obj(
                        _py,
                        index_bits,
                        "list indices must be integers or have an __index__ method",
                    );
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if idx < 0 {
                        idx += len;
                    }
                    if idx < 0 {
                        idx = 0;
                    }
                    if idx > len {
                        idx = len;
                    }
                    let elems = seq_vec(list_ptr);
                    elems.insert(idx as usize, val_bits);
                    inc_ref_bits(_py, val_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

unsafe fn list_snapshot(_py: &PyToken<'_>, list_ptr: *mut u8) -> Vec<u64> {
    unsafe {
        let elems = seq_vec_ref(list_ptr);
        let mut out = Vec::with_capacity(elems.len());
        for &elem in elems.iter() {
            inc_ref_bits(_py, elem);
            out.push(elem);
        }
        out
    }
}

unsafe fn list_snapshot_release(_py: &PyToken<'_>, snapshot: Vec<u64>) {
    for elem in snapshot {
        dec_ref_bits(_py, elem);
    }
}

unsafe fn list_elem_at(list_ptr: *mut u8, idx: usize) -> Option<u64> {
    unsafe {
        let elems = seq_vec_ref(list_ptr);
        elems.get(idx).copied()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_remove(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    let snapshot = list_snapshot(_py, list_ptr);
                    let mut matched_idx = None;
                    for (idx, &elem_bits) in snapshot.iter().enumerate() {
                        let eq = match eq_bool_from_bits(_py, elem_bits, val_bits) {
                            Some(val) => val,
                            None => {
                                list_snapshot_release(_py, snapshot);
                                return MoltObject::none().bits();
                            }
                        };
                        if eq {
                            matched_idx = Some(idx);
                            break;
                        }
                    }
                    list_snapshot_release(_py, snapshot);
                    if let Some(target_idx) = matched_idx {
                        let elems = seq_vec(list_ptr);
                        if target_idx < elems.len() {
                            let removed = elems.remove(target_idx);
                            dec_ref_bits(_py, removed);
                            return MoltObject::none().bits();
                        }
                    }
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "list.remove(x): x not in list",
                    );
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_clear(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    let elems = seq_vec(list_ptr);
                    for &elem in elems.iter() {
                        dec_ref_bits(_py, elem);
                    }
                    elems.clear();
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_init_method(list_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "list.__init__ expects list");
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return raise_exception::<_>(_py, "TypeError", "list.__init__ expects list");
            }
        }
        let _ = molt_list_clear(list_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if iterable_bits == missing_bits(_py) {
            return MoltObject::none().bits();
        }
        let _ = molt_list_extend(list_bits, iterable_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_copy(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    let elems = seq_vec_ref(list_ptr);
                    let out_ptr = alloc_list(_py, elems.as_slice());
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_reverse(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    let elems = seq_vec(list_ptr);
                    elems.reverse();
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_sort(list_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(list_ptr) != TYPE_ID_LIST {
                    return MoltObject::none().bits();
                }
                let use_key = !obj_from_bits(key_bits).is_none();
                let reverse = is_truthy(_py, obj_from_bits(reverse_bits));
                let elems = seq_vec_ref(list_ptr);
                let mut items: Vec<SortItem> = Vec::with_capacity(elems.len());
                for &val_bits in elems.iter() {
                    let key_val_bits = if use_key {
                        let res_bits = call_callable1(_py, key_bits, val_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, res_bits);
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
                let mut new_elems: Vec<u64> = Vec::with_capacity(items.len());
                for item in items.iter() {
                    new_elems.push(item.value_bits);
                }
                if use_key {
                    for item in items.drain(..) {
                        dec_ref_bits(_py, item.key_bits);
                    }
                }
                let elems_mut = seq_vec(list_ptr);
                *elems_mut = new_elems;
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_add_method(list_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "list.__add__ expects list");
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return raise_exception::<_>(_py, "TypeError", "list.__add__ expects list");
            }
            let other_obj = obj_from_bits(other_bits);
            let Some(other_ptr) = other_obj.as_ptr() else {
                let msg = format!(
                    "can only concatenate list (not \"{}\") to list",
                    type_name(_py, other_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            if object_type_id(other_ptr) != TYPE_ID_LIST {
                let msg = format!(
                    "can only concatenate list (not \"{}\") to list",
                    type_name(_py, other_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let l_len = list_len(list_ptr);
            let r_len = list_len(other_ptr);
            let l_elems = seq_vec_ref(list_ptr);
            let r_elems = seq_vec_ref(other_ptr);
            let mut combined = Vec::with_capacity(l_len + r_len);
            combined.extend_from_slice(l_elems);
            combined.extend_from_slice(r_elems);
            let ptr = alloc_list(_py, &combined);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_mul_method(list_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "list.__mul__ expects list");
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return raise_exception::<_>(_py, "TypeError", "list.__mul__ expects list");
            }
        }
        let rhs_type = type_name(_py, obj_from_bits(count_bits));
        let msg = format!("can't multiply sequence by non-int of type '{rhs_type}'");
        let count = index_i64_from_obj(_py, count_bits, &msg);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(bits) = repeat_sequence(_py, list_ptr, count) else {
            return MoltObject::none().bits();
        };
        bits
    })
}


// heapq operations moved to ops_heapq.rs


fn bisect_len_from_obj(_py: &PyToken<'_>, obj: MoltObject) -> Option<i64> {
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__len__") {
                let call_bits = attr_lookup_ptr(_py, ptr, name_bits);
                dec_ref_bits(_py, name_bits);
                if let Some(call_bits) = call_bits {
                    let res_bits = call_callable0(_py, call_bits);
                    dec_ref_bits(_py, call_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    let res_obj = obj_from_bits(res_bits);
                    if let Some(i) = to_i64(res_obj) {
                        if i < 0 {
                            raise_exception::<()>(
                                _py,
                                "ValueError",
                                "__len__() should return >= 0",
                            );
                            return None;
                        }
                        return Some(i);
                    }
                    if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                        let big = bigint_ref(big_ptr);
                        if big.is_negative() {
                            raise_exception::<()>(
                                _py,
                                "ValueError",
                                "__len__() should return >= 0",
                            );
                            return None;
                        }
                        let Some(len) = big.to_usize() else {
                            raise_exception::<()>(
                                _py,
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer",
                            );
                            return None;
                        };
                        if len > i64::MAX as usize {
                            raise_exception::<()>(
                                _py,
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer",
                            );
                            return None;
                        }
                        return Some(len as i64);
                    }
                    let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                    let msg = format!("'{}' object cannot be interpreted as an integer", res_type);
                    raise_exception::<()>(_py, "TypeError", &msg);
                    return None;
                }
            }
        }
    }
    let type_name = class_name_for_error(type_of_bits(_py, obj.bits()));
    let msg = format!("object of type '{type_name}' has no len()");
    raise_exception::<()>(_py, "TypeError", &msg);
    None
}

fn bisect_item_at(_py: &PyToken<'_>, seq: MoltObject, idx: i64) -> Option<(u64, bool)> {
    if let Some(ptr) = seq.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_LIST {
                if idx < 0 {
                    raise_exception::<()>(_py, "IndexError", "list index out of range");
                    return None;
                }
                let len = list_len(ptr) as i64;
                if idx >= len {
                    raise_exception::<()>(_py, "IndexError", "list index out of range");
                    return None;
                }
                let elems = seq_vec_ref(ptr);
                return Some((elems[idx as usize], false));
            }
            if type_id == TYPE_ID_TUPLE {
                if idx < 0 {
                    raise_exception::<()>(_py, "IndexError", "tuple index out of range");
                    return None;
                }
                let elems = seq_vec_ref(ptr);
                if idx as usize >= elems.len() {
                    raise_exception::<()>(_py, "IndexError", "tuple index out of range");
                    return None;
                }
                return Some((elems[idx as usize], false));
            }
            if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") {
                if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                    dec_ref_bits(_py, name_bits);
                    let idx_bits = int_bits_from_i64(_py, idx);
                    let res_bits = call_callable1(_py, call_bits, idx_bits);
                    dec_ref_bits(_py, call_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    return Some((res_bits, true));
                }
                dec_ref_bits(_py, name_bits);
            }
            let msg = format!("'{}' object is not subscriptable", type_name(_py, seq));
            raise_exception::<()>(_py, "TypeError", &msg);
            return None;
        }
    }
    let msg = format!("'{}' object is not subscriptable", type_name(_py, seq));
    raise_exception::<()>(_py, "TypeError", &msg);
    None
}

fn bisect_lt_bool(_py: &PyToken<'_>, lhs_bits: u64, rhs_bits: u64) -> Option<bool> {
    let lhs = obj_from_bits(lhs_bits);
    let rhs = obj_from_bits(rhs_bits);
    match compare_builtin_bool(_py, lhs, rhs, CompareOp::Lt) {
        CompareBoolOutcome::True => return Some(true),
        CompareBoolOutcome::False => return Some(false),
        CompareBoolOutcome::Error => return None,
        CompareBoolOutcome::NotComparable => {}
    }
    let lt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__");
    let gt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__");
    match rich_compare_bool(_py, lhs, rhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => Some(true),
        CompareBoolOutcome::False => Some(false),
        CompareBoolOutcome::Error => None,
        CompareBoolOutcome::NotComparable => {
            compare_type_error(_py, lhs, rhs, "<");
            None
        }
    }
}

fn bisect_key_value(_py: &PyToken<'_>, key_bits: u64, item_bits: u64) -> Option<(u64, bool)> {
    let key_obj = obj_from_bits(key_bits);
    if key_obj.is_none() {
        return Some((item_bits, false));
    }
    let res_bits = unsafe { call_callable1(_py, key_bits, item_bits) };
    if exception_pending(_py) {
        return None;
    }
    Some((res_bits, true))
}

fn bisect_search_index(
    _py: &PyToken<'_>,
    seq_bits: u64,
    x_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
    key_bits: u64,
    left: bool,
) -> Option<i64> {
    let seq = obj_from_bits(seq_bits);
    let len = bisect_len_from_obj(_py, seq)?;
    let idx_err = format!(
        "'{}' object cannot be interpreted as an integer",
        type_name(_py, obj_from_bits(lo_bits))
    );
    let mut lo = index_i64_from_obj(_py, lo_bits, &idx_err);
    if exception_pending(_py) {
        return None;
    }
    if lo < 0 {
        raise_exception::<()>(_py, "ValueError", "lo must be non-negative");
        return None;
    }
    let mut hi = if obj_from_bits(hi_bits).is_none() {
        len
    } else {
        let hi_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(hi_bits))
        );
        index_i64_from_obj(_py, hi_bits, &hi_err)
    };
    if exception_pending(_py) {
        return None;
    }
    while lo < hi {
        let mid = lo + ((hi - lo) / 2);
        let (item_bits, item_owned) = bisect_item_at(_py, seq, mid)?;
        let Some((item_key_bits, key_owned)) = bisect_key_value(_py, key_bits, item_bits) else {
            if item_owned {
                dec_ref_bits(_py, item_bits);
            }
            return None;
        };
        let cmp = if left {
            bisect_lt_bool(_py, item_key_bits, x_bits)
        } else {
            bisect_lt_bool(_py, x_bits, item_key_bits)
        };
        if key_owned {
            dec_ref_bits(_py, item_key_bits);
        }
        if item_owned {
            dec_ref_bits(_py, item_bits);
        }
        let lt = cmp?;
        if lt {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    Some(lo)
}

fn bisect_insert(_py: &PyToken<'_>, seq_bits: u64, idx: i64, value_bits: u64) -> Option<()> {
    let seq = obj_from_bits(seq_bits);
    if let Some(ptr) = seq.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_LIST {
                let len = list_len(ptr) as i64;
                let mut pos = idx;
                if pos < 0 {
                    pos = 0;
                }
                if pos > len {
                    pos = len;
                }
                let elems = seq_vec(ptr);
                elems.insert(pos as usize, value_bits);
                inc_ref_bits(_py, value_bits);
                return Some(());
            }
            if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"insert") {
                if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                    dec_ref_bits(_py, name_bits);
                    let idx_bits = int_bits_from_i64(_py, idx);
                    let _ = call_callable2(_py, call_bits, idx_bits, value_bits);
                    dec_ref_bits(_py, call_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    return Some(());
                }
                dec_ref_bits(_py, name_bits);
            }
            let msg = format!("'{}' object has no attribute 'insert'", type_name(_py, seq));
            raise_exception::<()>(_py, "AttributeError", &msg);
            return None;
        }
    }
    let msg = format!("'{}' object has no attribute 'insert'", type_name(_py, seq));
    raise_exception::<()>(_py, "AttributeError", &msg);
    None
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_insort_left(
    seq_bits: u64,
    x_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
    key_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut x_key_bits = x_bits;
        let mut x_key_owned = false;
        if !obj_from_bits(key_bits).is_none() {
            let res_bits = unsafe { call_callable1(_py, key_bits, x_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            x_key_bits = res_bits;
            x_key_owned = true;
        }
        let pos = bisect_search_index(_py, seq_bits, x_key_bits, lo_bits, hi_bits, key_bits, true);
        if x_key_owned {
            dec_ref_bits(_py, x_key_bits);
        }
        let Some(pos) = pos else {
            return MoltObject::none().bits();
        };
        if bisect_insert(_py, seq_bits, pos, x_bits).is_none() {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_insort_right(
    seq_bits: u64,
    x_bits: u64,
    lo_bits: u64,
    hi_bits: u64,
    key_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut x_key_bits = x_bits;
        let mut x_key_owned = false;
        if !obj_from_bits(key_bits).is_none() {
            let res_bits = unsafe { call_callable1(_py, key_bits, x_bits) };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            x_key_bits = res_bits;
            x_key_owned = true;
        }
        let pos = bisect_search_index(_py, seq_bits, x_key_bits, lo_bits, hi_bits, key_bits, false);
        if x_key_owned {
            dec_ref_bits(_py, x_key_bits);
        }
        let Some(pos) = pos else {
            return MoltObject::none().bits();
        };
        if bisect_insert(_py, seq_bits, pos, x_bits).is_none() {
            return MoltObject::none().bits();
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_count(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_LIST {
                    let mut count = 0i64;
                    let mut idx = 0usize;
                    while let Some(val) = list_elem_at(ptr, idx) {
                        let elem_bits = val;
                        inc_ref_bits(_py, elem_bits);
                        let eq = match eq_bool_from_bits(_py, elem_bits, val_bits) {
                            Some(val) => val,
                            None => {
                                dec_ref_bits(_py, elem_bits);
                                return MoltObject::none().bits();
                            }
                        };
                        dec_ref_bits(_py, elem_bits);
                        if eq {
                            count += 1;
                        }
                        idx += 1;
                    }
                    return MoltObject::from_int(count).bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_index_range(
    list_bits: u64,
    val_bits: u64,
    start_bits: u64,
    stop_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_LIST {
                    let len = list_len(ptr) as i64;
                    let missing = missing_bits(_py);
                    let err = "slice indices must be integers or have an __index__ method";
                    let mut start = if start_bits == missing {
                        0
                    } else {
                        index_i64_from_obj(_py, start_bits, err)
                    };
                    let mut stop = if stop_bits == missing {
                        len
                    } else {
                        index_i64_from_obj(_py, stop_bits, err)
                    };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if start < 0 {
                        start += len;
                    }
                    if stop < 0 {
                        stop += len;
                    }
                    if start < 0 {
                        start = 0;
                    }
                    if stop < 0 {
                        stop = 0;
                    }
                    if start > len {
                        start = len;
                    }
                    if stop > len {
                        stop = len;
                    }
                    if start < stop {
                        let mut idx = start;
                        while idx < stop {
                            let elem_bits = match list_elem_at(ptr, idx as usize) {
                                Some(val) => val,
                                None => break,
                            };
                            inc_ref_bits(_py, elem_bits);
                            let eq = match eq_bool_from_bits(_py, elem_bits, val_bits) {
                                Some(val) => val,
                                None => {
                                    dec_ref_bits(_py, elem_bits);
                                    return MoltObject::none().bits();
                                }
                            };
                            dec_ref_bits(_py, elem_bits);
                            if eq {
                                return MoltObject::from_int(idx).bits();
                            }
                            idx += 1;
                        }
                    }
                    return raise_exception::<_>(_py, "ValueError", "list.index(x): x not in list");
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_index(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        molt_list_index_range(list_bits, val_bits, missing, missing)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_count(tuple_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let tuple_obj = obj_from_bits(tuple_bits);
        if let Some(ptr) = tuple_obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(ptr);
                    let mut count = 0i64;
                    for &elem in elems.iter() {
                        let eq = match eq_bool_from_bits(_py, elem, val_bits) {
                            Some(val) => val,
                            None => return MoltObject::none().bits(),
                        };
                        if eq {
                            count += 1;
                        }
                    }
                    return MoltObject::from_int(count).bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_index(tuple_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        molt_tuple_index_range(tuple_bits, val_bits, missing, missing)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_index_range(
    tuple_bits: u64,
    val_bits: u64,
    start_bits: u64,
    stop_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let tuple_obj = obj_from_bits(tuple_bits);
        if let Some(ptr) = tuple_obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(ptr);
                    let len = elems.len() as i64;
                    let mut start = if start_bits != missing {
                        index_i64_from_obj(
                            _py,
                            start_bits,
                            "slice indices must be integers or have an __index__ method",
                        )
                    } else {
                        0
                    };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let mut stop = if stop_bits != missing {
                        index_i64_from_obj(
                            _py,
                            stop_bits,
                            "slice indices must be integers or have an __index__ method",
                        )
                    } else {
                        len
                    };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if start < 0 {
                        start += len;
                    }
                    if stop < 0 {
                        stop += len;
                    }
                    if start < 0 {
                        start = 0;
                    }
                    if stop < 0 {
                        stop = 0;
                    }
                    if start > len {
                        start = len;
                    }
                    if stop > len {
                        stop = len;
                    }
                    let mut idx = start;
                    while idx < stop {
                        let eq = match eq_bool_from_bits(_py, elems[idx as usize], val_bits) {
                            Some(val) => val,
                            None => return MoltObject::none().bits(),
                        };
                        if eq {
                            return MoltObject::from_int(idx).bits();
                        }
                        idx += 1;
                    }
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "tuple.index(x): x not in tuple",
                    );
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_print_obj(val: u64) {
    crate::with_gil_entry!(_py, {
        let args_ptr = alloc_tuple(_py, &[val]);
        if args_ptr.is_null() {
            return;
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let none_bits = MoltObject::none().bits();
        let flush_bits = MoltObject::from_bool(true).bits();
        let res_bits = molt_print_builtin(args_bits, none_bits, none_bits, none_bits, flush_bits);
        dec_ref_bits(_py, res_bits);
        dec_ref_bits(_py, args_bits);
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_print_newline() {
    crate::with_gil_entry!(_py, {
        let args_ptr = alloc_tuple(_py, &[]);
        if args_ptr.is_null() {
            return;
        }
        let args_bits = MoltObject::from_ptr(args_ptr).bits();
        let none_bits = MoltObject::none().bits();
        let flush_bits = MoltObject::from_bool(true).bits();
        let res_bits = molt_print_builtin(args_bits, none_bits, none_bits, none_bits, flush_bits);
        dec_ref_bits(_py, res_bits);
        dec_ref_bits(_py, args_bits);
    })
}

fn format_float(f: f64) -> String {
    if f.is_nan() {
        return "nan".to_string();
    }
    if f.is_infinite() {
        if f.is_sign_negative() {
            return "-inf".to_string();
        }
        return "inf".to_string();
    }
    let abs = f.abs();
    if abs != 0.0 && !(1e-4..1e16).contains(&abs) {
        return format_float_scientific(f);
    }
    if f.fract() == 0.0 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

fn format_float_scientific(f: f64) -> String {
    let raw = f.to_string();
    if raw.contains('e') || raw.contains('E') {
        return normalize_scientific(&raw);
    }
    let mut digits = raw.as_str();
    if let Some(rest) = digits.strip_prefix('-') {
        digits = rest;
    }
    let digits_only: String = digits.chars().filter(|ch| *ch != '.').collect();
    let sig_digits = digits_only.trim_start_matches('0').len().max(1);
    let precision = sig_digits.saturating_sub(1).min(16);
    let formatted = format!("{:.*e}", precision, f);
    normalize_scientific(&formatted)
}

fn normalize_scientific(formatted: &str) -> String {
    let normalized = formatted.to_lowercase();
    let Some(exp_pos) = normalized.find('e') else {
        return normalized;
    };
    let (mantissa, exp) = normalized.split_at(exp_pos);
    let mut mant = mantissa.to_string();
    if mant.contains('.') {
        while mant.ends_with('0') {
            mant.pop();
        }
        if mant.ends_with('.') {
            mant.pop();
        }
    }
    let exp_val: i32 = exp[1..].parse().unwrap_or(0);
    let sign = if exp_val < 0 { "-" } else { "+" };
    let exp_abs = exp_val.unsigned_abs();
    let exp_text = format!("{exp_abs:02}");
    format!("{mant}e{sign}{exp_text}")
}

fn format_complex_float(f: f64) -> String {
    let text = format_float(f);
    if let Some(stripped) = text.strip_suffix(".0") {
        stripped.to_string()
    } else {
        text
    }
}

fn format_complex(re: f64, im: f64) -> String {
    let re_zero = re == 0.0 && !re.is_sign_negative();
    let re_text = format_complex_float(re);
    if re_zero {
        let im_text = format_complex_float(im);
        return format!("{im_text}j");
    }
    let sign = if im.is_sign_negative() { "-" } else { "+" };
    let im_text = format_complex_float(im.abs());
    format!("({re_text}{sign}{im_text}j)")
}

fn format_range(start: &BigInt, stop: &BigInt, step: &BigInt) -> String {
    if step == &BigInt::from(1) {
        format!("range({start}, {stop})")
    } else {
        format!("range({start}, {stop}, {step})")
    }
}

fn format_slice(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let start = format_obj(_py, obj_from_bits(slice_start_bits(ptr)));
        let stop = format_obj(_py, obj_from_bits(slice_stop_bits(ptr)));
        let step = format_obj(_py, obj_from_bits(slice_step_bits(ptr)));
        format!("slice({start}, {stop}, {step})")
    }
}

fn format_type_name_for_alias(_py: &PyToken<'_>, type_ptr: *mut u8) -> Option<String> {
    unsafe {
        let name =
            string_obj_to_owned(obj_from_bits(class_name_bits(type_ptr))).unwrap_or_default();
        if name.is_empty() {
            return None;
        }
        let mut qualname = name;
        let mut module_name: Option<String> = None;
        if !exception_pending(_py) {
            let dict_bits = class_dict_bits(type_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                if let Some(module_key) = attr_name_bits_from_bytes(_py, b"__module__")
                    && let Some(bits) = dict_get_in_place(_py, dict_ptr, module_key)
                    && let Some(val) = string_obj_to_owned(obj_from_bits(bits))
                {
                    module_name = Some(val);
                }
                if let Some(qual_key) = attr_name_bits_from_bytes(_py, b"__qualname__")
                    && let Some(bits) = dict_get_in_place(_py, dict_ptr, qual_key)
                    && let Some(val) = string_obj_to_owned(obj_from_bits(bits))
                {
                    qualname = val;
                }
            }
        }
        if let Some(module) = module_name
            && !module.is_empty()
            && module != "builtins"
        {
            return Some(format!("{module}.{qualname}"));
        }
        Some(qualname)
    }
}

fn format_generic_alias(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let origin_bits = generic_alias_origin_bits(ptr);
        let args_bits = generic_alias_args_bits(ptr);
        let origin_obj = obj_from_bits(origin_bits);
        let render_arg = |arg_bits: u64| {
            let arg_obj = obj_from_bits(arg_bits);
            if let Some(arg_ptr) = arg_obj.as_ptr()
                && object_type_id(arg_ptr) == TYPE_ID_TYPE
                && let Some(name) = format_type_name_for_alias(_py, arg_ptr)
            {
                return name;
            }
            format_obj(_py, arg_obj)
        };
        let origin_repr = if let Some(origin_ptr) = origin_obj.as_ptr() {
            if object_type_id(origin_ptr) == TYPE_ID_TYPE {
                format_type_name_for_alias(_py, origin_ptr)
                    .unwrap_or_else(|| format_obj(_py, origin_obj))
            } else {
                format_obj(_py, origin_obj)
            }
        } else {
            format_obj(_py, origin_obj)
        };
        let mut out = String::new();
        out.push_str(&origin_repr);
        out.push('[');
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr() {
            if object_type_id(args_ptr) == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(args_ptr);
                for (idx, elem_bits) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&render_arg(*elem_bits));
                }
            } else {
                out.push_str(&render_arg(args_bits));
            }
        } else {
            out.push_str(&render_arg(args_bits));
        }
        out.push(']');
        out
    }
}

fn format_union_type(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let args_bits = union_type_args_bits(ptr);
        let render_arg = |arg_bits: u64| {
            let arg_obj = obj_from_bits(arg_bits);
            if let Some(arg_ptr) = arg_obj.as_ptr()
                && object_type_id(arg_ptr) == TYPE_ID_TYPE
                && let Some(name) = format_type_name_for_alias(_py, arg_ptr)
            {
                return name;
            }
            format_obj(_py, arg_obj)
        };
        let mut out = String::new();
        let args_obj = obj_from_bits(args_bits);
        if let Some(args_ptr) = args_obj.as_ptr()
            && object_type_id(args_ptr) == TYPE_ID_TUPLE
        {
            let elems = seq_vec_ref(args_ptr);
            for (idx, elem_bits) in elems.iter().enumerate() {
                if idx > 0 {
                    out.push_str(" | ");
                }
                out.push_str(&render_arg(*elem_bits));
            }
            return out;
        }
        out.push_str(&render_arg(args_bits));
        out
    }
}

pub(crate) fn string_obj_to_owned(obj: MoltObject) -> Option<String> {
    let ptr = obj.as_ptr()?;
    unsafe {
        if object_type_id(ptr) != TYPE_ID_STRING {
            return None;
        }
        let len = string_len(ptr);
        let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
        Some(String::from_utf8_lossy(bytes).to_string())
    }
}

pub(crate) fn decode_string_list(obj: MoltObject) -> Option<Vec<String>> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        let mut out = Vec::with_capacity(elems.len());
        for &elem_bits in elems.iter() {
            let elem_obj = obj_from_bits(elem_bits);
            let s = string_obj_to_owned(elem_obj)?;
            out.push(s);
        }
        Some(out)
    }
}

pub(crate) fn decode_value_list(obj: MoltObject) -> Option<Vec<u64>> {
    let ptr = obj.as_ptr()?;
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
            return None;
        }
        let elems = seq_vec_ref(ptr);
        Some(elems.to_vec())
    }
}

fn format_dataclass(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let desc_ptr = dataclass_desc_ptr(ptr);
        if desc_ptr.is_null() {
            return "<dataclass>".to_string();
        }
        let desc = &*desc_ptr;
        let fields = dataclass_fields_ref(ptr);
        let mut out = String::new();
        out.push_str(&desc.name);
        out.push('(');
        let mut first = true;
        for (idx, name) in desc.field_names.iter().enumerate() {
            let flag = desc.field_flags.get(idx).copied().unwrap_or(0x7);
            if (flag & 0x1) == 0 {
                continue;
            }
            if !first {
                out.push_str(", ");
            }
            first = false;
            out.push_str(name);
            out.push('=');
            let val = fields
                .get(idx)
                .copied()
                .unwrap_or(MoltObject::none().bits());
            if is_missing_bits(_py, val) {
                let type_label = if desc.name.is_empty() {
                    "dataclass"
                } else {
                    desc.name.as_str()
                };
                let _ = attr_error(_py, type_label, name);
                return "<dataclass>".to_string();
            }
            out.push_str(&format_obj(_py, obj_from_bits(val)));
        }
        out.push(')');
        out
    }
}

struct ReprGuard {
    ptr: *mut u8,
    active: bool,
    depth_active: bool,
}

impl ReprGuard {
    fn new(_py: &PyToken<'_>, ptr: *mut u8) -> Self {
        if !repr_depth_enter() {
            let _ = raise_exception::<u64>(
                _py,
                "RecursionError",
                "maximum recursion depth exceeded while getting the repr of an object",
            );
            return Self {
                ptr,
                active: false,
                depth_active: false,
            };
        }
        let active = REPR_STACK.with(|stack| {
            REPR_SET.with(|set| {
                let mut set = set.borrow_mut();
                let slot = PtrSlot(ptr);
                if !set.insert(slot) {
                    return false;
                }
                stack.borrow_mut().push(slot);
                true
            })
        });
        if !active {
            repr_depth_exit();
        }
        Self {
            ptr,
            active,
            depth_active: active,
        }
    }

    fn active(&self) -> bool {
        self.active
    }
}

impl Drop for ReprGuard {
    fn drop(&mut self) {
        if self.active {
            REPR_SET.with(|set| {
                set.borrow_mut().remove(&PtrSlot(self.ptr));
            });
            REPR_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                if stack.last().is_some_and(|slot| slot.0 == self.ptr) {
                    stack.pop();
                } else if let Some(pos) = stack.iter().rposition(|slot| slot.0 == self.ptr) {
                    stack.remove(pos);
                }
            });
        }
        if self.depth_active {
            repr_depth_exit();
        }
    }
}

fn repr_depth_enter() -> bool {
    let limit = recursion_limit_get();
    REPR_DEPTH.with(|depth| {
        let current = depth.get();
        if current + 1 > limit {
            false
        } else {
            depth.set(current + 1);
            true
        }
    })
}

fn repr_depth_exit() {
    REPR_DEPTH.with(|depth| {
        let current = depth.get();
        if current > 0 {
            depth.set(current - 1);
        }
    });
}

fn format_default_object_repr(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    let class_bits = unsafe {
        if object_type_id(ptr) == TYPE_ID_OBJECT || object_type_id(ptr) == TYPE_ID_DATACLASS {
            object_class_bits(ptr)
        } else {
            type_of_bits(_py, MoltObject::from_ptr(ptr).bits())
        }
    };
    let class_name = class_name_for_error(class_bits);
    // Look up __module__ on the class to produce CPython-style qualified repr.
    let class_obj = obj_from_bits(class_bits);
    if let Some(class_ptr) = class_obj.as_ptr() {
        unsafe {
            if object_type_id(class_ptr) == TYPE_ID_TYPE && !exception_pending(_py) {
                let dict_bits = class_dict_bits(class_ptr);
                if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                    && let Some(module_key) = attr_name_bits_from_bytes(_py, b"__module__")
                    && let Some(bits) = dict_get_in_place(_py, dict_ptr, module_key)
                    && let Some(module) = string_obj_to_owned(obj_from_bits(bits))
                    && !module.is_empty()
                    && module != "builtins"
                {
                    let mut qualname = class_name.clone();
                    if let Some(qual_key) = attr_name_bits_from_bytes(_py, b"__qualname__")
                        && let Some(qbits) = dict_get_in_place(_py, dict_ptr, qual_key)
                        && let Some(val) = string_obj_to_owned(obj_from_bits(qbits))
                    {
                        qualname = val;
                    }
                    return format!("<{module}.{qualname} object at 0x{:x}>", ptr as usize);
                }
            }
        }
    }
    format!("<{class_name} object at 0x{:x}>", ptr as usize)
}

fn call_bits_is_default_object_repr(call_bits: u64) -> bool {
    let call_obj = obj_from_bits(call_bits);
    let Some(mut call_ptr) = call_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(call_ptr) == TYPE_ID_BOUND_METHOD {
            let func_bits = bound_method_func_bits(call_ptr);
            let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
                return false;
            };
            call_ptr = func_ptr;
        }
        object_type_id(call_ptr) == TYPE_ID_FUNCTION
            && function_fn_ptr(call_ptr) == fn_addr!(molt_repr_from_obj)
    }
}

fn call_bits_is_default_object_str(call_bits: u64) -> bool {
    let call_obj = obj_from_bits(call_bits);
    let Some(mut call_ptr) = call_obj.as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(call_ptr) == TYPE_ID_BOUND_METHOD {
            let func_bits = bound_method_func_bits(call_ptr);
            let Some(func_ptr) = obj_from_bits(func_bits).as_ptr() else {
                return false;
            };
            call_ptr = func_ptr;
        }
        object_type_id(call_ptr) == TYPE_ID_FUNCTION
            && function_fn_ptr(call_ptr) == fn_addr!(molt_str_from_obj)
    }
}

pub(crate) fn format_obj_str(_py: &PyToken<'_>, obj: MoltObject) -> String {
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TYPE {
                return format_obj(_py, obj);
            }
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return String::from_utf8_lossy(bytes).into_owned();
            }
            if type_id == TYPE_ID_EXCEPTION {
                return format_exception_message(_py, ptr);
            }
            let str_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.str_name, b"__str__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, str_name_bits) {
                if call_bits_is_default_object_str(call_bits) {
                    dec_ref_bits(_py, call_bits);
                    // CPython's default object.__str__ delegates to __repr__;
                    // preserve that path so custom __repr__ methods render correctly.
                    return format_obj(_py, obj);
                }
                if call_bits_is_default_object_repr(call_bits) {
                    dec_ref_bits(_py, call_bits);
                    // object.__str__ delegates to repr; use format_obj so custom
                    // __repr__ overrides participate instead of forcing default
                    // pointer-style formatting.
                    return format_obj(_py, obj);
                }
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(rendered) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(_py, res_bits);
                    return rendered;
                }
                dec_ref_bits(_py, res_bits);
            }
            if exception_pending(_py) {
                return "<object>".to_string();
            }
        }
    }
    format_obj(_py, obj)
}

pub(crate) fn format_obj(_py: &PyToken<'_>, obj: MoltObject) -> String {
    if let Some(b) = obj.as_bool() {
        return if b {
            "True".to_string()
        } else {
            "False".to_string()
        };
    }
    if let Some(i) = obj.as_int() {
        return i.to_string();
    }
    // NaN-boxing: raw 0x0 is IEEE 754 +0.0.  Previous code treated it
    // as int 0 because Cranelift zero-inits variables to 0x0, but that
    // broke float parity (e.g. math.sin(0) displayed "0" not "0.0").
    // Proper int 0 is MoltObject::from_int(0) (0x7ff9_0000_0000_0000).
    if let Some(f) = obj.as_float() {
        return format_float(f);
    }
    if obj.is_none() {
        return "None".to_string();
    }
    if obj.is_pending() {
        return "<pending>".to_string();
    }
    if obj.bits() == ellipsis_bits(_py) {
        return "Ellipsis".to_string();
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                return format_string_repr_bytes(bytes);
            }
            if type_id == TYPE_ID_BIGINT {
                return bigint_ref(ptr).to_string();
            }
            if type_id == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return format_complex(value.re, value.im);
            }
            if type_id == TYPE_ID_BYTES {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return format_bytes(bytes);
            }
            if type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return format!("bytearray({})", format_bytes(bytes));
            }
            if type_id == TYPE_ID_RANGE {
                if let Some((start, stop, step)) = range_components_bigint(ptr) {
                    return format_range(&start, &stop, &step);
                }
                return "range(?)".to_string();
            }
            if type_id == TYPE_ID_SLICE {
                return format_slice(_py, ptr);
            }
            if type_id == TYPE_ID_GENERIC_ALIAS {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "...".to_string();
                }
                return format_generic_alias(_py, ptr);
            }
            if type_id == TYPE_ID_UNION {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "...".to_string();
                }
                return format_union_type(_py, ptr);
            }
            if type_id == TYPE_ID_NOT_IMPLEMENTED {
                return "NotImplemented".to_string();
            }
            if type_id == TYPE_ID_ELLIPSIS {
                return "Ellipsis".to_string();
            }
            if type_id == TYPE_ID_EXCEPTION {
                return format_exception(_py, ptr);
            }
            if type_id == TYPE_ID_CONTEXT_MANAGER {
                return "<context_manager>".to_string();
            }
            if type_id == TYPE_ID_FILE_HANDLE {
                return "<file_handle>".to_string();
            }
            if type_id == TYPE_ID_FUNCTION {
                return "<function>".to_string();
            }
            if type_id == TYPE_ID_CODE {
                let name =
                    string_obj_to_owned(obj_from_bits(code_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<code>".to_string();
                }
                return format!("<code {name}>");
            }
            if type_id == TYPE_ID_BOUND_METHOD {
                return "<bound_method>".to_string();
            }
            if type_id == TYPE_ID_GENERATOR {
                return "<generator>".to_string();
            }
            if type_id == TYPE_ID_ASYNC_GENERATOR {
                return "<async_generator>".to_string();
            }
            if type_id == TYPE_ID_MODULE {
                let name =
                    string_obj_to_owned(obj_from_bits(module_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<module>".to_string();
                }
                return format!("<module '{name}'>");
            }
            if type_id == TYPE_ID_TYPE {
                let name =
                    string_obj_to_owned(obj_from_bits(class_name_bits(ptr))).unwrap_or_default();
                if name.is_empty() {
                    return "<type>".to_string();
                }
                let mut qualname = name.clone();
                let mut module_name: Option<String> = None;
                if !exception_pending(_py) {
                    let dict_bits = class_dict_bits(ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        if let Some(module_key) = attr_name_bits_from_bytes(_py, b"__module__")
                            && let Some(bits) = dict_get_in_place(_py, dict_ptr, module_key)
                            && let Some(val) = string_obj_to_owned(obj_from_bits(bits))
                        {
                            module_name = Some(val);
                        }
                        if let Some(qual_key) = attr_name_bits_from_bytes(_py, b"__qualname__")
                            && let Some(bits) = dict_get_in_place(_py, dict_ptr, qual_key)
                            && let Some(val) = string_obj_to_owned(obj_from_bits(bits))
                        {
                            qualname = val;
                        }
                    }
                }
                if let Some(module) = module_name
                    && !module.is_empty()
                    && module != "builtins"
                {
                    return format!("<class '{module}.{qualname}'>");
                }
                return format!("<class '{qualname}'>");
            }
            if type_id == TYPE_ID_CLASSMETHOD {
                return "<classmethod>".to_string();
            }
            if type_id == TYPE_ID_STATICMETHOD {
                return "<staticmethod>".to_string();
            }
            if type_id == TYPE_ID_PROPERTY {
                return "<property>".to_string();
            }
            if type_id == TYPE_ID_SUPER {
                return "<super>".to_string();
            }
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() && (*desc_ptr).repr {
                    return format_dataclass(_py, ptr);
                }
            }
            if type_id == TYPE_ID_BUFFER2D {
                let buf_ptr = buffer2d_ptr(ptr);
                if buf_ptr.is_null() {
                    return "<buffer2d>".to_string();
                }
                let buf = &*buf_ptr;
                return format!("<buffer2d {}x{}>", buf.rows, buf.cols);
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                let len = memoryview_len(ptr);
                let stride = memoryview_stride(ptr);
                let readonly = memoryview_readonly(ptr);
                return format!("<memoryview len={len} stride={stride} readonly={readonly}>");
            }
            if type_id == TYPE_ID_INTARRAY {
                let elems = intarray_slice(ptr);
                let mut out = String::from("intarray([");
                for (idx, val) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&val.to_string());
                }
                out.push_str("])");
                return out;
            }
            if type_id == TYPE_ID_LIST {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "[...]".to_string();
                }
                let elems = seq_vec_ref(ptr);
                let mut out = String::from("[");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(_py, obj_from_bits(*elem)));
                }
                out.push(']');
                return out;
            }
            if type_id == TYPE_ID_TUPLE {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "(...)".to_string();
                }
                let elems = seq_vec_ref(ptr);
                let mut out = String::from("(");
                for (idx, elem) in elems.iter().enumerate() {
                    if idx > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format_obj(_py, obj_from_bits(*elem)));
                }
                if elems.len() == 1 {
                    out.push(',');
                }
                out.push(')');
                return out;
            }
            if type_id == TYPE_ID_DICT {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "{...}".to_string();
                }
                let pairs = dict_order(ptr);
                let mut out = String::from("{");
                let mut idx = 0;
                let mut first = true;
                while idx + 1 < pairs.len() {
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_str(&format_obj(_py, obj_from_bits(pairs[idx])));
                    out.push_str(": ");
                    out.push_str(&format_obj(_py, obj_from_bits(pairs[idx + 1])));
                    idx += 2;
                }
                out.push('}');
                return out;
            }
            if type_id == TYPE_ID_SET {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "{...}".to_string();
                }
                let order = set_order(ptr);
                if order.is_empty() {
                    return "set()".to_string();
                }
                let table = set_table(ptr);
                let mut out = String::from("{");
                let mut first = true;
                for &entry in table.iter() {
                    if entry == 0 {
                        continue;
                    }
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    let elem = order[entry - 1];
                    out.push_str(&format_obj(_py, obj_from_bits(elem)));
                }
                out.push('}');
                return out;
            }
            if type_id == TYPE_ID_FROZENSET {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return "frozenset({...})".to_string();
                }
                let order = set_order(ptr);
                if order.is_empty() {
                    return "frozenset()".to_string();
                }
                let table = set_table(ptr);
                let mut out = String::from("frozenset({");
                let mut first = true;
                for &entry in table.iter() {
                    if entry == 0 {
                        continue;
                    }
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    let elem = order[entry - 1];
                    out.push_str(&format_obj(_py, obj_from_bits(elem)));
                }
                out.push_str("})");
                return out;
            }
            if type_id == TYPE_ID_DICT_KEYS_VIEW
                || type_id == TYPE_ID_DICT_VALUES_VIEW
                || type_id == TYPE_ID_DICT_ITEMS_VIEW
            {
                let guard = ReprGuard::new(_py, ptr);
                if !guard.active() {
                    return if type_id == TYPE_ID_DICT_KEYS_VIEW {
                        "dict_keys(...)".to_string()
                    } else if type_id == TYPE_ID_DICT_VALUES_VIEW {
                        "dict_values(...)".to_string()
                    } else {
                        "dict_items(...)".to_string()
                    };
                }
                let dict_bits = dict_view_dict_bits(ptr);
                let dict_obj = obj_from_bits(dict_bits);
                if let Some(dict_ptr) = dict_obj.as_ptr()
                    && object_type_id(dict_ptr) == TYPE_ID_DICT
                {
                    let pairs = dict_order(dict_ptr);
                    let mut out = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                        String::from("dict_keys([")
                    } else if type_id == TYPE_ID_DICT_VALUES_VIEW {
                        String::from("dict_values([")
                    } else {
                        String::from("dict_items([")
                    };
                    let mut idx = 0;
                    let mut first = true;
                    while idx + 1 < pairs.len() {
                        if !first {
                            out.push_str(", ");
                        }
                        first = false;
                        if type_id == TYPE_ID_DICT_ITEMS_VIEW {
                            out.push('(');
                            out.push_str(&format_obj(_py, obj_from_bits(pairs[idx])));
                            out.push_str(", ");
                            out.push_str(&format_obj(_py, obj_from_bits(pairs[idx + 1])));
                            out.push(')');
                        } else {
                            let val = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                                pairs[idx]
                            } else {
                                pairs[idx + 1]
                            };
                            out.push_str(&format_obj(_py, obj_from_bits(val)));
                        }
                        idx += 2;
                    }
                    out.push_str("])");
                    return out;
                }
            }
            if type_id == TYPE_ID_ITER {
                return "<iter>".to_string();
            }
            let repr_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.repr_name, b"__repr__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, repr_name_bits) {
                if call_bits_is_default_object_repr(call_bits) {
                    dec_ref_bits(_py, call_bits);
                    return format_default_object_repr(_py, ptr);
                }
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                let res_obj = obj_from_bits(res_bits);
                if let Some(rendered) = string_obj_to_owned(res_obj) {
                    dec_ref_bits(_py, res_bits);
                    return rendered;
                }
                dec_ref_bits(_py, res_bits);
                return "<object>".to_string();
            }
            if exception_pending(_py) {
                return "<object>".to_string();
            }
        }
    }
    "<object>".to_string()
}

fn format_bytes(bytes: &[u8]) -> String {
    let mut out = String::from("b'");
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{:02x}", b)),
        }
    }
    out.push('\'');
    out
}

fn format_string_repr_bytes(bytes: &[u8]) -> String {
    let use_double = bytes.contains(&b'\'') && !bytes.contains(&b'"');
    let quote = if use_double { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for cp in wtf8_from_bytes(bytes).code_points() {
        let code = cp.to_u32();
        match code {
            0x5C => out.push_str("\\\\"),
            0x0A => out.push_str("\\n"),
            0x0D => out.push_str("\\r"),
            0x09 => out.push_str("\\t"),
            // U+2028/U+2029 are printable in CPython 3.12 repr — no escaping
            _ if code == quote as u32 => {
                out.push('\\');
                out.push(quote);
            }
            _ if is_surrogate(code) => {
                out.push_str(&format!("\\u{code:04x}"));
            }
            _ => {
                let ch = char::from_u32(code).unwrap_or('\u{FFFD}');
                if ch.is_control() {
                    out.push_str(&unicode_escape(ch));
                } else {
                    out.push(ch);
                }
            }
        }
    }
    out.push(quote);
    out
}

#[allow(dead_code)]
fn format_string_repr(s: &str) -> String {
    let use_double = s.contains('\'') && !s.contains('"');
    let quote = if use_double { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // U+2028/U+2029 are printable in CPython 3.12 repr — no escaping
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => {
                let code = c as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{:02x}", code));
                } else if code <= 0xffff {
                    out.push_str(&format!("\\u{:04x}", code));
                } else {
                    out.push_str(&format!("\\U{:08x}", code));
                }
            }
            _ => out.push(ch),
        }
    }
    out.push(quote);
    out
}

pub(crate) struct FormatSpec {
    fill: char,
    align: Option<char>,
    sign: Option<char>,
    alternate: bool,
    width: Option<usize>,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

pub(crate) type FormatError = (&'static str, Cow<'static, str>);

pub(crate) fn parse_format_spec(spec: &str) -> Result<FormatSpec, &'static str> {
    if spec.is_empty() {
        return Ok(FormatSpec {
            fill: ' ',
            align: None,
            sign: None,
            alternate: false,
            width: None,
            grouping: None,
            precision: None,
            ty: None,
        });
    }
    let mut chars = spec.chars().peekable();
    let mut fill = ' ';
    let mut align = None;
    let mut sign = None;
    let mut alternate = false;
    let mut grouping = None;
    let mut peeked = chars.clone();
    let first = peeked.next();
    let second = peeked.next();
    if let (Some(c1), Some(c2)) = (first, second) {
        if matches!(c2, '<' | '>' | '^' | '=') {
            fill = c1;
            align = Some(c2);
            chars.next();
            chars.next();
        } else if matches!(c1, '<' | '>' | '^' | '=') {
            align = Some(c1);
            chars.next();
        }
    } else if let Some(c1) = first
        && matches!(c1, '<' | '>' | '^' | '=')
    {
        align = Some(c1);
        chars.next();
    }

    if let Some(ch) = chars.peek().copied()
        && matches!(ch, '+' | '-' | ' ')
    {
        sign = Some(ch);
        chars.next();
    }

    if matches!(chars.peek(), Some('#')) {
        alternate = true;
        chars.next();
    }

    if align.is_none() && matches!(chars.peek(), Some('0')) {
        fill = '0';
        align = Some('=');
        chars.next();
    }

    let mut width_text = String::new();
    while let Some(ch) = chars.peek().copied() {
        if ch.is_ascii_digit() {
            width_text.push(ch);
            chars.next();
        } else {
            break;
        }
    }
    let width = if width_text.is_empty() {
        None
    } else {
        Some(
            width_text
                .parse::<usize>()
                .map_err(|_| "Invalid format width")?,
        )
    };

    if let Some(ch) = chars.peek().copied()
        && (ch == ',' || ch == '_')
    {
        grouping = Some(ch);
        chars.next();
    }

    let mut precision = None;
    if matches!(chars.peek(), Some('.')) {
        chars.next();
        let mut prec_text = String::new();
        while let Some(ch) = chars.peek().copied() {
            if ch.is_ascii_digit() {
                prec_text.push(ch);
                chars.next();
            } else {
                break;
            }
        }
        if prec_text.is_empty() {
            return Err("Invalid format precision");
        }
        precision = Some(
            prec_text
                .parse::<usize>()
                .map_err(|_| "Invalid format precision")?,
        );
    }

    let remaining: String = chars.collect();
    if remaining.len() > 1 {
        return Err("Invalid format spec");
    }
    let ty = if remaining.is_empty() {
        None
    } else {
        Some(remaining.chars().next().unwrap())
    };

    Ok(FormatSpec {
        fill,
        align,
        sign,
        alternate,
        width,
        grouping,
        precision,
        ty,
    })
}

fn apply_grouping(text: &str, group: usize, sep: char) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / group);
    for (count, ch) in text.chars().rev().enumerate() {
        if count > 0 && count.is_multiple_of(group) {
            out.push(sep);
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

fn apply_alignment(prefix: &str, body: &str, spec: &FormatSpec, default_align: char) -> String {
    let text = format!("{prefix}{body}");
    let width = match spec.width {
        Some(val) => val,
        None => return text,
    };
    let len = text.chars().count();
    if len >= width {
        return text;
    }
    let pad_len = width - len;
    let align = spec.align.unwrap_or(default_align);
    let fill = spec.fill;
    if align == '=' {
        let padding = fill.to_string().repeat(pad_len);
        return format!("{prefix}{padding}{body}");
    }
    let padding = fill.to_string().repeat(pad_len);
    match align {
        '<' => format!("{text}{padding}"),
        '>' => format!("{padding}{text}"),
        '^' => {
            let left = pad_len / 2;
            let right = pad_len - left;
            format!(
                "{}{}{}",
                fill.to_string().repeat(left),
                text,
                fill.to_string().repeat(right)
            )
        }
        _ => text,
    }
}

fn trim_float_trailing(text: &str, alternate: bool) -> String {
    if alternate {
        return text.to_string();
    }
    let exp_pos = text.find(['e', 'E']).unwrap_or(text.len());
    let (mantissa, exp) = text.split_at(exp_pos);
    let mut end = mantissa.len();
    if let Some(dot) = mantissa.find('.') {
        let bytes = mantissa.as_bytes();
        while end > dot + 1 && bytes[end - 1] == b'0' {
            end -= 1;
        }
        if end == dot + 1 {
            end = dot;
        }
    }
    let trimmed = &mantissa[..end];
    format!("{trimmed}{exp}")
}

fn normalize_exponent(text: &str, upper: bool) -> String {
    let (exp_pos, exp_char) = if let Some(pos) = text.find('e') {
        (pos, 'e')
    } else if let Some(pos) = text.find('E') {
        (pos, 'E')
    } else {
        return text.to_string();
    };
    let (mantissa, exp) = text.split_at(exp_pos);
    let mut exp_text = &exp[1..];
    let mut sign = '+';
    if let Some(first) = exp_text.chars().next()
        && (first == '+' || first == '-')
    {
        sign = first;
        exp_text = &exp_text[1..];
    }
    let digits = if exp_text.is_empty() { "0" } else { exp_text };
    let mut padded = String::from(digits);
    if padded.len() == 1 {
        padded.insert(0, '0');
    }
    let exp_out = if upper { 'E' } else { exp_char };
    format!("{mantissa}{exp_out}{sign}{padded}")
}

fn format_string_with_spec(text: String, spec: &FormatSpec) -> String {
    let mut out = text;
    if let Some(prec) = spec.precision {
        out = out.chars().take(prec).collect();
    }
    apply_alignment("", &out, spec, '<')
}

fn format_int_with_spec(obj: MoltObject, spec: &FormatSpec) -> Result<String, FormatError> {
    if spec.precision.is_some() {
        return Err((
            "ValueError",
            Cow::Borrowed("precision not allowed in integer format"),
        ));
    }
    let ty = spec.ty.unwrap_or('d');
    let mut value = if let Some(i) = obj.as_int() {
        BigInt::from(i)
    } else if let Some(b) = obj.as_bool() {
        BigInt::from(if b { 1 } else { 0 })
    } else if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        unsafe { bigint_ref(ptr).clone() }
    } else {
        return Err(("TypeError", Cow::Borrowed("format requires int")));
    };
    if ty == 'c' {
        if value.is_negative() {
            return Err((
                "ValueError",
                Cow::Borrowed("format c requires non-negative int"),
            ));
        }
        let code = value
            .to_u32()
            .ok_or(("ValueError", Cow::Borrowed("format c out of range")))?;
        let ch = std::char::from_u32(code)
            .ok_or(("ValueError", Cow::Borrowed("format c out of range")))?;
        return Ok(format_string_with_spec(ch.to_string(), spec));
    }
    let base = match ty {
        'b' => 2,
        'o' => 8,
        'x' | 'X' => 16,
        'd' | 'n' => 10,
        _ => return Err(("ValueError", Cow::Borrowed("unsupported int format type"))),
    };
    let negative = value.is_negative();
    if negative {
        value = -value;
    }
    let mut digits = value.to_str_radix(base);
    if ty == 'X' {
        digits = digits.to_uppercase();
    }
    if let Some(sep) = spec.grouping {
        let group = match base {
            2 | 16 => 4,
            8 => 3,
            _ => 3,
        };
        digits = apply_grouping(&digits, group, sep);
    }
    let mut prefix = String::new();
    if negative {
        prefix.push('-');
    } else if let Some(sign) = spec.sign
        && (sign == '+' || sign == ' ')
    {
        prefix.push(sign);
    }
    if spec.alternate {
        match ty {
            'b' => prefix.push_str("0b"),
            'o' => prefix.push_str("0o"),
            'x' => prefix.push_str("0x"),
            'X' => prefix.push_str("0X"),
            _ => {}
        }
    }
    Ok(apply_alignment(&prefix, &digits, spec, '>'))
}

pub(crate) fn format_float_with_spec(obj: MoltObject, spec: &FormatSpec) -> Result<String, FormatError> {
    let val = if let Some(f) = obj.as_float() {
        f
    } else if let Some(i) = obj.as_int() {
        i as f64
    } else if let Some(b) = obj.as_bool() {
        if b { 1.0 } else { 0.0 }
    } else {
        return Err(("TypeError", Cow::Borrowed("format requires float")));
    };
    let use_default = spec.ty.is_none() && spec.precision.is_none();
    let ty = spec.ty.unwrap_or('g');
    let upper = matches!(ty, 'F' | 'E' | 'G');
    if val.is_nan() {
        let text = if upper { "NAN" } else { "nan" };
        let prefix = if val.is_sign_negative() { "-" } else { "" };
        return Ok(apply_alignment(prefix, text, spec, '>'));
    }
    if val.is_infinite() {
        let text = if upper { "INF" } else { "inf" };
        let prefix = if val.is_sign_negative() { "-" } else { "" };
        return Ok(apply_alignment(prefix, text, spec, '>'));
    }
    let mut prefix = String::new();
    if val.is_sign_negative() {
        prefix.push('-');
    } else if let Some(sign) = spec.sign
        && (sign == '+' || sign == ' ')
    {
        prefix.push(sign);
    }
    let abs_val = val.abs();
    let prec = spec.precision.unwrap_or(6);
    let mut body = if use_default {
        format_float(abs_val)
    } else {
        match ty {
            'f' | 'F' => format!("{:.*}", prec, abs_val),
            'e' | 'E' => format!("{:.*e}", prec, abs_val),
            'g' | 'G' => {
                let digits = if prec == 0 { 1 } else { prec };
                if abs_val == 0.0 {
                    "0".to_string()
                } else {
                    let exp = abs_val.log10().floor() as i32;
                    if exp < -4 || exp >= digits as i32 {
                        let text = format!("{:.*e}", digits - 1, abs_val);
                        trim_float_trailing(&text, spec.alternate)
                    } else {
                        let frac = (digits as i32 - 1 - exp).max(0) as usize;
                        let text = format!("{:.*}", frac, abs_val);
                        trim_float_trailing(&text, spec.alternate)
                    }
                }
            }
            '%' => {
                let scaled = abs_val * 100.0;
                format!("{:.*}", prec, scaled)
            }
            _ => return Err(("ValueError", Cow::Borrowed("unsupported float format type"))),
        }
    };
    body = normalize_exponent(&body, upper);
    if upper {
        body = body.replace('e', "E");
    }
    if spec.alternate && !body.contains('.') && !body.contains('E') && !body.contains('e') {
        body.push('.');
    }
    if let Some(sep) = spec.grouping
        && !body.contains('e')
        && !body.contains('E')
    {
        let mut parts = body.splitn(2, '.');
        let int_part = parts.next().unwrap_or("");
        let frac_part = parts.next();
        let grouped = apply_grouping(int_part, 3, sep);
        body = if let Some(frac) = frac_part {
            format!("{grouped}.{frac}")
        } else {
            grouped
        };
    }
    if ty == '%' {
        body.push('%');
    }
    Ok(apply_alignment(&prefix, &body, spec, '>'))
}

fn apply_grouping_to_float_text(text: &str, sep: char) -> String {
    if text.contains('e') || text.contains('E') {
        return text.to_string();
    }
    let mut parts = text.splitn(2, '.');
    let int_part = parts.next().unwrap_or("");
    let frac_part = parts.next();
    let grouped = apply_grouping(int_part, 3, sep);
    if let Some(frac) = frac_part {
        format!("{grouped}.{frac}")
    } else {
        grouped
    }
}

fn format_complex_with_spec(
    _py: &PyToken<'_>,
    value: ComplexParts,
    spec: &FormatSpec,
) -> Result<String, FormatError> {
    let mut ty = spec.ty;
    let mut grouping = spec.grouping;
    if ty == Some('n') {
        if let Some(sep) = grouping {
            let msg = if sep == ',' {
                "Cannot specify ',' with 'n'."
            } else {
                "Cannot specify '_' with 'n'."
            };
            return Err(("ValueError", Cow::Borrowed(msg)));
        }
        ty = Some('g');
        grouping = None;
    }
    if let Some(code) = ty
        && !matches!(code, 'e' | 'E' | 'f' | 'F' | 'g' | 'G')
    {
        let msg = format!("Unknown format code '{code}' for object of type 'complex'");
        return Err(("ValueError", Cow::Owned(msg)));
    }
    if spec.fill == '0' {
        return Err((
            "ValueError",
            Cow::Borrowed("Zero padding is not allowed in complex format specifier"),
        ));
    }
    if spec.align == Some('=') {
        return Err((
            "ValueError",
            Cow::Borrowed("'=' alignment flag is not allowed in complex format specifier"),
        ));
    }
    let re = value.re;
    let im = value.im;
    let re_is_zero = re == 0.0 && !re.is_sign_negative();
    let im_is_negative = im.is_sign_negative();
    let im_sign = if im_is_negative { '-' } else { '+' };
    let use_default = spec.ty.is_none() && spec.precision.is_none();
    let (real_text, imag_text) = if use_default {
        let mut real_text = format_complex_float(re.abs());
        let mut imag_text = format_complex_float(im.abs());
        if let Some(sep) = grouping {
            real_text = apply_grouping_to_float_text(&real_text, sep);
            imag_text = apply_grouping_to_float_text(&imag_text, sep);
        }
        (real_text, imag_text)
    } else {
        let real_spec = FormatSpec {
            fill: spec.fill,
            align: None,
            sign: spec.sign,
            alternate: spec.alternate,
            width: None,
            grouping,
            precision: spec.precision,
            ty,
        };
        let imag_spec = FormatSpec {
            fill: spec.fill,
            align: None,
            sign: None,
            alternate: spec.alternate,
            width: None,
            grouping,
            precision: spec.precision,
            ty,
        };
        let real_text = format_float_with_spec(MoltObject::from_float(re), &real_spec)?;
        let imag_text = format_float_with_spec(MoltObject::from_float(im.abs()), &imag_spec)?;
        (real_text, imag_text)
    };
    let include_real = ty.is_some() || !re_is_zero;
    let body = if include_real {
        let real_text = if use_default {
            let mut prefix = String::new();
            if re.is_sign_negative() {
                prefix.push('-');
            } else if let Some(sign) = spec.sign
                && (sign == '+' || sign == ' ')
            {
                prefix.push(sign);
            }
            format!("{prefix}{real_text}")
        } else {
            real_text
        };
        let combined = format!("{real_text}{im_sign}{imag_text}j");
        if ty.is_none() {
            format!("({combined})")
        } else {
            combined
        }
    } else {
        let prefix = if im_is_negative {
            "-"
        } else if let Some(sign) = spec.sign {
            if sign == '+' || sign == ' ' {
                if sign == '+' { "+" } else { " " }
            } else {
                ""
            }
        } else {
            ""
        };
        format!("{prefix}{imag_text}j")
    };
    Ok(apply_alignment("", &body, spec, '>'))
}

pub(crate) fn format_with_spec(
    _py: &PyToken<'_>,
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, FormatError> {
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            if object_type_id(ptr) == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return format_complex_with_spec(_py, value, spec);
            }
        }
    }
    if spec.ty == Some('n') {
        if let Some(sep) = spec.grouping {
            let msg = if sep == ',' {
                "Cannot specify ',' with 'n'."
            } else {
                "Cannot specify '_' with 'n'."
            };
            return Err(("ValueError", Cow::Borrowed(msg)));
        }
        let mut normalized = FormatSpec {
            fill: spec.fill,
            align: spec.align,
            sign: spec.sign,
            alternate: spec.alternate,
            width: spec.width,
            grouping: None,
            precision: spec.precision,
            ty: None,
        };
        if obj.as_float().is_some() {
            normalized.ty = Some('g');
            return format_float_with_spec(obj, &normalized);
        }
        normalized.ty = Some('d');
        return format_int_with_spec(obj, &normalized);
    }
    match spec.ty {
        Some('s') => Ok(format_string_with_spec(format_obj_str(_py, obj), spec)),
        Some('d') | Some('b') | Some('o') | Some('x') | Some('X') | Some('c') => {
            format_int_with_spec(obj, spec)
        }
        Some('f') | Some('F') | Some('e') | Some('E') | Some('g') | Some('G') | Some('%') => {
            format_float_with_spec(obj, spec)
        }
        Some(_) => Err(("ValueError", Cow::Borrowed("unsupported format type"))),
        None => {
            // Check int/bool before float to match CPython's __format__
            // dispatch order.  Also guards against codegen producing raw
            // 0x0 bits (Cranelift zero-init) which NaN-boxing interprets
            // as float +0.0 but semantically represents int 0.
            if obj.as_bool().is_some() {
                Ok(format_string_with_spec(format_obj_str(_py, obj), spec))
            } else if obj.as_int().is_some() || bigint_ptr_from_bits(obj.bits()).is_some() {
                format_int_with_spec(obj, spec)
            } else if obj.as_float().is_some() {
                format_float_with_spec(obj, spec)
            } else {
                Ok(format_string_with_spec(format_obj_str(_py, obj), spec))
            }
        }
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

pub(crate) enum BinaryDunderOutcome {
    Value(u64),
    NotImplemented,
    Missing,
    Error,
}

pub(crate) unsafe fn call_dunder_raw(
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

pub(crate) unsafe fn call_binary_dunder(
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

pub(crate) unsafe fn call_inplace_dunder(
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

pub(crate) struct HashSecret {
    k0: u64,
    k1: u64,
}

const PY_HASH_BITS: u32 = 61;
const PY_HASH_MODULUS: u64 = (1u64 << PY_HASH_BITS) - 1;
const PY_HASH_INF: i64 = 314_159;
const PY_HASH_NONE: i64 = 0xfca86420;
const PY_HASHSEED_MAX: u64 = 4_294_967_295;

static HASH_MODULUS_BIG: OnceLock<BigInt> = OnceLock::new();

fn hash_modulus_big() -> &'static BigInt {
    HASH_MODULUS_BIG.get_or_init(|| BigInt::from(PY_HASH_MODULUS))
}

fn hash_secret(_py: &PyToken<'_>) -> &'static HashSecret {
    runtime_state(_py).hash_secret.get_or_init(init_hash_secret)
}

fn init_hash_secret() -> HashSecret {
    match std::env::var("PYTHONHASHSEED") {
        Ok(value) => {
            if value == "random" {
                if !os_random_supported() {
                    fatal_hash_seed_unavailable();
                }
                return random_hash_secret();
            }
            let seed: u32 = value.parse().unwrap_or_else(|_| fatal_hash_seed(&value));
            if seed == 0 {
                return HashSecret { k0: 0, k1: 0 };
            }
            let bytes = lcg_hash_seed(seed);
            HashSecret {
                k0: u64::from_ne_bytes(bytes[..8].try_into().unwrap()),
                k1: u64::from_ne_bytes(bytes[8..].try_into().unwrap()),
            }
        }
        Err(_) => {
            if os_random_supported() {
                random_hash_secret()
            } else {
                HashSecret { k0: 0, k1: 0 }
            }
        }
    }
}

pub(crate) fn fatal_hash_seed(value: &str) -> ! {
    eprintln!(
        "Fatal Python error: PYTHONHASHSEED must be \"random\" or an integer in range [0; {PY_HASHSEED_MAX}]"
    );
    eprintln!("PYTHONHASHSEED={value}");
    std::process::exit(1);
}

fn fatal_hash_seed_unavailable() -> ! {
    eprintln!("Fatal Python error: PYTHONHASHSEED=random is unavailable on wasm-freestanding");
    eprintln!("Use PYTHONHASHSEED=0 or an explicit integer seed.");
    std::process::exit(1);
}

fn random_hash_secret() -> HashSecret {
    let mut bytes = [0u8; 16];
    if let Err(err) = fill_os_random(&mut bytes) {
        eprintln!("Failed to initialize hash seed: {err}");
        std::process::exit(1);
    }
    HashSecret {
        k0: u64::from_ne_bytes(bytes[..8].try_into().unwrap()),
        k1: u64::from_ne_bytes(bytes[8..].try_into().unwrap()),
    }
}

fn lcg_hash_seed(seed: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut x = seed;
    for byte in out.iter_mut() {
        x = x.wrapping_mul(214013).wrapping_add(2531011);
        *byte = ((x >> 16) & 0xff) as u8;
    }
    out
}

struct SipHasher13 {
    v0: u64,
    v1: u64,
    v2: u64,
    v3: u64,
    tail: u64,
    ntail: usize,
    total_len: u64,
}

impl SipHasher13 {
    fn new(k0: u64, k1: u64) -> Self {
        Self {
            v0: 0x736f6d6570736575 ^ k0,
            v1: 0x646f72616e646f6d ^ k1,
            v2: 0x6c7967656e657261 ^ k0,
            v3: 0x7465646279746573 ^ k1,
            tail: 0,
            ntail: 0,
            total_len: 0,
        }
    }

    fn sip_round(&mut self) {
        self.v0 = self.v0.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(13);
        self.v1 ^= self.v0;
        self.v0 = self.v0.rotate_left(32);
        self.v2 = self.v2.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(16);
        self.v3 ^= self.v2;
        self.v0 = self.v0.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(21);
        self.v3 ^= self.v0;
        self.v2 = self.v2.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(17);
        self.v1 ^= self.v2;
        self.v2 = self.v2.rotate_left(32);
    }

    fn process_block(&mut self, block: u64) {
        self.v3 ^= block;
        self.sip_round();
        self.v0 ^= block;
    }

    fn update(&mut self, bytes: &[u8]) {
        self.total_len = self.total_len.wrapping_add(bytes.len() as u64);
        let mut offset = 0usize;

        // If there's a partial tail from a previous update, fill it first.
        if self.ntail > 0 {
            while offset < bytes.len() && self.ntail < 8 {
                self.tail |= (bytes[offset] as u64) << (8 * self.ntail);
                self.ntail += 1;
                offset += 1;
            }
            if self.ntail == 8 {
                self.process_block(self.tail);
                self.tail = 0;
                self.ntail = 0;
            }
        }

        // Bulk path: process 8-byte blocks directly using little-endian reads.
        // This avoids per-byte shift-and-OR for strings >16 bytes (common for
        // dict keys like module-qualified names, file paths, etc.).
        let remaining = &bytes[offset..];
        let chunks = remaining.len() / 8;
        for i in 0..chunks {
            let block = u64::from_le_bytes([
                remaining[i * 8],
                remaining[i * 8 + 1],
                remaining[i * 8 + 2],
                remaining[i * 8 + 3],
                remaining[i * 8 + 4],
                remaining[i * 8 + 5],
                remaining[i * 8 + 6],
                remaining[i * 8 + 7],
            ]);
            self.process_block(block);
        }
        offset += chunks * 8;

        // Tail: accumulate remaining bytes (0-7).
        for &byte in &bytes[offset..] {
            self.tail |= (byte as u64) << (8 * self.ntail);
            self.ntail += 1;
        }
    }

    fn finish(mut self) -> u64 {
        let b = self.tail | ((self.total_len & 0xff) << 56);
        self.process_block(b);
        self.v2 ^= 0xff;
        for _ in 0..3 {
            self.sip_round();
        }
        self.v0 ^ self.v1 ^ self.v2 ^ self.v3
    }
}

fn fix_hash(hash: i64) -> i64 {
    if hash == -1 { -2 } else { hash }
}

fn exp_mod(exp: i32) -> u32 {
    if exp >= 0 {
        (exp as u32) % PY_HASH_BITS
    } else {
        PY_HASH_BITS - 1 - ((-1 - exp) as u32 % PY_HASH_BITS)
    }
}

fn pow2_mod(exp: u32) -> u64 {
    let mut value = 1u64;
    for _ in 0..exp {
        value <<= 1;
        if value >= PY_HASH_MODULUS {
            value -= PY_HASH_MODULUS;
        }
    }
    value
}

fn reduce_mersenne(mut value: u128) -> u64 {
    let mask = PY_HASH_MODULUS as u128;
    value = (value & mask) + (value >> PY_HASH_BITS);
    value = (value & mask) + (value >> PY_HASH_BITS);
    if value >= mask {
        value -= mask;
    }
    if value == mask { 0 } else { value as u64 }
}

fn mul_mod_mersenne(lhs: u64, rhs: u64) -> u64 {
    reduce_mersenne((lhs as u128) * (rhs as u128))
}

fn frexp(value: f64) -> (f64, i32) {
    if value == 0.0 {
        return (0.0, 0);
    }
    let bits = value.to_bits();
    let mut exp = ((bits >> 52) & 0x7ff) as i32;
    let mut mant = bits & ((1u64 << 52) - 1);
    if exp == 0 {
        let mut e = -1022;
        while mant & (1u64 << 52) == 0 {
            mant <<= 1;
            e -= 1;
        }
        exp = e;
        mant &= (1u64 << 52) - 1;
    } else {
        exp -= 1022;
    }
    let frac_bits = (1022u64 << 52) | mant;
    let frac = f64::from_bits(frac_bits);
    (frac, exp)
}

fn hash_bytes_with_secret(bytes: &[u8], secret: &HashSecret) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let mut hasher = SipHasher13::new(secret.k0, secret.k1);
    hasher.update(bytes);
    fix_hash(hasher.finish() as i64)
}

fn hash_bytes(_py: &PyToken<'_>, bytes: &[u8]) -> i64 {
    hash_bytes_with_secret(bytes, hash_secret(_py))
}

pub(crate) fn hash_string_bytes(_py: &PyToken<'_>, bytes: &[u8]) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let secret = hash_secret(_py);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return hash_bytes_with_secret(bytes, secret);
    };
    // SIMD fast path: if all bytes < 0x80, all codepoints are ASCII (max_codepoint ≤ 0x7F).
    // Use SIMD to check this in bulk rather than iterating char-by-char.
    let max_codepoint = simd_max_byte_value(bytes);
    let mut hasher = SipHasher13::new(secret.k0, secret.k1);
    if max_codepoint <= 0x7f {
        // Pure ASCII: each byte is a codepoint, hash as u8 directly
        hasher.update(bytes);
    } else if max_codepoint <= 0xff {
        for ch in text.chars() {
            hasher.update(&[ch as u8]);
        }
    } else if max_codepoint <= 0xffff {
        for ch in text.chars() {
            let bytes = (ch as u16).to_ne_bytes();
            hasher.update(&bytes);
        }
    } else {
        for ch in text.chars() {
            let bytes = (ch as u32).to_ne_bytes();
            hasher.update(&bytes);
        }
    }
    fix_hash(hasher.finish() as i64)
}

/// SIMD-accelerated max byte value scan. Returns the maximum byte value in the slice.
/// Used to quickly determine string encoding width (ASCII, Latin-1, BMP, full Unicode).
#[inline]
fn simd_max_byte_value(bytes: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 32 && std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { simd_max_byte_avx2(bytes) };
        }
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { simd_max_byte_sse2(bytes) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { simd_max_byte_neon(bytes) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        if bytes.len() >= 16 {
            return unsafe { simd_max_byte_wasm32(bytes) };
        }
    }
    // Scalar fallback — also handles short strings and decodes actual codepoints
    let mut max = 0u32;
    if let Ok(text) = std::str::from_utf8(bytes) {
        for ch in text.chars() {
            max = max.max(ch as u32);
        }
    } else {
        for &b in bytes {
            max = max.max(b as u32);
        }
    }
    max
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_max_byte_sse2(bytes: &[u8]) -> u32 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vmax = _mm_setzero_si128();
    while i + 16 <= bytes.len() {
        let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
        vmax = _mm_max_epu8(vmax, v);
        i += 16;
    }
    // Horizontal max: fold 128 bits down to a single max byte
    let hi64 = _mm_srli_si128(vmax, 8);
    vmax = _mm_max_epu8(vmax, hi64);
    let hi32 = _mm_srli_si128(vmax, 4);
    vmax = _mm_max_epu8(vmax, hi32);
    let hi16 = _mm_srli_si128(vmax, 2);
    vmax = _mm_max_epu8(vmax, hi16);
    let hi8 = _mm_srli_si128(vmax, 1);
    vmax = _mm_max_epu8(vmax, hi8);
    let mut max = (_mm_extract_epi8(vmax, 0) & 0xFF) as u32;
    // Tail bytes
    for &b in &bytes[i..] {
        max = max.max(b as u32);
    }
    // If all bytes < 0x80, return the byte max directly (it's ASCII, so codepoint == byte)
    // If any byte >= 0x80, fall back to full codepoint scan since UTF-8 multi-byte chars
    // could have codepoints > 0xFF
    if max >= 0x80 {
        let mut cp_max = 0u32;
        if let Ok(text) = std::str::from_utf8(bytes) {
            for ch in text.chars() {
                cp_max = cp_max.max(ch as u32);
            }
        }
        return cp_max;
    }
    max
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_max_byte_avx2(bytes: &[u8]) -> u32 {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    let mut vmax = _mm256_setzero_si256();
    while i + 32 <= bytes.len() {
        let v = _mm256_loadu_si256(bytes.as_ptr().add(i) as *const __m256i);
        vmax = _mm256_max_epu8(vmax, v);
        i += 32;
    }
    // Fold 256 to 128
    let lo = _mm256_castsi256_si128(vmax);
    let hi = _mm256_extracti128_si256(vmax, 1);
    let mut v128 = _mm_max_epu8(lo, hi);
    // Fold 128 to single byte
    let hi64 = _mm_srli_si128(v128, 8);
    v128 = _mm_max_epu8(v128, hi64);
    let hi32 = _mm_srli_si128(v128, 4);
    v128 = _mm_max_epu8(v128, hi32);
    let hi16 = _mm_srli_si128(v128, 2);
    v128 = _mm_max_epu8(v128, hi16);
    let hi8 = _mm_srli_si128(v128, 1);
    v128 = _mm_max_epu8(v128, hi8);
    let mut max = (_mm_extract_epi8(v128, 0) & 0xFF) as u32;
    for &b in &bytes[i..] {
        max = max.max(b as u32);
    }
    if max >= 0x80 {
        let mut cp_max = 0u32;
        if let Ok(text) = std::str::from_utf8(bytes) {
            for ch in text.chars() {
                cp_max = cp_max.max(ch as u32);
            }
        }
        return cp_max;
    }
    max
}

#[cfg(target_arch = "aarch64")]
unsafe fn simd_max_byte_neon(bytes: &[u8]) -> u32 {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vmax = vdupq_n_u8(0);
        while i + 16 <= bytes.len() {
            let v = vld1q_u8(bytes.as_ptr().add(i));
            vmax = vmaxq_u8(vmax, v);
            i += 16;
        }
        let mut max = vmaxvq_u8(vmax) as u32;
        for &b in &bytes[i..] {
            max = max.max(b as u32);
        }
        if max >= 0x80 {
            let mut cp_max = 0u32;
            if let Ok(text) = std::str::from_utf8(bytes) {
                for ch in text.chars() {
                    cp_max = cp_max.max(ch as u32);
                }
            }
            return cp_max;
        }
        max
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn simd_max_byte_wasm32(bytes: &[u8]) -> u32 {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        let mut vmax = u8x16_splat(0);
        while i + 16 <= bytes.len() {
            let v = v128_load(bytes.as_ptr().add(i) as *const v128);
            vmax = u8x16_max(vmax, v);
            i += 16;
        }
        // Horizontal max: fold 128 bits down to single byte
        let hi64 =
            u8x16_shuffle::<8, 9, 10, 11, 12, 13, 14, 15, 0, 0, 0, 0, 0, 0, 0, 0>(vmax, vmax);
        vmax = u8x16_max(vmax, hi64);
        let hi32 = u8x16_shuffle::<4, 5, 6, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0>(vmax, vmax);
        vmax = u8x16_max(vmax, hi32);
        let hi16 = u8x16_shuffle::<2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0>(vmax, vmax);
        vmax = u8x16_max(vmax, hi16);
        let hi8 = u8x16_shuffle::<1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0>(vmax, vmax);
        vmax = u8x16_max(vmax, hi8);
        let mut max = u8x16_extract_lane::<0>(vmax) as u32;
        for &b in &bytes[i..] {
            max = max.max(b as u32);
        }
        if max >= 0x80 {
            let mut cp_max = 0u32;
            if let Ok(text) = std::str::from_utf8(bytes) {
                for ch in text.chars() {
                    cp_max = cp_max.max(ch as u32);
                }
            }
            return cp_max;
        }
        max
    }
}

fn hash_string(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let cached = super::object_state(ptr);
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let len = unsafe { string_len(ptr) };
    let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), len) };
    let hash = hash_string_bytes(_py, bytes);
    super::object_set_state(ptr, hash.wrapping_add(1));
    hash
}

fn hash_bytes_cached(_py: &PyToken<'_>, ptr: *mut u8, bytes: &[u8]) -> i64 {
    let cached = super::object_state(ptr);
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let hash = hash_bytes(_py, bytes);
    super::object_set_state(ptr, hash.wrapping_add(1));
    hash
}

fn hash_int(val: i64) -> i64 {
    // Fast path: for values whose magnitude fits within PY_HASH_MODULUS
    // (which includes all 47-bit inline NaN-boxed ints), skip the i128
    // modulus arithmetic entirely.
    if val >= 0 && (val as u64) < PY_HASH_MODULUS {
        return val; // val >= 0 so val != -1, fix_hash not needed
    }
    if val < 0 && val != i64::MIN {
        let mag = (-val) as u64;
        if mag < PY_HASH_MODULUS {
            return fix_hash(val); // fix_hash handles -1 -> -2
        }
    }
    let mut mag = val as i128;
    let sign = if mag < 0 { -1 } else { 1 };
    if mag < 0 {
        mag = -mag;
    }
    let modulus = PY_HASH_MODULUS as i128;
    let mut hash = (mag % modulus) as i64;
    if sign < 0 {
        hash = -hash;
    }
    fix_hash(hash)
}

fn hash_bigint(ptr: *mut u8) -> i64 {
    let big = unsafe { bigint_ref(ptr) };
    let sign = big.sign();
    let modulus = hash_modulus_big();
    let hash = big.abs().mod_floor(modulus);
    let mut hash = hash.to_i64().unwrap_or(0);
    if sign == Sign::Minus {
        hash = -hash;
    }
    fix_hash(hash)
}

fn hash_float(val: f64) -> i64 {
    if val.is_nan() {
        return 0;
    }
    if val.is_infinite() {
        return if val.is_sign_positive() {
            PY_HASH_INF
        } else {
            -PY_HASH_INF
        };
    }
    if val == 0.0 {
        return 0;
    }
    let value = val.abs();
    let mut sign = 1i64;
    if val.is_sign_negative() {
        sign = -1;
    }
    let (mut frac, mut exp) = frexp(value);
    let mut hash = 0u64;
    while frac != 0.0 {
        frac *= (1u64 << 28) as f64;
        let intpart = frac as u64;
        frac -= intpart as f64;
        hash = ((hash << 28) & PY_HASH_MODULUS) | intpart;
        exp -= 28;
    }
    let exp = exp_mod(exp);
    hash = mul_mod_mersenne(hash, pow2_mod(exp));
    let hash = (hash as i64) * sign;
    fix_hash(hash)
}

fn hash_complex(re: f64, im: f64) -> i64 {
    let re_hash = hash_float(re);
    let im_hash = hash_float(im);
    let mut hash = re_hash.wrapping_add(im_hash.wrapping_mul(1000003));
    if hash == -1 {
        hash = -2;
    }
    hash
}

fn hash_tuple(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let elems = unsafe { seq_vec_ref(ptr) };
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let mut acc = XXPRIME_5;
        for &elem in elems.iter() {
            let lane = hash_bits_signed(_py, elem);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(31);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((elems.len() as u64) ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let mut acc = XXPRIME_5;
        for &elem in elems.iter() {
            let lane = hash_bits_signed(_py, elem);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(13);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((elems.len() as u32) ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        (acc as i32) as i64
    }
}

fn hash_dataclass_fields(
    _py: &PyToken<'_>,
    fields: &[u64],
    flags: &[u8],
    field_names: &[String],
    type_label: &str,
) -> i64 {
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let mut acc = XXPRIME_5;
        let mut count = 0usize;
        for (idx, &elem) in fields.iter().enumerate() {
            let flag = flags.get(idx).copied().unwrap_or(0x7);
            if (flag & 0x4) == 0 {
                continue;
            }
            if is_missing_bits(_py, elem) {
                let name = field_names.get(idx).map(|s| s.as_str()).unwrap_or("field");
                let _ = attr_error(_py, type_label, name);
                return 0;
            }
            count += 1;
            let lane = hash_bits_signed(_py, elem);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(31);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((count as u64) ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let mut acc = XXPRIME_5;
        let mut count = 0usize;
        for (idx, &elem) in fields.iter().enumerate() {
            let flag = flags.get(idx).copied().unwrap_or(0x7);
            if (flag & 0x4) == 0 {
                continue;
            }
            if is_missing_bits(_py, elem) {
                let name = field_names.get(idx).map(|s| s.as_str()).unwrap_or("field");
                let _ = attr_error(_py, type_label, name);
                return 0;
            }
            count += 1;
            let lane = hash_bits_signed(_py, elem);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(13);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((count as u32) ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        (acc as i32) as i64
    }
}

fn hash_generic_alias(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let origin_bits = unsafe { generic_alias_origin_bits(ptr) };
    let args_bits = unsafe { generic_alias_args_bits(ptr) };
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let mut acc = XXPRIME_5;
        for lane_bits in [origin_bits, args_bits] {
            let lane = hash_bits_signed(_py, lane_bits);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(31);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add(2u64 ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let mut acc = XXPRIME_5;
        for lane_bits in [origin_bits, args_bits] {
            let lane = hash_bits_signed(_py, lane_bits);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(13);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add(2u32 ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        (acc as i32) as i64
    }
}

fn hash_union_type(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let args_bits = unsafe { union_type_args_bits(ptr) };
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let lane = hash_bits_signed(_py, args_bits);
        if exception_pending(_py) {
            return 0;
        }
        let mut acc = XXPRIME_5;
        acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(XXPRIME_1);
        acc = acc.wrapping_add(1u64 ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let lane = hash_bits_signed(_py, args_bits);
        if exception_pending(_py) {
            return 0;
        }
        let mut acc = XXPRIME_5;
        acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(13);
        acc = acc.wrapping_mul(XXPRIME_1);
        acc = acc.wrapping_add(1u32 ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        (acc as i32) as i64
    }
}

#[cfg(target_pointer_width = "64")]
fn slice_hash_acc(lanes: [u64; 3]) -> u64 {
    const XXPRIME_1: u64 = 11400714785074694791;
    const XXPRIME_2: u64 = 14029467366897019727;
    const XXPRIME_5: u64 = 2870177450012600261;
    let mut acc = XXPRIME_5;
    for lane in lanes {
        acc = acc.wrapping_add(lane.wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(XXPRIME_1);
    }
    acc
}

#[cfg(target_pointer_width = "32")]
fn slice_hash_acc(lanes: [u32; 3]) -> u32 {
    const XXPRIME_1: u32 = 2654435761;
    const XXPRIME_2: u32 = 2246822519;
    const XXPRIME_5: u32 = 374761393;
    let mut acc = XXPRIME_5;
    for lane in lanes {
        acc = acc.wrapping_add(lane.wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(13);
        acc = acc.wrapping_mul(XXPRIME_1);
    }
    acc
}

pub(crate) fn hash_slice_bits(
    _py: &PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> Option<i64> {
    let mut lanes = [0i64; 3];
    let elems = [start_bits, stop_bits, step_bits];
    for (idx, bits) in elems.iter().enumerate() {
        lanes[idx] = hash_bits_signed(_py, *bits);
        if exception_pending(_py) {
            return None;
        }
    }
    #[cfg(target_pointer_width = "64")]
    {
        let acc = slice_hash_acc([lanes[0] as u64, lanes[1] as u64, lanes[2] as u64]);
        if acc == u64::MAX {
            return Some(1546275796);
        }
        Some(acc as i64)
    }
    #[cfg(target_pointer_width = "32")]
    {
        let acc = slice_hash_acc([lanes[0] as u32, lanes[1] as u32, lanes[2] as u32]);
        if acc == u32::MAX {
            return Some(1546275796);
        }
        Some((acc as i32) as i64)
    }
}

fn shuffle_frozenset_hash(hash: u64) -> u64 {
    let mixed = (hash ^ 89869747u64) ^ (hash << 16);
    mixed.wrapping_mul(3644798167u64)
}

fn hash_frozenset(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let elems = unsafe { set_order(ptr) };
    let mut hash = 0u64;
    for &elem in elems.iter() {
        hash ^= shuffle_frozenset_hash(hash_bits(_py, elem));
    }
    if elems.len() & 1 == 1 {
        hash ^= shuffle_frozenset_hash(0);
    }
    hash ^= ((elems.len() as u64) + 1).wrapping_mul(1927868237u64);
    hash ^= (hash >> 11) ^ (hash >> 25);
    hash = hash.wrapping_mul(69069u64).wrapping_add(907133923u64);
    if hash == u64::MAX {
        hash = 590923713u64;
    }
    hash as i64
}

pub(crate) fn hash_pointer(ptr: u64) -> i64 {
    let hash = (ptr >> 4) as i64;
    fix_hash(hash)
}

fn hash_unhashable(_py: &PyToken<'_>, obj: MoltObject) -> i64 {
    let name = type_name(_py, obj);
    let msg = format!("unhashable type: '{name}'");
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn is_unhashable_type(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_LIST
            | TYPE_ID_DICT
            | TYPE_ID_SET
            | TYPE_ID_BYTEARRAY
            | TYPE_ID_MEMORYVIEW
            | TYPE_ID_LIST_BUILDER
            | TYPE_ID_DICT_BUILDER
            | TYPE_ID_SET_BUILDER
            | TYPE_ID_DICT_KEYS_VIEW
            | TYPE_ID_DICT_VALUES_VIEW
            | TYPE_ID_DICT_ITEMS_VIEW
            | TYPE_ID_CALLARGS
    )
}

pub(crate) fn hash_bits_signed(_py: &PyToken<'_>, bits: u64) -> i64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = obj.as_int() {
        return hash_int(i);
    }
    if let Some(b) = obj.as_bool() {
        return hash_int(if b { 1 } else { 0 });
    }
    if obj.is_none() {
        return PY_HASH_NONE;
    }
    if let Some(f) = obj.as_float() {
        return hash_float(f);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if is_unhashable_type(type_id) {
                return hash_unhashable(_py, obj);
            }
            if type_id == TYPE_ID_STRING {
                return hash_string(_py, ptr);
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return hash_bytes_cached(_py, ptr, bytes);
            }
            if type_id == TYPE_ID_BIGINT {
                return hash_bigint(ptr);
            }
            if type_id == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return hash_complex(value.re, value.im);
            }
            if type_id == TYPE_ID_TUPLE {
                return hash_tuple(_py, ptr);
            }
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(ptr);
                if desc_ptr.is_null() {
                    return hash_pointer(ptr as u64);
                }
                let desc = &*desc_ptr;
                match desc.hash_mode {
                    2 => return hash_unhashable(_py, obj),
                    3 => {
                        let hash_name_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.hash_name,
                            b"__hash__",
                        );
                        if let Some(call_bits) =
                            attr_lookup_ptr_allow_missing(_py, ptr, hash_name_bits)
                        {
                            let res_bits = call_callable0(_py, call_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                dec_ref_bits(_py, res_bits);
                                return 0;
                            }
                            let res_obj = obj_from_bits(res_bits);
                            if let Some(val) = to_i64(res_obj) {
                                dec_ref_bits(_py, res_bits);
                                return fix_hash(val);
                            }
                            if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                                let big = bigint_ref(big_ptr);
                                let Some(val) = big.to_i64() else {
                                    dec_ref_bits(_py, res_bits);
                                    return raise_exception::<i64>(
                                        _py,
                                        "OverflowError",
                                        "cannot fit 'int' into an index-sized integer",
                                    );
                                };
                                dec_ref_bits(_py, res_bits);
                                return fix_hash(val);
                            }
                            dec_ref_bits(_py, res_bits);
                            return raise_exception::<i64>(
                                _py,
                                "TypeError",
                                "__hash__ returned non-int",
                            );
                        }
                        return hash_pointer(ptr as u64);
                    }
                    1 => {
                        let fields = dataclass_fields_ref(ptr);
                        let type_label = if desc.name.is_empty() {
                            "dataclass"
                        } else {
                            desc.name.as_str()
                        };
                        return hash_dataclass_fields(
                            _py,
                            fields,
                            &desc.field_flags,
                            &desc.field_names,
                            type_label,
                        );
                    }
                    _ => return hash_pointer(ptr as u64),
                }
            }
            if type_id == TYPE_ID_TYPE {
                let metaclass_bits = type_of_bits(_py, obj.bits());
                if metaclass_bits == builtin_classes(_py).type_obj {
                    return hash_pointer(ptr as u64);
                }
                let hash_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.hash_name, b"__hash__");
                let eq_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
                let mut meta_overrides_hash = false;
                if let Some(meta_ptr) = obj_from_bits(metaclass_bits).as_ptr()
                    && object_type_id(meta_ptr) == TYPE_ID_TYPE
                {
                    let dict_bits = class_dict_bits(meta_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        meta_overrides_hash = dict_get_in_place(_py, dict_ptr, hash_name_bits)
                            .is_some()
                            || dict_get_in_place(_py, dict_ptr, eq_name_bits).is_some();
                    }
                }
                if meta_overrides_hash && let Some(hash) = hash_from_dunder(_py, obj, ptr) {
                    return hash;
                }
                return hash_pointer(ptr as u64);
            }
            if type_id == TYPE_ID_GENERIC_ALIAS {
                return hash_generic_alias(_py, ptr);
            }
            if type_id == TYPE_ID_UNION {
                return hash_union_type(_py, ptr);
            }
            if type_id == TYPE_ID_SLICE {
                let start_bits = slice_start_bits(ptr);
                let stop_bits = slice_stop_bits(ptr);
                let step_bits = slice_step_bits(ptr);
                if let Some(hash) = hash_slice_bits(_py, start_bits, stop_bits, step_bits) {
                    return hash;
                }
                return 0;
            }
            if type_id == TYPE_ID_FROZENSET {
                return hash_frozenset(_py, ptr);
            }
            if let Some(hash) = hash_from_dunder(_py, obj, ptr) {
                return hash;
            }
        }
        return hash_pointer(ptr as u64);
    }
    hash_pointer(bits)
}

unsafe fn hash_from_dunder(_py: &PyToken<'_>, obj: MoltObject, obj_ptr: *mut u8) -> Option<i64> {
    unsafe {
        let hash_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.hash_name, b"__hash__");
        let eq_name_bits = intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
        let class_bits = type_of_bits(_py, obj.bits());
        let default_type_hashable = class_bits == builtin_classes(_py).type_obj;
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && !default_type_hashable
        {
            let dict_bits = class_dict_bits(class_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                let hash_entry = dict_get_in_place(_py, dict_ptr, hash_name_bits);
                if exception_pending(_py) {
                    return Some(0);
                }
                if let Some(hash_bits) = hash_entry {
                    if obj_from_bits(hash_bits).is_none() {
                        let name = type_name(_py, obj);
                        let msg = format!("unhashable type: '{name}'");
                        return Some(raise_exception::<i64>(_py, "TypeError", &msg));
                    }
                } else if dict_get_in_place(_py, dict_ptr, eq_name_bits).is_some() {
                    let name = type_name(_py, obj);
                    let msg = format!("unhashable type: '{name}'");
                    return Some(raise_exception::<i64>(_py, "TypeError", &msg));
                }
                if exception_pending(_py) {
                    return Some(0);
                }
            }
        }
        let call_bits = attr_lookup_ptr_allow_missing(_py, obj_ptr, hash_name_bits)?;
        if obj_from_bits(call_bits).is_none() {
            dec_ref_bits(_py, call_bits);
            if default_type_hashable {
                return None;
            }
            let name = type_name(_py, obj);
            let msg = format!("unhashable type: '{name}'");
            return Some(raise_exception::<i64>(_py, "TypeError", &msg));
        }
        let res_bits = call_callable0(_py, call_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            if !obj_from_bits(res_bits).is_none() {
                dec_ref_bits(_py, res_bits);
            }
            return Some(0);
        }
        let res_obj = obj_from_bits(res_bits);
        let hash = if let Some(i) = to_i64(res_obj) {
            hash_int(i)
        } else if let Some(ptr) = res_obj.as_ptr() {
            if object_type_id(ptr) == TYPE_ID_BIGINT {
                hash_bigint(ptr)
            } else {
                let msg = "__hash__ method should return an integer";
                dec_ref_bits(_py, res_bits);
                return Some(raise_exception::<i64>(_py, "TypeError", msg));
            }
        } else {
            let msg = "__hash__ method should return an integer";
            dec_ref_bits(_py, res_bits);
            return Some(raise_exception::<i64>(_py, "TypeError", msg));
        };
        dec_ref_bits(_py, res_bits);
        Some(hash)
    }
}

fn hash_bits(_py: &PyToken<'_>, bits: u64) -> u64 {
    hash_bits_signed(_py, bits) as u64
}

pub(crate) fn ensure_hashable(_py: &PyToken<'_>, key_bits: u64) -> bool {
    let obj = obj_from_bits(key_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if is_unhashable_type(type_id) {
                let name = type_name(_py, obj);
                let msg = format!("unhashable type: '{name}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    true
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
pub(crate) unsafe fn simd_bytes_eq(a: *const u8, b: *const u8, len: usize) -> bool {
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

pub(crate) fn concat_bytes_like(_py: &PyToken<'_>, left: &[u8], right: &[u8], type_id: u32) -> Option<u64> {
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

pub(crate) fn fill_repeated_bytes(dst: &mut [u8], pattern: &[u8]) {
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

