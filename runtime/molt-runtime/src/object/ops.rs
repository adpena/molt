use crate::object::utf8_cache::{
    Utf8CountCache, Utf8CountCacheEntry, Utf8IndexCache, UTF8_CACHE_BLOCK, UTF8_CACHE_MIN_LEN,
    UTF8_COUNT_CACHE_SHARDS, UTF8_COUNT_PREFIX_MIN_LEN, UTF8_COUNT_TLS,
};
use crate::*;
use getrandom::getrandom;
use memchr::{memchr, memmem};
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::ffi::CStr;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};

fn slice_bounds_from_args(
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

fn slice_match(slice: &[u8], needle: &[u8], start_raw: i64, total: i64, suffix: bool) -> bool {
    if needle.is_empty() {
        return start_raw <= total;
    }
    if suffix {
        slice.ends_with(needle)
    } else {
        slice.starts_with(needle)
    }
}

#[no_mangle]
pub extern "C" fn molt_range_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let start = match to_i64(obj_from_bits(start_bits)) {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let stop = match to_i64(obj_from_bits(stop_bits)) {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let step = match to_i64(obj_from_bits(step_bits)) {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        if step == 0 {
            return raise_exception::<_>(_py, "ValueError", "range() arg 3 must not be zero");
        }
        let ptr = alloc_range(_py, start, stop, step);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_slice_reduce_ex(slice_bits: u64, _protocol_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_slice_reduce(slice_bits) })
}

#[no_mangle]
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
                )
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
                )
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
        let desc = Box::new(DataclassDesc {
            name,
            field_names,
            frozen,
            eq,
            repr,
            slots,
            class_bits: 0,
        });
        let desc_ptr = Box::into_raw(desc);

        let total = std::mem::size_of::<MoltHeader>()
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

#[no_mangle]
pub extern "C" fn molt_dataclass_get(obj_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let idx = match obj_from_bits(index_bits).as_int() {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "dataclass field index must be int")
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
                inc_ref_bits(_py, val);
                return val;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_dataclass_set(obj_bits: u64, index_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let idx = match obj_from_bits(index_bits).as_int() {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "dataclass field index must be int")
            }
        };
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) != TYPE_ID_DATACLASS {
                    return MoltObject::none().bits();
                }
                let desc_ptr = dataclass_desc_ptr(ptr);
                if !desc_ptr.is_null() && (*desc_ptr).frozen {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "cannot assign to frozen dataclass field",
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

#[no_mangle]
pub extern "C" fn molt_dataclass_set_class(obj_bits: u64, class_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dataclass expects object");
        };
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
            }
        }
        MoltObject::none().bits()
    })
}

// --- NaN-boxed ops ---

#[no_mangle]
pub extern "C" fn molt_add(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            let res = li as i128 + ri as i128;
            return int_bits_from_i128(_py, res);
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
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return MoltObject::from_float(lf + rf).bits();
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

#[no_mangle]
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
                    return a;
                }
                if ltype == TYPE_ID_BYTEARRAY {
                    if bytearray_concat_in_place(_py, ptr, b) {
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

#[no_mangle]
pub extern "C" fn molt_sub(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            let res = li as i128 - ri as i128;
            return int_bits_from_i128(_py, res);
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big - r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return MoltObject::from_float(lf - rf).bits();
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

#[no_mangle]
pub extern "C" fn molt_inplace_sub(a: u64, b: u64) -> u64 {
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
                        return raise_unsupported_inplace(_py, "-=", a, b);
                    }
                    let _ = molt_set_difference_update(a, b);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
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

fn repeat_sequence(_py: &PyToken<'_>, ptr: *mut u8, count: i64) -> Option<u64> {
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

unsafe fn bytearray_repeat_in_place(_py: &PyToken<'_>, ptr: *mut u8, count: i64) -> bool {
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

unsafe fn bytearray_concat_in_place(_py: &PyToken<'_>, ptr: *mut u8, other_bits: u64) -> bool {
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

#[no_mangle]
pub extern "C" fn molt_inplace_mul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
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
                    return a;
                }
            }
        }
        molt_mul(a, b)
    })
}

#[no_mangle]
pub extern "C" fn molt_mul(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            let res = li as i128 * ri as i128;
            return int_bits_from_i128(_py, res);
        }
        if let Some(count) = to_i64(lhs) {
            if let Some(ptr) = rhs.as_ptr() {
                if let Some(bits) = repeat_sequence(_py, ptr, count) {
                    return bits;
                }
            }
        }
        if let Some(count) = to_i64(rhs) {
            if let Some(ptr) = lhs.as_ptr() {
                if let Some(bits) = repeat_sequence(_py, ptr, count) {
                    return bits;
                }
            }
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            let res = l_big * r_big;
            if let Some(i) = bigint_to_inline(&res) {
                return MoltObject::from_int(i).bits();
            }
            return bigint_bits(_py, res);
        }
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            return MoltObject::from_float(lf * rf).bits();
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

#[no_mangle]
pub extern "C" fn molt_div(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let Some((lf, rf)) = float_pair_from_obj(_py, lhs, rhs) {
            if rf == 0.0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "division by zero");
            }
            return MoltObject::from_float(lf / rf).bits();
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
        return raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for /");
    })
}

#[no_mangle]
pub extern "C" fn molt_floordiv(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            return MoltObject::from_int(li.div_euclid(ri)).bits();
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
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
        return raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for //");
    })
}

#[no_mangle]
pub extern "C" fn molt_mod(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
            if ri == 0 {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
            }
            let mut rem = li % ri;
            if rem != 0 && (rem > 0) != (ri > 0) {
                rem += ri;
            }
            return MoltObject::from_int(rem).bits();
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if r_big.is_zero() {
                return raise_exception::<_>(
                    _py,
                    "ZeroDivisionError",
                    "integer division or modulo by zero",
                );
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
        return raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for %");
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

#[no_mangle]
pub extern "C" fn molt_pow(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
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
            return MoltObject::from_float(lf.powf(rf)).bits();
        }
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
            if let Some(exp) = r_big.to_u64() {
                let res = l_big.pow(exp as u32);
                if let Some(i) = bigint_to_inline(&res) {
                    return MoltObject::from_int(i).bits();
                }
                return bigint_bits(_py, res);
            }
            if r_big.is_negative() {
                if let Some(lf) = l_big.to_f64() {
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
            return MoltObject::from_float(lf.powf(rf)).bits();
        }
        return raise_exception::<_>(_py, "TypeError", "unsupported operand type(s) for **");
    })
}

#[no_mangle]
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
        return raise_exception::<_>(
            _py,
            "TypeError",
            "pow() 3rd argument not allowed unless all arguments are integers",
        );
    })
}

#[no_mangle]
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
        if !val.is_int() && !val.is_bool() && !val.is_float() {
            if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
                unsafe {
                    let round_name_bits = intern_static_name(
                        _py,
                        &runtime_state(_py).interned.round_name,
                        b"__round__",
                    );
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
        }
        if let Some(i) = to_i64(val) {
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
        return raise_exception::<_>(_py, "TypeError", "round() expects a real number");
    })
}

#[no_mangle]
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
        return raise_exception::<_>(_py, "TypeError", "trunc() expects a real number");
    })
}

fn set_like_result_type_id(type_id: u32) -> u32 {
    if type_id == TYPE_ID_FROZENSET {
        TYPE_ID_FROZENSET
    } else {
        TYPE_ID_SET
    }
}

unsafe fn set_like_new_bits(type_id: u32, capacity: usize) -> u64 {
    if type_id == TYPE_ID_FROZENSET {
        molt_frozenset_new(capacity as u64)
    } else {
        molt_set_new(capacity as u64)
    }
}

unsafe fn set_like_union(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
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

unsafe fn set_like_intersection(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
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
        if set_find_entry(_py, probe_elems, probe_table, entry).is_some() {
            set_add_in_place(_py, res_ptr, entry);
        }
    }
    res_bits
}

unsafe fn set_like_difference(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
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
        if set_find_entry(_py, r_elems, r_table, entry).is_none() {
            set_add_in_place(_py, res_ptr, entry);
        }
    }
    res_bits
}

unsafe fn set_like_symdiff(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
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
        if set_find_entry(_py, r_elems, r_table, entry).is_none() {
            set_add_in_place(_py, res_ptr, entry);
        }
    }
    for &entry in r_elems.iter() {
        if set_find_entry(_py, l_elems, l_table, entry).is_none() {
            set_add_in_place(_py, res_ptr, entry);
        }
    }
    res_bits
}

unsafe fn set_like_copy_bits(_py: &PyToken<'_>, ptr: *mut u8, result_type_id: u32) -> u64 {
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

unsafe fn set_like_ptr_from_bits(
    _py: &PyToken<'_>,
    other_bits: u64,
) -> Option<(*mut u8, Option<u64>)> {
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

fn binary_type_error(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject, op: &str) -> u64 {
    let msg = format!(
        "unsupported operand type(s) for {op}: '{}' and '{}'",
        type_name(_py, lhs),
        type_name(_py, rhs)
    );
    return raise_exception::<_>(_py, "TypeError", &msg);
}

#[no_mangle]
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

#[no_mangle]
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
                    return a;
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
        return raise_exception::<_>(_py, "TypeError", &msg);
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
                if value >= 0 {
                    0
                } else {
                    -1
                }
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

#[no_mangle]
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
        binary_type_error(_py, lhs, rhs, "@")
    })
}

fn compare_type_error(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject, op: &str) -> u64 {
    let msg = format!(
        "'{}' not supported between instances of '{}' and '{}'",
        op,
        type_name(_py, lhs),
        type_name(_py, rhs),
    );
    return raise_exception::<_>(_py, "TypeError", &msg);
}

#[derive(Clone, Copy)]
enum CompareOutcome {
    Ordered(Ordering),
    Unordered,
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
enum CompareBoolOutcome {
    True,
    False,
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

fn is_number(obj: MoltObject) -> bool {
    to_i64(obj).is_some() || obj.is_float() || bigint_ptr_from_bits(obj.bits()).is_some()
}

fn compare_numbers_outcome(lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    if let Some(ordering) = compare_numbers(lhs, rhs) {
        return CompareOutcome::Ordered(ordering);
    }
    if is_number(lhs) && is_number(rhs) {
        return CompareOutcome::Unordered;
    }
    CompareOutcome::NotComparable
}

unsafe fn compare_string_bytes(lhs_ptr: *mut u8, rhs_ptr: *mut u8) -> Ordering {
    let l_len = string_len(lhs_ptr);
    let r_len = string_len(rhs_ptr);
    let l_bytes = std::slice::from_raw_parts(string_bytes(lhs_ptr), l_len);
    let r_bytes = std::slice::from_raw_parts(string_bytes(rhs_ptr), r_len);
    l_bytes.cmp(r_bytes)
}

unsafe fn compare_bytes_like(lhs_ptr: *mut u8, rhs_ptr: *mut u8) -> Ordering {
    let l_len = bytes_len(lhs_ptr);
    let r_len = bytes_len(rhs_ptr);
    let l_bytes = std::slice::from_raw_parts(bytes_data(lhs_ptr), l_len);
    let r_bytes = std::slice::from_raw_parts(bytes_data(rhs_ptr), r_len);
    l_bytes.cmp(r_bytes)
}

unsafe fn compare_sequence(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
) -> CompareOutcome {
    let lhs = seq_vec_ref(lhs_ptr);
    let rhs = seq_vec_ref(rhs_ptr);
    let common = lhs.len().min(rhs.len());
    for idx in 0..common {
        let l_bits = lhs[idx];
        let r_bits = rhs[idx];
        if obj_eq(_py, obj_from_bits(l_bits), obj_from_bits(r_bits)) {
            continue;
        }
        return compare_objects(_py, obj_from_bits(l_bits), obj_from_bits(r_bits));
    }
    CompareOutcome::Ordered(lhs.len().cmp(&rhs.len()))
}

fn compare_objects_builtin(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    match compare_numbers_outcome(lhs, rhs) {
        CompareOutcome::NotComparable => {}
        outcome => return outcome,
    }
    let (Some(lhs_ptr), Some(rhs_ptr)) = (lhs.as_ptr(), rhs.as_ptr()) else {
        return CompareOutcome::NotComparable;
    };
    unsafe {
        let ltype = object_type_id(lhs_ptr);
        let rtype = object_type_id(rhs_ptr);
        if ltype == TYPE_ID_STRING && rtype == TYPE_ID_STRING {
            return CompareOutcome::Ordered(compare_string_bytes(lhs_ptr, rhs_ptr));
        }
        if (ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY)
            && (rtype == TYPE_ID_BYTES || rtype == TYPE_ID_BYTEARRAY)
        {
            return CompareOutcome::Ordered(compare_bytes_like(lhs_ptr, rhs_ptr));
        }
        if ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST {
            return compare_sequence(_py, lhs_ptr, rhs_ptr);
        }
        if ltype == TYPE_ID_TUPLE && rtype == TYPE_ID_TUPLE {
            return compare_sequence(_py, lhs_ptr, rhs_ptr);
        }
    }
    CompareOutcome::NotComparable
}

fn ordering_matches(ordering: Ordering, op: CompareOp) -> bool {
    match op {
        CompareOp::Lt => ordering == Ordering::Less,
        CompareOp::Le => ordering != Ordering::Greater,
        CompareOp::Gt => ordering == Ordering::Greater,
        CompareOp::Ge => ordering != Ordering::Less,
    }
}

fn compare_builtin_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: CompareOp,
) -> CompareBoolOutcome {
    match compare_objects_builtin(_py, lhs, rhs) {
        CompareOutcome::Ordered(ordering) => {
            if ordering_matches(ordering, op) {
                CompareBoolOutcome::True
            } else {
                CompareBoolOutcome::False
            }
        }
        CompareOutcome::Unordered => CompareBoolOutcome::False,
        CompareOutcome::NotComparable => CompareBoolOutcome::NotComparable,
        CompareOutcome::Error => CompareBoolOutcome::Error,
    }
}

fn rich_compare_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> CompareBoolOutcome {
    unsafe {
        if let Some(lhs_ptr) = lhs.as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, lhs_ptr, op_name_bits) {
                let res_bits = call_callable1(_py, call_bits, rhs.bits());
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return CompareBoolOutcome::Error;
                }
                if is_not_implemented_bits(_py, res_bits) {
                    dec_ref_bits(_py, res_bits);
                } else {
                    let truthy = is_truthy(_py, obj_from_bits(res_bits));
                    dec_ref_bits(_py, res_bits);
                    return if truthy {
                        CompareBoolOutcome::True
                    } else {
                        CompareBoolOutcome::False
                    };
                }
            }
            if exception_pending(_py) {
                return CompareBoolOutcome::Error;
            }
        }
        if let Some(rhs_ptr) = rhs.as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, rhs_ptr, reverse_name_bits)
            {
                let res_bits = call_callable1(_py, call_bits, lhs.bits());
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return CompareBoolOutcome::Error;
                }
                if is_not_implemented_bits(_py, res_bits) {
                    dec_ref_bits(_py, res_bits);
                } else {
                    let truthy = is_truthy(_py, obj_from_bits(res_bits));
                    dec_ref_bits(_py, res_bits);
                    return if truthy {
                        CompareBoolOutcome::True
                    } else {
                        CompareBoolOutcome::False
                    };
                }
            }
            if exception_pending(_py) {
                return CompareBoolOutcome::Error;
            }
        }
    }
    CompareBoolOutcome::NotComparable
}

fn rich_compare_order(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    let lt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__");
    let gt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__");
    match rich_compare_bool(_py, lhs, rhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => return CompareOutcome::Ordered(Ordering::Less),
        CompareBoolOutcome::False => {}
        CompareBoolOutcome::NotComparable => return CompareOutcome::NotComparable,
        CompareBoolOutcome::Error => return CompareOutcome::Error,
    }
    match rich_compare_bool(_py, rhs, lhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => CompareOutcome::Ordered(Ordering::Greater),
        CompareBoolOutcome::False => CompareOutcome::Ordered(Ordering::Equal),
        CompareBoolOutcome::NotComparable => CompareOutcome::NotComparable,
        CompareBoolOutcome::Error => CompareOutcome::Error,
    }
}

fn compare_objects(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    match compare_objects_builtin(_py, lhs, rhs) {
        CompareOutcome::NotComparable => {}
        outcome => return outcome,
    }
    rich_compare_order(_py, lhs, rhs)
}

#[no_mangle]
pub extern "C" fn molt_lt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_bool(_py, lhs, rhs, CompareOp::Lt) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let lt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__");
        let gt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__");
        match rich_compare_bool(_py, lhs, rhs, lt_name_bits, gt_name_bits) {
            CompareBoolOutcome::True => MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => compare_type_error(_py, lhs, rhs, "<"),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_le(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_bool(_py, lhs, rhs, CompareOp::Le) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let le_name_bits = intern_static_name(_py, &runtime_state(_py).interned.le_name, b"__le__");
        let ge_name_bits = intern_static_name(_py, &runtime_state(_py).interned.ge_name, b"__ge__");
        match rich_compare_bool(_py, lhs, rhs, le_name_bits, ge_name_bits) {
            CompareBoolOutcome::True => MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => compare_type_error(_py, lhs, rhs, "<="),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_gt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_bool(_py, lhs, rhs, CompareOp::Gt) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let gt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__");
        let lt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__");
        match rich_compare_bool(_py, lhs, rhs, gt_name_bits, lt_name_bits) {
            CompareBoolOutcome::True => MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => compare_type_error(_py, lhs, rhs, ">"),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_ge(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_bool(_py, lhs, rhs, CompareOp::Ge) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let ge_name_bits = intern_static_name(_py, &runtime_state(_py).interned.ge_name, b"__ge__");
        let le_name_bits = intern_static_name(_py, &runtime_state(_py).interned.le_name, b"__le__");
        match rich_compare_bool(_py, lhs, rhs, ge_name_bits, le_name_bits) {
            CompareBoolOutcome::True => MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => compare_type_error(_py, lhs, rhs, ">="),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let eq_name_bits = intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
        match rich_compare_bool(_py, lhs, rhs, eq_name_bits, eq_name_bits) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        MoltObject::from_bool(obj_eq(_py, lhs, rhs)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_ne(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let ne_name_bits = intern_static_name(_py, &runtime_state(_py).interned.ne_name, b"__ne__");
        let default_ne_bits = builtin_func_bits(
            _py,
            &runtime_state(_py).method_cache.object_ne,
            fn_addr!(molt_object_ne),
            2,
        );
        let mut saw_explicit = false;
        unsafe {
            if let Some(lhs_ptr) = lhs.as_ptr() {
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, lhs_ptr, ne_name_bits) {
                    let is_default = call_bits == default_ne_bits;
                    let res_bits = call_callable1(_py, call_bits, rhs.bits());
                    dec_ref_bits(_py, call_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::none().bits();
                    }
                    if is_not_implemented_bits(_py, res_bits) {
                        dec_ref_bits(_py, res_bits);
                        if !is_default {
                            saw_explicit = true;
                        }
                    } else {
                        let truthy = is_truthy(_py, obj_from_bits(res_bits));
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::from_bool(truthy).bits();
                    }
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
            if let Some(rhs_ptr) = rhs.as_ptr() {
                if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, rhs_ptr, ne_name_bits) {
                    let is_default = call_bits == default_ne_bits;
                    let res_bits = call_callable1(_py, call_bits, lhs.bits());
                    dec_ref_bits(_py, call_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::none().bits();
                    }
                    if is_not_implemented_bits(_py, res_bits) {
                        dec_ref_bits(_py, res_bits);
                        if !is_default {
                            saw_explicit = true;
                        }
                    } else {
                        let truthy = is_truthy(_py, obj_from_bits(res_bits));
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::from_bool(truthy).bits();
                    }
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        if saw_explicit {
            return MoltObject::from_bool(a != b).bits();
        }
        let eq_bits = molt_eq(a, b);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        molt_not(eq_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_string_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let Some(lp) = lhs.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        let Some(rp) = rhs.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(lp) != TYPE_ID_STRING || object_type_id(rp) != TYPE_ID_STRING {
                return MoltObject::from_bool(false).bits();
            }
            if lp == rp {
                return MoltObject::from_bool(true).bits();
            }
            let l_len = string_len(lp);
            let r_len = string_len(rp);
            if l_len != r_len {
                return MoltObject::from_bool(false).bits();
            }
            let l_bytes = std::slice::from_raw_parts(string_bytes(lp), l_len);
            let r_bytes = std::slice::from_raw_parts(string_bytes(rp), r_len);
            MoltObject::from_bool(l_bytes == r_bytes).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_is(a: u64, b: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(a == b).bits() })
}

#[no_mangle]
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

#[no_mangle]
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
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
    } else if base_val == 2 {
        if let Some(rest) = digits
            .strip_prefix("0b")
            .or_else(|| digits.strip_prefix("0B"))
        {
            digits = rest;
        }
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
#[no_mangle]
pub unsafe extern "C" fn molt_bigint_from_str(ptr: *const u8, len_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let len = usize_from_bits(len_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        let bytes = std::slice::from_raw_parts(ptr, len);
        let text = match std::str::from_utf8(bytes) {
            Ok(val) => val,
            Err(_) => return raise_exception::<_>(_py, "ValueError", "invalid literal for int()"),
        };
        let (parsed, _base_used) = match parse_int_from_str(text, 10) {
            Ok(val) => val,
            Err(_) => return raise_exception::<_>(_py, "ValueError", "invalid literal for int()"),
        };
        if let Some(i) = bigint_to_inline(&parsed) {
            return MoltObject::from_int(i).bits();
        }
        bigint_bits(_py, parsed)
    })
}

#[no_mangle]
pub extern "C" fn molt_float_from_obj(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val_bits);
        if obj.is_float() {
            return val_bits;
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
        return raise_exception::<_>(
            _py,
            "TypeError",
            "float() argument must be a string or a number",
        );
    })
}

#[no_mangle]
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
            return raise_exception::<_>(_py, "ValueError", &msg);
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
        return raise_exception::<_>(
            _py,
            "TypeError",
            "int() argument must be a string or a number",
        );
    })
}

#[no_mangle]
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
            return raise_exception::<_>(_py, "TypeError", "type guard mismatch");
        }
        val_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_is_truthy(val: u64) -> i64 {
    crate::with_gil_entry!(_py, {
        if is_truthy(_py, obj_from_bits(val)) {
            1
        } else {
            0
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_not(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_bool(!is_truthy(_py, obj_from_bits(val))).bits()
    })
}

#[no_mangle]
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
        let async_polls = ASYNC_POLL_COUNT.load(AtomicOrdering::Relaxed);
        let async_pending = ASYNC_PENDING_COUNT.load(AtomicOrdering::Relaxed);
        let async_wakeups = ASYNC_WAKEUP_COUNT.load(AtomicOrdering::Relaxed);
        let async_sleep_reg = ASYNC_SLEEP_REGISTER_COUNT.load(AtomicOrdering::Relaxed);
        eprintln!(
        "molt_profile call_dispatch={} string_count_cache_hit={} string_count_cache_miss={} struct_field_store={} attr_lookup={} handle_resolve={} layout_guard={} layout_guard_fail={} alloc_count={} async_polls={} async_pending={} async_wakeups={} async_sleep_register={}",
        call_dispatch,
        cache_hit,
        cache_miss,
        struct_stores,
        attr_lookups,
        handle_resolves,
        layout_guard,
        layout_guard_fail,
        allocs,
        async_polls,
        async_pending,
        async_wakeups,
        async_sleep_reg
    );
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
    if prod == 1 {
        if let Some(result) = prod_ints_unboxed_trivial(elems) {
            return result;
        }
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
    max_ints_trusted_scalar(elems, acc)
}

#[no_mangle]
pub extern "C" fn molt_vec_sum_int(seq_bits: u64, acc_bits: u64) -> u64 {
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
            if let Some(sum) = sum_ints_checked(elems, acc) {
                return vec_sum_result(_py, MoltObject::from_int(sum).bits(), true);
            }
        }
        vec_sum_result(_py, MoltObject::from_int(acc).bits(), false)
    })
}

#[no_mangle]
pub extern "C" fn molt_vec_sum_int_trusted(seq_bits: u64, acc_bits: u64) -> u64 {
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
            let sum = sum_ints_trusted(elems, acc);
            vec_sum_result(_py, MoltObject::from_int(sum).bits(), true)
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
            if let Some(sum) = sum_ints_checked(slice, acc) {
                return vec_sum_result(_py, MoltObject::from_int(sum).bits(), true);
            }
        }
        vec_sum_result(_py, MoltObject::from_int(acc).bits(), false)
    })
}

#[no_mangle]
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
            let sum = sum_ints_trusted(slice, acc);
            vec_sum_result(_py, MoltObject::from_int(sum).bits(), true)
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
        SliceError::Type => {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "slice indices must be integers or None or have an __index__ method",
            );
        }
        SliceError::Value => {
            return raise_exception::<_>(_py, "ValueError", "slice step cannot be zero");
        }
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

fn collect_bytearray_assign_bytes(_py: &PyToken<'_>, bits: u64) -> Option<Vec<u8>> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                return Some(bytes_like_slice_raw(ptr).unwrap_or(&[]).to_vec());
            }
            if type_id == TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "can assign only bytes, buffers, or iterables of ints in range(0, 256)",
                );
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if let Some(slice) = memoryview_bytes_slice(ptr) {
                    return Some(slice.to_vec());
                }
                return memoryview_collect_bytes(ptr);
            }
        }
    }
    let iter_bits = molt_iter(bits);
    if obj_from_bits(iter_bits).is_none() {
        if exception_pending(_py) {
            return None;
        }
        return raise_exception::<_>(
            _py,
            "TypeError",
            "can assign only bytes, buffers, or iterables of ints in range(0, 256)",
        );
    }
    bytes_collect_from_iter(_py, iter_bits, BytesCtorKind::Bytearray)
}

#[no_mangle]
pub extern "C" fn molt_len(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    return MoltObject::from_int(string_len(ptr) as i64).bits();
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
                    let len = range_len_i64(range_start(ptr), range_stop(ptr), range_step(ptr));
                    return MoltObject::from_int(len).bits();
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
        return raise_exception::<_>(_py, "TypeError", &msg);
    })
}

#[no_mangle]
pub extern "C" fn molt_hash_builtin(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hash = hash_bits_signed(_py, val);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        int_bits_from_i64(_py, hash)
    })
}

#[no_mangle]
pub extern "C" fn molt_id(val: u64) -> u64 {
    crate::with_gil_entry!(_py, { int_bits_from_i64(_py, val as i64) })
}

fn ord_length_error(_py: &PyToken<'_>, len: usize) -> u64 {
    let msg = format!("ord() expected a character, but string of length {len} found");
    return raise_exception::<_>(_py, "TypeError", &msg);
}

#[no_mangle]
pub extern "C" fn molt_ord(val: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let Ok(s) = std::str::from_utf8(bytes) else {
                        return MoltObject::none().bits();
                    };
                    let char_count = s.chars().count();
                    if char_count != 1 {
                        return ord_length_error(_py, char_count);
                    }
                    let ch = s.chars().next().unwrap();
                    return MoltObject::from_int(ch as i64).bits();
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
        return raise_exception::<_>(_py, "TypeError", &msg);
    })
}

#[no_mangle]
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
        let Some(ch) = std::char::from_u32(code) else {
            return raise_exception::<_>(_py, "ValueError", "chr() arg not in range(0x110000)");
        };
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        let out = alloc_string(_py, s.as_bytes());
        if out.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(out).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_missing() -> u64 {
    crate::with_gil_entry!(_py, {
        let bits = missing_bits(_py);
        inc_ref_bits(_py, bits);
        bits
    })
}

#[no_mangle]
pub extern "C" fn molt_not_implemented() -> u64 {
    crate::with_gil_entry!(_py, { not_implemented_bits(_py) })
}

#[no_mangle]
pub extern "C" fn molt_ellipsis() -> u64 {
    crate::with_gil_entry!(_py, { ellipsis_bits(_py) })
}

#[no_mangle]
pub extern "C" fn molt_getrecursionlimit() -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_int(recursion_limit_get() as i64).bits()
    })
}

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_getargv() -> u64 {
    crate::with_gil_entry!(_py, {
        let args = runtime_state(_py).argv.lock().unwrap();
        let mut elems = Vec::with_capacity(args.len());
        for arg in args.iter() {
            let ptr = alloc_string(_py, arg.as_bytes());
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

#[no_mangle]
/// # Safety
/// Caller must ensure `argv` points to `argc` null-terminated strings.
pub unsafe extern "C" fn molt_set_argv(argc: i32, argv: *const *const u8) {
    crate::with_gil_entry!(_py, {
        let mut args = Vec::new();
        if argc > 0 && !argv.is_null() {
            for idx in 0..argc {
                let ptr = *argv.add(idx as usize);
                if ptr.is_null() {
                    args.push(String::new());
                    continue;
                }
                // TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial):
                // decode argv using filesystem encoding + surrogateescape once Molt strings support
                // surrogate escapes.
                let bytes = CStr::from_ptr(ptr as *const i8).to_bytes();
                args.push(String::from_utf8_lossy(bytes).into_owned());
            }
        }
        *runtime_state(_py).argv.lock().unwrap() = args;
    })
}

#[cfg(target_os = "windows")]
#[no_mangle]
/// # Safety
/// Caller must ensure `argv` points to `argc` null-terminated UTF-16 strings.
pub unsafe extern "C" fn molt_set_argv_utf16(argc: i32, argv: *const *const u16) {
    crate::with_gil_entry!(_py, {
        let mut args = Vec::new();
        if argc > 0 && !argv.is_null() {
            for idx in 0..argc {
                let ptr = *argv.add(idx as usize);
                if ptr.is_null() {
                    args.push(String::new());
                    continue;
                }
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                // TODO(stdlib-compat, owner:runtime, milestone:SL1, priority:P2, status:partial):
                // preserve invalid UTF-16 data once Molt strings can represent surrogate escapes.
                args.push(String::from_utf16_lossy(slice));
            }
        }
        *runtime_state(_py).argv.lock().unwrap() = args;
    })
}

#[no_mangle]
pub extern "C" fn molt_getpid() -> u64 {
    crate::with_gil_entry!(_py, {
        #[cfg(target_arch = "wasm32")]
        {
            // TODO(wasm-parity, owner:runtime, milestone:SL2, priority:P2, status:planned):
            // decide on a stable WASM getpid shim (host-provided or documented 0 placeholder).
            MoltObject::from_int(0).bits()
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            MoltObject::from_int(std::process::id() as i64).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_time_monotonic() -> u64 {
    crate::with_gil_entry!(_py, {
        MoltObject::from_float(monotonic_now_secs(_py)).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_time_monotonic_ns() -> u64 {
    crate::with_gil_entry!(_py, {
        int_bits_from_bigint(_py, BigInt::from(monotonic_now_nanos(_py)))
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_recursion_guard_enter() -> i64 {
    crate::with_gil_entry!(_py, {
        if recursion_guard_enter() {
            1
        } else {
            raise_exception::<i64>(_py, "RecursionError", "maximum recursion depth exceeded")
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_recursion_guard_exit() {
    crate::with_gil_entry!(_py, {
        recursion_guard_exit();
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
                        if let Some(bound_ptr) = obj_from_bits(bound_func_bits).as_ptr() {
                            if object_type_id(bound_ptr) == TYPE_ID_FUNCTION {
                                code_bits = ensure_function_code_bits(_py, bound_ptr);
                            }
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

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_trace_exit() -> u64 {
    crate::with_gil_entry!(_py, {
        frame_stack_pop(_py);
        MoltObject::none().bits()
    })
}

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_repr_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_repr_from_obj(val_bits) })
}

#[no_mangle]
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
                    if class_bits != 0 {
                        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
                            if object_type_id(class_ptr) == TYPE_ID_TYPE {
                                let format_bits = intern_static_name(
                                    _py,
                                    &runtime_state(_py).interned.format_name,
                                    b"__format__",
                                );
                                if let Some(call_bits) = class_attr_lookup(
                                    _py,
                                    class_ptr,
                                    class_ptr,
                                    Some(obj_ptr),
                                    format_bits,
                                ) {
                                    return call_callable1(_py, call_bits, spec_bits);
                                }
                            }
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
                .unwrap_or(false);
        if supports_format {
            return molt_string_format(val_bits, spec_bits);
        }
        if spec_text.is_empty() {
            return molt_str_from_obj(val_bits);
        }
        let type_label = type_name(_py, obj);
        let msg = format!("unsupported format string passed to {type_label}.__format__");
        return raise_exception::<_>(_py, "TypeError", &msg);
    })
}

#[no_mangle]
pub extern "C" fn molt_callable_builtin(val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_is_callable(val_bits) })
}

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_enumerate_builtin(iter_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let has_start = start_bits != missing;
        let start = if has_start {
            start_bits
        } else {
            MoltObject::from_int(0).bits()
        };
        let has_start_bits = MoltObject::from_bool(has_start).bits();
        molt_enumerate(iter_bits, start, has_start_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_next_builtin(iter_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        let pair_bits = molt_iter_next(iter_bits);
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
                if default_bits != missing {
                    inc_ref_bits(_py, default_bits);
                    return default_bits;
                }
                if obj_from_bits(val_bits).is_none() {
                    return raise_exception::<_>(_py, "StopIteration", "");
                }
                let msg_bits = molt_str_from_obj(val_bits);
                let msg = string_obj_to_owned(obj_from_bits(msg_bits)).unwrap_or_default();
                dec_ref_bits(_py, msg_bits);
                return raise_exception::<_>(_py, "StopIteration", &msg);
            }
            inc_ref_bits(_py, val_bits);
            val_bits
        }
    })
}

#[no_mangle]
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
                    return MoltObject::from_bool(false).bits();
                }
                if is_truthy(_py, obj_from_bits(val_bits)) {
                    return MoltObject::from_bool(true).bits();
                }
            }
        }
    })
}

#[no_mangle]
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
                    return MoltObject::from_bool(true).bits();
                }
                if !is_truthy(_py, obj_from_bits(val_bits)) {
                    return MoltObject::from_bool(false).bits();
                }
            }
        }
    })
}

#[no_mangle]
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
        if let Some(ptr) = maybe_ptr_from_bits(val_bits) {
            if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__abs__") {
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
        }
        let type_name = class_name_for_error(type_of_bits(_py, val_bits));
        let msg = format!("bad operand type for abs(): '{type_name}'");
        return raise_exception::<_>(_py, "TypeError", &msg);
    })
}

#[no_mangle]
pub extern "C" fn molt_divmod_builtin(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let lhs = obj_from_bits(a_bits);
        let rhs = obj_from_bits(b_bits);
        if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
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
        if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
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
        return raise_exception::<_>(_py, "TypeError", &msg);
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

#[no_mangle]
pub extern "C" fn molt_min_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, false, "min")
    })
}

#[no_mangle]
pub extern "C" fn molt_max_builtin(args_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        molt_minmax_builtin(_py, args_bits, key_bits, default_bits, true, "max")
    })
}

#[no_mangle]
pub extern "C" fn molt_map_builtin(func_bits: u64, iterables_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iterables_obj = obj_from_bits(iterables_bits);
        let Some(iterables_ptr) = iterables_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "map expects a tuple");
        };
        unsafe {
            if object_type_id(iterables_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "map expects a tuple");
            }
            let iterables = seq_vec_ref(iterables_ptr);
            if iterables.is_empty() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "map() must have at least two arguments",
                );
            }
            let mut iters = Vec::with_capacity(iterables.len());
            for &iterable_bits in iterables.iter() {
                let iter_bits = molt_iter(iterable_bits);
                if obj_from_bits(iter_bits).is_none() {
                    return raise_not_iterable(_py, iterable_bits);
                }
                iters.push(iter_bits);
            }
            let total = std::mem::size_of::<MoltHeader>()
                + std::mem::size_of::<u64>()
                + std::mem::size_of::<*mut Vec<u64>>();
            let map_ptr = alloc_object(_py, total, TYPE_ID_MAP);
            if map_ptr.is_null() {
                for iter_bits in iters {
                    dec_ref_bits(_py, iter_bits);
                }
                return MoltObject::none().bits();
            }
            let iters_ptr = Box::into_raw(Box::new(iters));
            *(map_ptr as *mut u64) = func_bits;
            *(map_ptr.add(std::mem::size_of::<u64>()) as *mut *mut Vec<u64>) = iters_ptr;
            inc_ref_bits(_py, func_bits);
            MoltObject::from_ptr(map_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_filter_builtin(func_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
        let filter_ptr = alloc_object(_py, total, TYPE_ID_FILTER);
        if filter_ptr.is_null() {
            dec_ref_bits(_py, iter_bits);
            return MoltObject::none().bits();
        }
        unsafe {
            *(filter_ptr as *mut u64) = func_bits;
            *(filter_ptr.add(std::mem::size_of::<u64>()) as *mut u64) = iter_bits;
        }
        inc_ref_bits(_py, func_bits);
        MoltObject::from_ptr(filter_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_zip_builtin(iterables_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iterables_obj = obj_from_bits(iterables_bits);
        let Some(iterables_ptr) = iterables_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "zip expects a tuple");
        };
        unsafe {
            if object_type_id(iterables_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "zip expects a tuple");
            }
            let iterables = seq_vec_ref(iterables_ptr);
            let mut iters = Vec::with_capacity(iterables.len());
            for &iterable_bits in iterables.iter() {
                let iter_bits = molt_iter(iterable_bits);
                if obj_from_bits(iter_bits).is_none() {
                    return raise_not_iterable(_py, iterable_bits);
                }
                iters.push(iter_bits);
            }
            let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<*mut Vec<u64>>();
            let zip_ptr = alloc_object(_py, total, TYPE_ID_ZIP);
            if zip_ptr.is_null() {
                for iter_bits in iters {
                    dec_ref_bits(_py, iter_bits);
                }
                return MoltObject::none().bits();
            }
            let iters_ptr = Box::into_raw(Box::new(iters));
            *(zip_ptr as *mut *mut Vec<u64>) = iters_ptr;
            MoltObject::from_ptr(zip_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_reversed_builtin(seq_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(seq_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    let idx = dict_len(dict_ptr);
                    let total = std::mem::size_of::<MoltHeader>()
                        + std::mem::size_of::<u64>()
                        + std::mem::size_of::<usize>();
                    let rev_ptr = alloc_object(_py, total, TYPE_ID_REVERSED);
                    if rev_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, dict_bits);
                    *(rev_ptr as *mut u64) = dict_bits;
                    reversed_set_index(rev_ptr, idx);
                    return MoltObject::from_ptr(rev_ptr).bits();
                }
                if type_id == TYPE_ID_LIST
                    || type_id == TYPE_ID_TUPLE
                    || type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                    || type_id == TYPE_ID_RANGE
                    || type_id == TYPE_ID_DICT
                    || type_id == TYPE_ID_DICT_KEYS_VIEW
                    || type_id == TYPE_ID_DICT_VALUES_VIEW
                    || type_id == TYPE_ID_DICT_ITEMS_VIEW
                {
                    let idx = if type_id == TYPE_ID_STRING {
                        string_len(ptr)
                    } else if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                        bytes_len(ptr)
                    } else if type_id == TYPE_ID_DICT {
                        dict_order(ptr).len() / 2
                    } else if type_id == TYPE_ID_DICT_KEYS_VIEW
                        || type_id == TYPE_ID_DICT_VALUES_VIEW
                        || type_id == TYPE_ID_DICT_ITEMS_VIEW
                    {
                        dict_view_len(ptr)
                    } else if type_id == TYPE_ID_RANGE {
                        range_len_i64(range_start(ptr), range_stop(ptr), range_step(ptr)) as usize
                    } else if type_id == TYPE_ID_LIST {
                        list_len(ptr)
                    } else {
                        tuple_len(ptr)
                    };
                    let total = std::mem::size_of::<MoltHeader>()
                        + std::mem::size_of::<u64>()
                        + std::mem::size_of::<usize>();
                    let rev_ptr = alloc_object(_py, total, TYPE_ID_REVERSED);
                    if rev_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, seq_bits);
                    *(rev_ptr as *mut u64) = seq_bits;
                    reversed_set_index(rev_ptr, idx);
                    return MoltObject::from_ptr(rev_ptr).bits();
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__reversed__") {
                    if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        let res = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        return res;
                    }
                    dec_ref_bits(_py, name_bits);
                }
            }
        }
        let msg = format!("'{}' object is not reversible", type_name(_py, obj));
        return raise_exception::<_>(_py, "TypeError", &msg);
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

#[no_mangle]
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
                return MoltObject::none().bits();
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

#[no_mangle]
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
        let iter_obj = molt_iter(iter_bits);
        if obj_from_bits(iter_obj).is_none() {
            return raise_not_iterable(_py, iter_bits);
        }
        let mut total_bits = start_bits;
        let mut total_owned = false;
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

#[no_mangle]
pub extern "C" fn molt_getattr_builtin(obj_bits: u64, name_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if default_bits == missing {
            return molt_get_attr_name(obj_bits, name_bits);
        }
        molt_get_attr_name_default(obj_bits, name_bits, default_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_vars_builtin(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let missing = missing_bits(_py);
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

#[no_mangle]
pub extern "C" fn molt_dir_builtin(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mut names: Vec<u64> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let _obj = obj_from_bits(obj_bits);
        if let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) {
            unsafe {
                if let Some(dir_bits) = attr_name_bits_from_bytes(_py, b"__dir__") {
                    if let Some(method_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, dir_bits)
                    {
                        let res_bits = call_callable0(_py, method_bits);
                        dec_ref_bits(_py, method_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        return res_bits;
                    }
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
        let list_ptr = alloc_list(_py, &names);
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

#[no_mangle]
pub extern "C" fn molt_object_init(_self_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::none().bits() })
}

#[no_mangle]
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
                        return attr_error(_py, type_label, &attr_name) as u64;
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
                    return attr_error(_py, type_label, &attr_name) as u64;
                }
                if type_id == TYPE_ID_TYPE {
                    let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(obj_ptr)))
                        .unwrap_or_default();
                    let msg = format!("type object '{class_name}' has no attribute '{attr_name}'");
                    return raise_exception::<_>(_py, "AttributeError", &msg);
                }
                return attr_error(
                    _py,
                    type_name(_py, MoltObject::from_ptr(obj_ptr)),
                    &attr_name,
                ) as u64;
            }
            let obj = obj_from_bits(obj_bits);
            attr_error(_py, type_name(_py, obj), &attr_name) as u64
        }
    })
}

#[no_mangle]
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
                let res = if type_id == TYPE_ID_OBJECT {
                    object_setattr_raw(_py, obj_ptr, attr_bits, &attr_name, val_bits)
                } else if type_id == TYPE_ID_DATACLASS {
                    dataclass_setattr_raw(_py, obj_ptr, attr_bits, &attr_name, val_bits)
                } else {
                    let bytes = string_bytes(name_ptr);
                    let len = string_len(name_ptr);
                    molt_set_attr_generic(obj_ptr, bytes, len as u64, val_bits)
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

#[no_mangle]
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
                let res = if type_id == TYPE_ID_OBJECT {
                    object_delattr_raw(_py, obj_ptr, attr_bits, &attr_name)
                } else if type_id == TYPE_ID_DATACLASS {
                    dataclass_delattr_raw(_py, obj_ptr, attr_bits, &attr_name)
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

#[no_mangle]
pub extern "C" fn molt_object_eq(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_bits == other_bits {
            return MoltObject::from_bool(true).bits();
        }
        not_implemented_bits(_py)
    })
}

#[no_mangle]
pub extern "C" fn molt_object_ne(self_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if self_bits == other_bits {
            return MoltObject::from_bool(false).bits();
        }
        not_implemented_bits(_py)
    })
}

#[no_mangle]
pub extern "C" fn molt_anext_builtin(iter_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        if default_bits == missing {
            return molt_anext(iter_bits);
        }
        let obj_bits = molt_alloc(3 * std::mem::size_of::<u64>() as u64);
        let Some(obj_ptr) = resolve_obj_ptr(obj_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let header = header_from_obj_ptr(obj_ptr);
            (*header).poll_fn = anext_default_poll_fn_addr();
            (*header).state = 0;
            let payload_ptr = obj_ptr as *mut u64;
            *payload_ptr = iter_bits;
            inc_ref_bits(_py, iter_bits);
            *payload_ptr.add(1) = default_bits;
            inc_ref_bits(_py, default_bits);
            *payload_ptr.add(2) = MoltObject::none().bits();
        }
        obj_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_print_builtin(
    args_bits: u64,
    sep_bits: u64,
    end_bits: u64,
    file_bits: u64,
    flush_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        fn print_string_arg(
            _py: &PyToken<'_>,
            bits: u64,
            default: &str,
            label: &str,
        ) -> Option<String> {
            let obj = obj_from_bits(bits);
            if obj.is_none() {
                return Some(default.to_string());
            }
            if let Some(val) = string_obj_to_owned(obj) {
                return Some(val);
            }
            let msg = format!(
                "{} must be None or a string, not {}",
                label,
                type_name(_py, obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }

        let args_obj = obj_from_bits(args_bits);
        let Some(args_ptr) = args_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "print expects a tuple");
        };
        unsafe {
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "print expects a tuple");
            }
            let Some(sep) = print_string_arg(_py, sep_bits, " ", "sep") else {
                return MoltObject::none().bits();
            };
            let Some(end) = print_string_arg(_py, end_bits, "\n", "end") else {
                return MoltObject::none().bits();
            };

            let mut resolved_file_bits = file_bits;
            let mut sys_found = false;
            let mut file_from_sys = false;
            if obj_from_bits(resolved_file_bits).is_none() {
                // TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial):
                // ensure sys is initialized so print(file=None) always honors sys.stdout.
                let sys_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.sys_name, b"sys");
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

            let elems = seq_vec_ref(args_ptr);
            // TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned):
            // stream writes to avoid building an intermediate output string for large print payloads.
            let mut output = String::new();
            for (idx, &val_bits) in elems.iter().enumerate() {
                if idx > 0 {
                    output.push_str(&sep);
                }
                let str_bits = molt_str_from_obj(val_bits);
                if exception_pending(_py) {
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
                let Some(text) = string_obj_to_owned(obj_from_bits(str_bits)) else {
                    dec_ref_bits(_py, str_bits);
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                };
                output.push_str(&text);
                dec_ref_bits(_py, str_bits);
            }
            output.push_str(&end);

            let do_flush = is_truthy(_py, obj_from_bits(flush_bits));

            if obj_from_bits(resolved_file_bits).is_none() && !sys_found {
                print!("{output}");
                if do_flush {
                    let _ = std::io::stdout().flush();
                }
                return MoltObject::none().bits();
            }

            let out_ptr = alloc_string(_py, output.as_bytes());
            if out_ptr.is_null() {
                if file_from_sys {
                    dec_ref_bits(_py, resolved_file_bits);
                }
                return MoltObject::none().bits();
            }
            let out_bits = MoltObject::from_ptr(out_ptr).bits();
            if let Some(ptr) = obj_from_bits(resolved_file_bits).as_ptr() {
                if object_type_id(ptr) == TYPE_ID_FILE_HANDLE {
                    let _ = molt_file_write(resolved_file_bits, out_bits);
                    dec_ref_bits(_py, out_bits);
                    if do_flush {
                        let _ = molt_file_flush(resolved_file_bits);
                    }
                    if file_from_sys {
                        dec_ref_bits(_py, resolved_file_bits);
                    }
                    return MoltObject::none().bits();
                }
            }
            let write_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.write_name, b"write");
            let write_bits = molt_get_attr_name(resolved_file_bits, write_name_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, out_bits);
                if file_from_sys {
                    dec_ref_bits(_py, resolved_file_bits);
                }
                return MoltObject::none().bits();
            }
            let res_bits = call_callable1(_py, write_bits, out_bits);
            dec_ref_bits(_py, write_bits);
            dec_ref_bits(_py, out_bits);
            dec_ref_bits(_py, res_bits);
            if exception_pending(_py) {
                if file_from_sys {
                    dec_ref_bits(_py, resolved_file_bits);
                }
                return MoltObject::none().bits();
            }
            if do_flush {
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
            if file_from_sys {
                dec_ref_bits(_py, resolved_file_bits);
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_super_builtin(type_bits: u64, obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_super_new(type_bits, obj_bits) })
}

#[no_mangle]
pub extern "C" fn molt_slice(obj_bits: u64, start_bits: u64, end_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let start_obj = obj_from_bits(start_bits);
        let end_obj = obj_from_bits(end_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let len = string_len(ptr) as isize;
                    let start = match decode_slice_bound(_py, start_obj, len, 0) {
                        Ok(v) => v,
                        Err(err) => return slice_error(_py, err),
                    };
                    let end = match decode_slice_bound(_py, end_obj, len, len) {
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
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), len as usize);
                    let slice = &bytes[start as usize..end as usize];
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
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_string_find(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_string_find_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_string_rfind(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_string_rfind_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_string_find_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!("must be str, not {}", type_name(_py, needle));
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!("must be str, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let hay_len = string_len(hay_ptr);
                let needle_len = string_len(needle_ptr);
                let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), hay_len);
                let needle_bytes = std::slice::from_raw_parts(string_bytes(needle_ptr), needle_len);
                let total_chars =
                    utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize));
                let (start, end, start_raw) = slice_bounds_from_args(
                    _py,
                    start_bits,
                    end_bits,
                    has_start,
                    has_end,
                    total_chars,
                );
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                if needle_bytes.is_empty() {
                    if start_raw > total_chars {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(start).bits();
                }
                let start_byte =
                    utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize));
                let end_byte =
                    utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                        .min(hay_bytes.len());
                let slice = &hay_bytes[start_byte..end_byte];
                let idx = bytes_find_impl(slice, needle_bytes);
                if idx < 0 {
                    return MoltObject::from_int(-1).bits();
                }
                if hay_bytes.is_ascii() && needle_bytes.is_ascii() {
                    return MoltObject::from_int(start + idx).bits();
                }
                let byte_idx = start_byte + idx as usize;
                let char_idx = utf8_byte_to_char_index_cached(
                    _py,
                    hay_bytes,
                    byte_idx,
                    Some(hay_ptr as usize),
                );
                MoltObject::from_int(char_idx).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_rfind_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!("must be str, not {}", type_name(_py, needle));
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!("must be str, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let hay_len = string_len(hay_ptr);
                let needle_len = string_len(needle_ptr);
                let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), hay_len);
                let needle_bytes = std::slice::from_raw_parts(string_bytes(needle_ptr), needle_len);
                let total_chars =
                    utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize));
                let (start, end, start_raw) = slice_bounds_from_args(
                    _py,
                    start_bits,
                    end_bits,
                    has_start,
                    has_end,
                    total_chars,
                );
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                if needle_bytes.is_empty() {
                    if start_raw > total_chars {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(end).bits();
                }
                let start_byte =
                    utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize));
                let end_byte =
                    utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                        .min(hay_bytes.len());
                let slice = &hay_bytes[start_byte..end_byte];
                let idx = bytes_rfind_impl(slice, needle_bytes);
                if idx < 0 {
                    return MoltObject::from_int(-1).bits();
                }
                if hay_bytes.is_ascii() && needle_bytes.is_ascii() {
                    return MoltObject::from_int(start + idx).bits();
                }
                let byte_idx = start_byte + idx as usize;
                let char_idx = utf8_byte_to_char_index_cached(
                    _py,
                    hay_bytes,
                    byte_idx,
                    Some(hay_ptr as usize),
                );
                MoltObject::from_int(char_idx).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

fn partition_string_bytes(
    _py: &PyToken<'_>,
    hay_bytes: &[u8],
    sep_bytes: &[u8],
    from_right: bool,
) -> Option<u64> {
    let idx = if from_right {
        bytes_rfind_impl(hay_bytes, sep_bytes)
    } else {
        bytes_find_impl(hay_bytes, sep_bytes)
    };
    let (head_bytes, sep_bytes, tail_bytes) = if idx < 0 {
        if from_right {
            (&[][..], &[][..], hay_bytes)
        } else {
            (hay_bytes, &[][..], &[][..])
        }
    } else {
        let idx = idx as usize;
        let end = idx + sep_bytes.len();
        (&hay_bytes[..idx], sep_bytes, &hay_bytes[end..])
    };
    let head_ptr = alloc_string(_py, head_bytes);
    if head_ptr.is_null() {
        return None;
    }
    let head_bits = MoltObject::from_ptr(head_ptr).bits();
    let sep_ptr = alloc_string(_py, sep_bytes);
    if sep_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        return None;
    }
    let sep_bits = MoltObject::from_ptr(sep_ptr).bits();
    let tail_ptr = alloc_string(_py, tail_bytes);
    if tail_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        dec_ref_bits(_py, sep_bits);
        return None;
    }
    let tail_bits = MoltObject::from_ptr(tail_ptr).bits();
    let tuple_ptr = alloc_tuple(_py, &[head_bits, sep_bits, tail_bits]);
    if tuple_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        dec_ref_bits(_py, sep_bits);
        dec_ref_bits(_py, tail_bits);
        return None;
    }
    Some(MoltObject::from_ptr(tuple_ptr).bits())
}

#[no_mangle]
pub extern "C" fn molt_string_partition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!("must be str, not {}", type_name(_py, sep));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(sep_ptr) != TYPE_ID_STRING {
                let msg = format!("must be str, not {}", type_name(_py, sep));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let tuple_bits = partition_string_bytes(_py, hay_bytes, sep_bytes, false);
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_rpartition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!("must be str, not {}", type_name(_py, sep));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(sep_ptr) != TYPE_ID_STRING {
                let msg = format!("must be str, not {}", type_name(_py, sep));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let tuple_bits = partition_string_bytes(_py, hay_bytes, sep_bytes, true);
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
// TODO(type-coverage, owner:runtime, milestone:TC2, priority:P1, status:partial): implement str.isdigit parity.
pub extern "C" fn molt_string_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_string_startswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_string_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_string_endswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_string_startswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let total_chars = utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize));
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total_chars);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let start_byte =
                utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize));
            let end_byte =
                utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                    .min(hay_bytes.len());
            let slice = &hay_bytes[start_byte..end_byte];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_STRING {
                    let needle_bytes = std::slice::from_raw_parts(
                        string_bytes(needle_ptr),
                        string_len(needle_ptr),
                    );
                    let ok = slice_match(slice, needle_bytes, start_raw, total_chars, false);
                    return MoltObject::from_bool(ok).bits();
                }
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "tuple for startswith must only contain str, not {}",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if object_type_id(elem_ptr) != TYPE_ID_STRING {
                            let msg = format!(
                                "tuple for startswith must only contain str, not {}",
                                type_name(_py, elem)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        let needle_bytes = std::slice::from_raw_parts(
                            string_bytes(elem_ptr),
                            string_len(elem_ptr),
                        );
                        if slice_match(slice, needle_bytes, start_raw, total_chars, false) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
            }
            let msg = format!(
                "startswith first arg must be str or a tuple of str, not {}",
                type_name(_py, needle)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_endswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let total_chars = utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize));
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total_chars);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let start_byte =
                utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize));
            let end_byte =
                utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                    .min(hay_bytes.len());
            let slice = &hay_bytes[start_byte..end_byte];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_STRING {
                    let needle_bytes = std::slice::from_raw_parts(
                        string_bytes(needle_ptr),
                        string_len(needle_ptr),
                    );
                    let ok = slice_match(slice, needle_bytes, start_raw, total_chars, true);
                    return MoltObject::from_bool(ok).bits();
                }
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "tuple for endswith must only contain str, not {}",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if object_type_id(elem_ptr) != TYPE_ID_STRING {
                            let msg = format!(
                                "tuple for endswith must only contain str, not {}",
                                type_name(_py, elem)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        let needle_bytes = std::slice::from_raw_parts(
                            string_bytes(elem_ptr),
                            string_len(elem_ptr),
                        );
                        if slice_match(slice, needle_bytes, start_raw, total_chars, true) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
            }
            let msg = format!(
                "endswith first arg must be str or a tuple of str, not {}",
                type_name(_py, needle)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_count(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!("must be str, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(needle_ptr) != TYPE_ID_STRING {
                let msg = format!("must be str, not {}", type_name(_py, needle));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let count = if needle_bytes.is_empty() {
                utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize)) + 1
            } else if let Some(cache) = utf8_count_cache_lookup(_py, hay_ptr as usize, needle_bytes)
            {
                cache.count
            } else {
                profile_hit(_py, &runtime_state(_py).string_count_cache_miss);
                let count = bytes_count_impl(hay_bytes, needle_bytes);
                utf8_count_cache_store(
                    _py,
                    hay_ptr as usize,
                    hay_bytes,
                    needle_bytes,
                    count,
                    Vec::new(),
                );
                count
            };
            MoltObject::from_int(count).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_count_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!("must be str, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if object_type_id(needle_ptr) != TYPE_ID_STRING {
                let msg = format!("must be str, not {}", type_name(_py, needle));
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let needle_bytes =
                std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
            let total_chars = utf8_codepoint_count_cached(_py, hay_bytes, Some(hay_ptr as usize));
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total_chars);
            if end < start {
                return MoltObject::from_int(0).bits();
            }
            if needle_bytes.is_empty() {
                if start_raw > total_chars {
                    return MoltObject::from_int(0).bits();
                }
                let count = end - start + 1;
                return MoltObject::from_int(count).bits();
            }
            let start_byte =
                utf8_char_to_byte_index_cached(_py, hay_bytes, start, Some(hay_ptr as usize));
            let end_byte =
                utf8_char_to_byte_index_cached(_py, hay_bytes, end, Some(hay_ptr as usize))
                    .min(hay_bytes.len());
            if let Some(cache) = utf8_count_cache_lookup(_py, hay_ptr as usize, needle_bytes) {
                let cache =
                    utf8_count_cache_upgrade_prefix(_py, hay_ptr as usize, &cache, hay_bytes);
                let count = utf8_count_cache_count_slice(&cache, hay_bytes, start_byte, end_byte);
                return MoltObject::from_int(count).bits();
            }
            let slice = &hay_bytes[start_byte..end_byte];
            let count = bytes_count_impl(slice, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_join(sep_bits: u64, items_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let sep = obj_from_bits(sep_bits);
        let items = obj_from_bits(items_bits);
        let sep_ptr = match sep.as_ptr() {
            Some(ptr) => ptr,
            None => return MoltObject::none().bits(),
        };
        unsafe {
            if object_type_id(sep_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "join expects a str separator");
            }
            let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            let mut total_len = 0usize;
            struct StringPart {
                bits: u64,
                data: *const u8,
                len: usize,
            }
            let mut parts = Vec::new();
            let mut all_same = true;
            let mut first_bits = 0u64;
            let mut first_data = std::ptr::null();
            let mut first_len = 0usize;
            let mut owned_bits = Vec::new();
            let mut iter_owned = false;
            if let Some(ptr) = items.as_ptr() {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(ptr);
                    parts.reserve(elems.len());
                    for (idx, &elem_bits) in elems.iter().enumerate() {
                        let elem_obj = obj_from_bits(elem_bits);
                        let elem_ptr = match elem_obj.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "sequence item {idx}: expected str instance, {} found",
                                    type_name(_py, elem_obj)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if object_type_id(elem_ptr) != TYPE_ID_STRING {
                            let msg = format!(
                                "sequence item {idx}: expected str instance, {} found",
                                type_name(_py, elem_obj)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        let len = string_len(elem_ptr);
                        total_len += len;
                        let data = string_bytes(elem_ptr);
                        if idx == 0 {
                            first_bits = elem_bits;
                            first_data = data;
                            first_len = len;
                        } else if elem_bits != first_bits {
                            all_same = false;
                        }
                        parts.push(StringPart {
                            bits: elem_bits,
                            data,
                            len,
                        });
                    }
                }
            }
            if parts.is_empty() {
                let iter_bits = molt_iter(items_bits);
                if obj_from_bits(iter_bits).is_none() {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "TypeError", "can only join an iterable");
                }
                iter_owned = true;
                let mut idx = 0usize;
                loop {
                    let pair_bits = molt_iter_next(iter_bits);
                    if exception_pending(_py) {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let pair_elems = seq_vec_ref(pair_ptr);
                    if pair_elems.len() < 2 {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        return MoltObject::none().bits();
                    }
                    let done_bits = pair_elems[1];
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        break;
                    }
                    let elem_bits = pair_elems[0];
                    let elem_obj = obj_from_bits(elem_bits);
                    let elem_ptr = match elem_obj.as_ptr() {
                        Some(ptr) => ptr,
                        None => {
                            for bits in owned_bits.iter().copied() {
                                dec_ref_bits(_py, bits);
                            }
                            let msg = format!(
                                "sequence item {idx}: expected str instance, {} found",
                                type_name(_py, elem_obj)
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                    };
                    if object_type_id(elem_ptr) != TYPE_ID_STRING {
                        for bits in owned_bits.iter().copied() {
                            dec_ref_bits(_py, bits);
                        }
                        let msg = format!(
                            "sequence item {idx}: expected str instance, {} found",
                            type_name(_py, elem_obj)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    let len = string_len(elem_ptr);
                    total_len += len;
                    let data = string_bytes(elem_ptr);
                    if idx == 0 {
                        first_bits = elem_bits;
                        first_data = data;
                        first_len = len;
                    } else if elem_bits != first_bits {
                        all_same = false;
                    }
                    parts.push(StringPart {
                        bits: elem_bits,
                        data,
                        len,
                    });
                    inc_ref_bits(_py, elem_bits);
                    owned_bits.push(elem_bits);
                    idx += 1;
                }
            }
            if !parts.is_empty() {
                let sep_total = sep_bytes
                    .len()
                    .saturating_mul(parts.len().saturating_sub(1));
                total_len = total_len.saturating_add(sep_total);
            }
            let mut result_bits = None;
            if parts.len() == 1 && !iter_owned {
                inc_ref_bits(_py, parts[0].bits);
                result_bits = Some(parts[0].bits);
            }
            if let Some(bits) = result_bits {
                return bits;
            }
            let out_ptr = alloc_bytes_like_with_len(_py, total_len, TYPE_ID_STRING);
            if out_ptr.is_null() {
                if iter_owned {
                    for bits in owned_bits.iter().copied() {
                        dec_ref_bits(_py, bits);
                    }
                }
                return MoltObject::none().bits();
            }
            let mut cursor = out_ptr.add(std::mem::size_of::<usize>());
            if all_same && parts.len() > 1 {
                let sep_len = sep_bytes.len();
                let elem_len = first_len;
                if elem_len > 0 {
                    std::ptr::copy_nonoverlapping(first_data, cursor, elem_len);
                    cursor = cursor.add(elem_len);
                }
                let pattern_len = sep_len.saturating_add(elem_len);
                let total_pattern_bytes = pattern_len.saturating_mul(parts.len() - 1);
                if total_pattern_bytes > 0 {
                    if sep_len > 0 {
                        std::ptr::copy_nonoverlapping(sep_bytes.as_ptr(), cursor, sep_len);
                    }
                    if elem_len > 0 {
                        std::ptr::copy_nonoverlapping(first_data, cursor.add(sep_len), elem_len);
                    }
                    let pattern_start = cursor;
                    let mut filled = pattern_len;
                    while filled < total_pattern_bytes {
                        let copy_len = (total_pattern_bytes - filled).min(filled);
                        std::ptr::copy_nonoverlapping(
                            pattern_start,
                            pattern_start.add(filled),
                            copy_len,
                        );
                        filled += copy_len;
                    }
                }
                let out_bits = MoltObject::from_ptr(out_ptr).bits();
                if iter_owned {
                    for bits in owned_bits.iter().copied() {
                        dec_ref_bits(_py, bits);
                    }
                }
                return out_bits;
            }
            for (idx, part) in parts.iter().enumerate() {
                if idx > 0 {
                    std::ptr::copy_nonoverlapping(sep_bytes.as_ptr(), cursor, sep_bytes.len());
                    cursor = cursor.add(sep_bytes.len());
                }
                std::ptr::copy_nonoverlapping(part.data, cursor, part.len);
                cursor = cursor.add(part.len);
            }
            let out_bits = MoltObject::from_ptr(out_ptr).bits();
            if iter_owned {
                for bits in owned_bits.iter().copied() {
                    dec_ref_bits(_py, bits);
                }
            }
            out_bits
        }
    })
}

#[derive(Copy, Clone)]
enum FormatContext {
    FormatString,
    FormatSpec,
}

struct FormatState {
    next_auto: usize,
    used_auto: bool,
    used_manual: bool,
}

struct FormatField<'a> {
    field_name: &'a str,
    conversion: Option<char>,
    format_spec: &'a str,
}

fn format_raise_value_error_str(_py: &PyToken<'_>, msg: &str) -> Option<String> {
    return raise_exception::<_>(_py, "ValueError", msg);
}

fn format_raise_value_error_bits(_py: &PyToken<'_>, msg: &str) -> Option<u64> {
    return raise_exception::<_>(_py, "ValueError", msg);
}

fn format_raise_index_error_bits(_py: &PyToken<'_>, msg: &str) -> Option<u64> {
    return raise_exception::<_>(_py, "IndexError", msg);
}

fn parse_format_field<'a>(
    _py: &PyToken<'_>,
    text: &'a str,
    start: usize,
    context: FormatContext,
) -> Option<(FormatField<'a>, usize)> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if start >= len {
        let msg = match context {
            FormatContext::FormatSpec => "unmatched '{' in format spec",
            FormatContext::FormatString => "Single '{' encountered in format string",
        };
        return raise_exception::<_>(_py, "ValueError", msg);
    }
    let mut idx = start;
    while idx < len {
        let b = bytes[idx];
        if b == b'!' || b == b':' || b == b'}' {
            break;
        }
        idx += 1;
    }
    let field_name = &text[start..idx];
    let mut conversion = None;
    if idx < len && bytes[idx] == b'!' {
        idx += 1;
        if idx >= len {
            let msg = match context {
                FormatContext::FormatSpec => "unmatched '{' in format spec",
                FormatContext::FormatString => "expected '}' before end of string",
            };
            return raise_exception::<_>(_py, "ValueError", msg);
        }
        let conv = bytes[idx] as char;
        if conv != 'r' && conv != 's' && conv != 'a' {
            if conv == '}' {
                return raise_exception::<_>(_py, "ValueError", "unmatched '{' in format spec");
            }
            let msg = format!("Unknown conversion specifier {conv}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        conversion = Some(conv);
        idx += 1;
    }
    let mut format_spec = "";
    if idx < len && bytes[idx] == b':' {
        idx += 1;
        let spec_start = idx;
        while idx < len {
            let b = bytes[idx];
            if b == b'{' {
                if idx + 1 < len && bytes[idx + 1] == b'{' {
                    idx += 2;
                    continue;
                }
                let (_, next_idx) =
                    parse_format_field(_py, text, idx + 1, FormatContext::FormatSpec)?;
                idx = next_idx;
                continue;
            }
            if b == b'}' {
                if idx + 1 < len && bytes[idx + 1] == b'}' {
                    idx += 2;
                    continue;
                }
                break;
            }
            idx += 1;
        }
        if idx >= len {
            let msg = match context {
                FormatContext::FormatSpec => "unmatched '{' in format spec",
                FormatContext::FormatString => "expected '}' before end of string",
            };
            return raise_exception::<_>(_py, "ValueError", msg);
        }
        format_spec = &text[spec_start..idx];
    }
    if idx >= len || bytes[idx] != b'}' {
        let msg = match context {
            FormatContext::FormatSpec => "unmatched '{' in format spec",
            FormatContext::FormatString => "expected '}' before end of string",
        };
        return raise_exception::<_>(_py, "ValueError", msg);
    }
    let next_idx = idx + 1;
    Some((
        FormatField {
            field_name,
            conversion,
            format_spec,
        },
        next_idx,
    ))
}

fn format_string_impl(
    _py: &PyToken<'_>,
    text: &str,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
    context: FormatContext,
) -> Option<String> {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(text.len());
    let mut idx = 0usize;
    while idx < len {
        let b = bytes[idx];
        if b == b'{' {
            if idx + 1 < len && bytes[idx + 1] == b'{' {
                out.push('{');
                idx += 2;
                continue;
            }
            let (field, next_idx) = parse_format_field(_py, text, idx + 1, context)?;
            let rendered = format_field(_py, field, args, kwargs_bits, state)?;
            out.push_str(&rendered);
            idx = next_idx;
            continue;
        }
        if b == b'}' {
            if idx + 1 < len && bytes[idx + 1] == b'}' {
                out.push('}');
                idx += 2;
                continue;
            }
            return format_raise_value_error_str(_py, "Single '}' encountered in format string");
        }
        let start = idx;
        idx += 1;
        while idx < len && bytes[idx] != b'{' && bytes[idx] != b'}' {
            idx += 1;
        }
        out.push_str(&text[start..idx]);
    }
    Some(out)
}

fn resolve_format_field(
    _py: &PyToken<'_>,
    field_name: &str,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
) -> Option<u64> {
    let bytes = field_name.as_bytes();
    let len = bytes.len();
    let mut idx = 0usize;
    while idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
        idx += 1;
    }
    let base = &field_name[..idx];
    let base_bits = if base.is_empty() {
        if state.used_manual {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from manual field specification to automatic field numbering",
            );
        }
        state.used_auto = true;
        let index = state.next_auto;
        state.next_auto += 1;
        if index >= args.len() {
            let msg = format!("Replacement index {index} out of range for positional args tuple");
            return format_raise_index_error_bits(_py, &msg);
        }
        args[index]
    } else if base.as_bytes().iter().all(|b| b.is_ascii_digit()) {
        if state.used_auto {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from automatic field numbering to manual field specification",
            );
        }
        state.used_manual = true;
        let index = match base.parse::<usize>() {
            Ok(val) => val,
            Err(_) => {
                return format_raise_value_error_bits(
                    _py,
                    "Too many decimal digits in format string",
                );
            }
        };
        if index >= args.len() {
            let msg = format!("Replacement index {base} out of range for positional args tuple");
            return format_raise_index_error_bits(_py, &msg);
        }
        args[index]
    } else {
        if state.used_auto {
            return format_raise_value_error_bits(
                _py,
                "cannot switch from automatic field numbering to manual field specification",
            );
        }
        state.used_manual = true;
        let key_ptr = alloc_string(_py, base.as_bytes());
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let kwargs_obj = obj_from_bits(kwargs_bits);
        let mut val_bits = None;
        if let Some(dict_ptr) = kwargs_obj.as_ptr() {
            unsafe {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    val_bits = dict_get_in_place(_py, dict_ptr, key_bits);
                }
            }
        }
        if val_bits.is_none() {
            raise_key_error_with_key::<()>(_py, key_bits);
            dec_ref_bits(_py, key_bits);
            return None;
        }
        dec_ref_bits(_py, key_bits);
        val_bits.unwrap()
    };
    let mut current_bits = base_bits;
    while idx < len {
        if bytes[idx] == b'.' {
            idx += 1;
            if idx >= len {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            let start = idx;
            while idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
                idx += 1;
            }
            let attr = &field_name[start..idx];
            if attr.is_empty() {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            let attr_ptr = alloc_string(_py, attr.as_bytes());
            if attr_ptr.is_null() {
                return None;
            }
            let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
            current_bits = molt_get_attr_name(current_bits, attr_bits);
            dec_ref_bits(_py, attr_bits);
            if exception_pending(_py) {
                return None;
            }
            continue;
        }
        if bytes[idx] == b'[' {
            idx += 1;
            if idx >= len {
                return format_raise_value_error_bits(_py, "expected '}' before end of string");
            }
            let start = idx;
            while idx < len && bytes[idx] != b']' {
                idx += 1;
            }
            if idx >= len {
                return format_raise_value_error_bits(_py, "expected '}' before end of string");
            }
            let key = &field_name[start..idx];
            if key.is_empty() {
                return format_raise_value_error_bits(_py, "Empty attribute in format string");
            }
            idx += 1;
            if idx < len && bytes[idx] != b'.' && bytes[idx] != b'[' {
                return format_raise_value_error_bits(
                    _py,
                    "Only '.' or '[' may follow ']' in format field specifier",
                );
            }
            let (key_bits, drop_key) = if key.as_bytes().iter().all(|b| b.is_ascii_digit()) {
                let val = match key.parse::<i64>() {
                    Ok(num) => num,
                    Err(_) => {
                        return format_raise_value_error_bits(
                            _py,
                            "Too many decimal digits in format string",
                        );
                    }
                };
                (MoltObject::from_int(val).bits(), false)
            } else {
                let key_ptr = alloc_string(_py, key.as_bytes());
                if key_ptr.is_null() {
                    return None;
                }
                (MoltObject::from_ptr(key_ptr).bits(), true)
            };
            current_bits = molt_index(current_bits, key_bits);
            if drop_key {
                dec_ref_bits(_py, key_bits);
            }
            if exception_pending(_py) {
                return None;
            }
            continue;
        }
        break;
    }
    Some(current_bits)
}

fn format_field(
    _py: &PyToken<'_>,
    field: FormatField,
    args: &[u64],
    kwargs_bits: u64,
    state: &mut FormatState,
) -> Option<String> {
    let mut value_bits = resolve_format_field(_py, field.field_name, args, kwargs_bits, state)?;
    if exception_pending(_py) {
        return None;
    }
    let mut drop_value = false;
    if let Some(conv) = field.conversion {
        value_bits = match conv {
            'r' => {
                drop_value = true;
                molt_repr_from_obj(value_bits)
            }
            's' => {
                drop_value = true;
                molt_str_from_obj(value_bits)
            }
            'a' => {
                drop_value = true;
                molt_ascii_from_obj(value_bits)
            }
            _ => value_bits,
        };
        if exception_pending(_py) {
            return None;
        }
    }
    let spec_text = if field.format_spec.is_empty() {
        String::new()
    } else {
        format_string_impl(
            _py,
            field.format_spec,
            args,
            kwargs_bits,
            state,
            FormatContext::FormatSpec,
        )?
    };
    let spec_ptr = alloc_string(_py, spec_text.as_bytes());
    if spec_ptr.is_null() {
        return None;
    }
    let spec_bits = MoltObject::from_ptr(spec_ptr).bits();
    let formatted_bits = molt_format_builtin(value_bits, spec_bits);
    dec_ref_bits(_py, spec_bits);
    if drop_value {
        dec_ref_bits(_py, value_bits);
    }
    if exception_pending(_py) {
        return None;
    }
    let formatted_obj = obj_from_bits(formatted_bits);
    let rendered =
        string_obj_to_owned(formatted_obj).unwrap_or_else(|| format_obj_str(_py, formatted_obj));
    dec_ref_bits(_py, formatted_bits);
    Some(rendered)
}

#[no_mangle]
pub extern "C" fn molt_string_format_method(
    self_bits: u64,
    args_bits: u64,
    kwargs_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "format requires a string");
            }
            let text = string_obj_to_owned(self_obj).unwrap_or_default();
            let args_obj = obj_from_bits(args_bits);
            let Some(args_ptr) = args_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "format arguments must be a tuple");
            };
            if object_type_id(args_ptr) != TYPE_ID_TUPLE {
                return raise_exception::<_>(_py, "TypeError", "format arguments must be a tuple");
            }
            let args_vec = seq_vec_ref(args_ptr);
            let mut state = FormatState {
                next_auto: 0,
                used_auto: false,
                used_manual: false,
            };
            let Some(rendered) = format_string_impl(
                _py,
                &text,
                args_vec.as_slice(),
                kwargs_bits,
                &mut state,
                FormatContext::FormatString,
            ) else {
                return MoltObject::none().bits();
            };
            let out_ptr = alloc_string(_py, rendered.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_format(val_bits: u64, spec_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let spec_obj = obj_from_bits(spec_bits);
        let spec_ptr = match spec_obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "format spec must be a str"),
        };
        unsafe {
            if object_type_id(spec_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "format spec must be a str");
            }
            let spec_bytes =
                std::slice::from_raw_parts(string_bytes(spec_ptr), string_len(spec_ptr));
            let spec_text = match std::str::from_utf8(spec_bytes) {
                Ok(val) => val,
                Err(_) => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "format spec must be valid UTF-8",
                    )
                }
            };
            let spec = match parse_format_spec(spec_text) {
                Ok(val) => val,
                Err(msg) => return raise_exception::<_>(_py, "ValueError", msg),
            };
            let obj = obj_from_bits(val_bits);
            let rendered = match format_with_spec(_py, obj, &spec) {
                Ok(val) => val,
                Err((kind, msg)) => return raise_exception::<_>(_py, kind, msg),
            };
            let out_ptr = alloc_string(_py, rendered.as_bytes());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_find(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_find_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_find_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let total = hay_bytes.len() as i64;
                let (start, end, start_raw) =
                    slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                let slice = &hay_bytes[start as usize..end as usize];
                if let Some(byte) = needle.as_int() {
                    if !(0..=255).contains(&byte) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "byte must be in range(0, 256)",
                        );
                    }
                    let idx = match memchr(byte as u8, slice) {
                        Some(pos) => start + pos as i64,
                        None => -1,
                    };
                    return MoltObject::from_int(idx).bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if needle_bytes.is_empty() {
                    if start_raw > total {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(start).bits();
                }
                let idx = bytes_find_impl(slice, needle_bytes);
                let out = if idx < 0 { -1 } else { start + idx };
                MoltObject::from_int(out).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_rfind(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_rfind_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_rfind_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let total = hay_bytes.len() as i64;
                let (start, end, start_raw) =
                    slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                let slice = &hay_bytes[start as usize..end as usize];
                if let Some(byte) = needle.as_int() {
                    if !(0..=255).contains(&byte) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "byte must be in range(0, 256)",
                        );
                    }
                    let idx = match memchr::memrchr(byte as u8, slice) {
                        Some(pos) => start + pos as i64,
                        None => -1,
                    };
                    return MoltObject::from_int(idx).bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if needle_bytes.is_empty() {
                    if start_raw > total {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(end).bits();
                }
                let idx = bytes_rfind_impl(slice, needle_bytes);
                let out = if idx < 0 { -1 } else { start + idx };
                MoltObject::from_int(out).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_startswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytes_endswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_startswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let needle_bytes = match bytes_like_slice(elem_ptr) {
                            Some(slice) => slice,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, false) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                if let Some(needle_bytes) = bytes_like_slice(needle_ptr) {
                    let ok = slice_match(slice, needle_bytes, start_raw, total, false);
                    return MoltObject::from_bool(ok).bits();
                }
            }
            let msg = format!(
                "startswith first arg must be bytes or a tuple of bytes, not {}",
                type_name(_py, needle)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_endswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let needle_bytes = match bytes_like_slice(elem_ptr) {
                            Some(slice) => slice,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, true) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                if let Some(needle_bytes) = bytes_like_slice(needle_ptr) {
                    let ok = slice_match(slice, needle_bytes, start_raw, total, true);
                    return MoltObject::from_bool(ok).bits();
                }
            }
            let msg = format!(
                "endswith first arg must be bytes or a tuple of bytes, not {}",
                type_name(_py, needle)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_count(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            if let Some(byte) = needle.as_int() {
                if !(0..=255).contains(&byte) {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "byte must be in range(0, 256)",
                    );
                }
                let count = memchr::memchr_iter(byte as u8, hay_bytes).count() as i64;
                return MoltObject::from_int(count).bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let count = bytes_count_impl(hay_bytes, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_count_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_int(0).bits();
            }
            if let Some(byte) = needle.as_int() {
                if !(0..=255).contains(&byte) {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "byte must be in range(0, 256)",
                    );
                }
                let slice = &hay_bytes[start as usize..end as usize];
                let count = memchr::memchr_iter(byte as u8, slice).count() as i64;
                return MoltObject::from_int(count).bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if needle_bytes.is_empty() {
                if start_raw > total {
                    return MoltObject::from_int(0).bits();
                }
                let count = end - start + 1;
                return MoltObject::from_int(count).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            let count = bytes_count_impl(slice, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

fn build_utf8_cache(bytes: &[u8]) -> Utf8IndexCache {
    let mut offsets = Vec::new();
    let mut prefix = Vec::new();
    let mut total = 0i64;
    let mut idx = 0usize;
    offsets.push(0);
    prefix.push(0);
    while idx < bytes.len() {
        let mut end = (idx + UTF8_CACHE_BLOCK).min(bytes.len());
        while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end += 1;
        }
        total += count_utf8_bytes(&bytes[idx..end]);
        offsets.push(end);
        prefix.push(total);
        idx = end;
    }
    Utf8IndexCache { offsets, prefix }
}

fn utf8_cache_get_or_build(
    _py: &PyToken<'_>,
    key: usize,
    bytes: &[u8],
) -> Option<Arc<Utf8IndexCache>> {
    if bytes.len() < UTF8_CACHE_MIN_LEN || bytes.is_ascii() {
        return None;
    }
    if let Ok(store) = runtime_state(_py).utf8_index_cache.lock() {
        if let Some(cache) = store.get(key) {
            return Some(cache);
        }
    }
    let cache = Arc::new(build_utf8_cache(bytes));
    if let Ok(mut store) = runtime_state(_py).utf8_index_cache.lock() {
        if let Some(existing) = store.get(key) {
            return Some(existing);
        }
        store.insert(key, cache.clone());
    }
    Some(cache)
}

pub(crate) fn utf8_cache_remove(_py: &PyToken<'_>, key: usize) {
    if let Ok(mut store) = runtime_state(_py).utf8_index_cache.lock() {
        store.remove(key);
    }
    utf8_count_cache_remove(_py, key);
    utf8_count_cache_tls_remove(key);
}

fn utf8_count_cache_shard(key: usize) -> usize {
    let mut x = key as u64;
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    (x as usize) & (UTF8_COUNT_CACHE_SHARDS - 1)
}

fn utf8_count_cache_remove(_py: &PyToken<'_>, key: usize) {
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard) {
        if let Ok(mut guard) = store.lock() {
            guard.remove(key);
        }
    }
}

fn utf8_count_cache_lookup(
    _py: &PyToken<'_>,
    key: usize,
    needle: &[u8],
) -> Option<Arc<Utf8CountCache>> {
    if let Some(cache) = UTF8_COUNT_TLS.with(|cell| {
        cell.borrow().as_ref().and_then(|entry| {
            if entry.key == key && entry.cache.needle == needle {
                Some(entry.cache.clone())
            } else {
                None
            }
        })
    }) {
        profile_hit(_py, &runtime_state(_py).string_count_cache_hit);
        return Some(cache);
    }
    let shard = utf8_count_cache_shard(key);
    let store = runtime_state(_py)
        .utf8_count_cache
        .get(shard)?
        .lock()
        .ok()?;
    let cache = store.get(key)?;
    if cache.needle == needle {
        profile_hit(_py, &runtime_state(_py).string_count_cache_hit);
        return Some(cache);
    }
    None
}

fn build_utf8_count_prefix(hay_bytes: &[u8], needle: &[u8]) -> Vec<i64> {
    if hay_bytes.len() < UTF8_COUNT_PREFIX_MIN_LEN || needle.is_empty() {
        return Vec::new();
    }
    let blocks = hay_bytes.len().div_ceil(UTF8_CACHE_BLOCK);
    let mut prefix = vec![0i64; blocks + 1];
    let mut count = 0i64;
    let mut idx = 1usize;
    let mut next_boundary = UTF8_CACHE_BLOCK.min(hay_bytes.len());
    let finder = memmem::Finder::new(needle);
    for pos in finder.find_iter(hay_bytes) {
        while pos >= next_boundary && idx < prefix.len() {
            prefix[idx] = count;
            idx += 1;
            next_boundary = (next_boundary + UTF8_CACHE_BLOCK).min(hay_bytes.len());
        }
        count += 1;
    }
    while idx < prefix.len() {
        prefix[idx] = count;
        idx += 1;
    }
    prefix
}

fn utf8_count_cache_store(
    _py: &PyToken<'_>,
    key: usize,
    hay_bytes: &[u8],
    needle: &[u8],
    count: i64,
    prefix: Vec<i64>,
) {
    let cache = Arc::new(Utf8CountCache {
        needle: needle.to_vec(),
        count,
        prefix,
        hay_len: hay_bytes.len(),
    });
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard) {
        if let Ok(mut guard) = store.lock() {
            guard.insert(key, cache.clone());
        }
    }
    UTF8_COUNT_TLS.with(|cell| {
        *cell.borrow_mut() = Some(Utf8CountCacheEntry { key, cache });
    });
}

fn utf8_count_cache_upgrade_prefix(
    _py: &PyToken<'_>,
    key: usize,
    cache: &Arc<Utf8CountCache>,
    hay_bytes: &[u8],
) -> Arc<Utf8CountCache> {
    if !cache.prefix.is_empty()
        || cache.hay_len != hay_bytes.len()
        || hay_bytes.len() < UTF8_COUNT_PREFIX_MIN_LEN
        || cache.needle.is_empty()
    {
        return cache.clone();
    }
    let prefix = build_utf8_count_prefix(hay_bytes, &cache.needle);
    if prefix.is_empty() {
        return cache.clone();
    }
    let upgraded = Arc::new(Utf8CountCache {
        needle: cache.needle.clone(),
        count: cache.count,
        prefix,
        hay_len: cache.hay_len,
    });
    let shard = utf8_count_cache_shard(key);
    if let Some(store) = runtime_state(_py).utf8_count_cache.get(shard) {
        if let Ok(mut guard) = store.lock() {
            guard.insert(key, upgraded.clone());
        }
    }
    UTF8_COUNT_TLS.with(|cell| {
        *cell.borrow_mut() = Some(Utf8CountCacheEntry {
            key,
            cache: upgraded.clone(),
        });
    });
    upgraded
}

fn utf8_count_cache_tls_remove(key: usize) {
    UTF8_COUNT_TLS.with(|cell| {
        let mut guard = cell.borrow_mut();
        if guard.as_ref().is_some_and(|entry| entry.key == key) {
            *guard = None;
        }
    });
}

fn count_matches_range(
    hay_bytes: &[u8],
    needle: &[u8],
    window_start: usize,
    window_end: usize,
    start_min: usize,
    start_max: usize,
) -> i64 {
    if window_end <= window_start || start_min > start_max {
        return 0;
    }
    let finder = memmem::Finder::new(needle);
    let mut count = 0i64;
    for pos in finder.find_iter(&hay_bytes[window_start..window_end]) {
        let abs = window_start + pos;
        if abs < start_min {
            continue;
        }
        if abs > start_max {
            break;
        }
        count += 1;
    }
    count
}

fn utf8_count_cache_count_slice(
    cache: &Utf8CountCache,
    hay_bytes: &[u8],
    start: usize,
    end: usize,
) -> i64 {
    let needle = &cache.needle;
    let needle_len = needle.len();
    if needle_len == 0 || end <= start {
        return 0;
    }
    if end - start < needle_len {
        return 0;
    }
    if cache.prefix.is_empty() || cache.hay_len != hay_bytes.len() {
        return bytes_count_impl(&hay_bytes[start..end], needle);
    }
    let end_limit = end - needle_len;
    let block = UTF8_CACHE_BLOCK;
    let start_block = start / block;
    let end_block = end_limit / block;
    if start_block == end_block {
        return bytes_count_impl(&hay_bytes[start..end], needle);
    }
    let mut total = 0i64;
    let block_end = ((start_block + 1) * block).min(hay_bytes.len());
    let left_scan_end = (block_end + needle_len - 1).min(end);
    let left_max = (block_end.saturating_sub(1)).min(end_limit);
    total += count_matches_range(hay_bytes, needle, start, left_scan_end, start, left_max);
    if end_block > start_block + 1 {
        total += cache.prefix[end_block] - cache.prefix[start_block + 1];
    }
    let right_block_start = (end_block * block).min(hay_bytes.len());
    if right_block_start <= end_limit {
        total += count_matches_range(
            hay_bytes,
            needle,
            right_block_start,
            end,
            right_block_start,
            end_limit,
        );
    }
    total
}

fn utf8_count_prefix_cached(bytes: &[u8], cache: &Utf8IndexCache, prefix_len: usize) -> i64 {
    let prefix_len = prefix_len.min(bytes.len());
    let block_idx = match cache.offsets.binary_search(&prefix_len) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
    };
    let mut total = *cache.prefix.get(block_idx).unwrap_or(&0);
    let start = *cache.offsets.get(block_idx).unwrap_or(&0);
    if start < prefix_len {
        total += count_utf8_bytes(&bytes[start..prefix_len]);
    }
    total
}

pub(crate) fn utf8_codepoint_count_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    cache_key: Option<usize>,
) -> i64 {
    if bytes.is_ascii() {
        return bytes.len() as i64;
    }
    if let Some(key) = cache_key {
        if let Some(cache) = utf8_cache_get_or_build(_py, key, bytes) {
            return *cache.prefix.last().unwrap_or(&0);
        }
    }
    utf8_count_prefix_blocked(bytes, bytes.len())
}

fn utf8_byte_to_char_index_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    byte_idx: usize,
    cache_key: Option<usize>,
) -> i64 {
    if byte_idx == 0 {
        return 0;
    }
    if bytes.is_ascii() {
        return byte_idx.min(bytes.len()) as i64;
    }
    let prefix_len = byte_idx.min(bytes.len());
    if let Some(key) = cache_key {
        if let Some(cache) = utf8_cache_get_or_build(_py, key, bytes) {
            return utf8_count_prefix_cached(bytes, &cache, prefix_len);
        }
    }
    utf8_count_prefix_blocked(bytes, prefix_len)
}

fn utf8_char_width(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

fn utf8_char_to_byte_index_scan(bytes: &[u8], target: usize) -> usize {
    let mut idx = 0usize;
    let mut count = 0usize;
    while idx < bytes.len() && count < target {
        let width = utf8_char_width(bytes[idx]);
        idx = idx.saturating_add(width);
        count = count.saturating_add(1);
    }
    idx.min(bytes.len())
}

fn utf8_char_to_byte_index_cached(
    _py: &PyToken<'_>,
    bytes: &[u8],
    char_idx: i64,
    cache_key: Option<usize>,
) -> usize {
    if char_idx <= 0 {
        return 0;
    }
    if bytes.is_ascii() {
        return (char_idx as usize).min(bytes.len());
    }
    let total = utf8_codepoint_count_cached(_py, bytes, cache_key);
    if char_idx >= total {
        return bytes.len();
    }
    let target = char_idx as usize;
    if let Some(key) = cache_key {
        if let Some(cache) = utf8_cache_get_or_build(_py, key, bytes) {
            let mut lo = 0usize;
            let mut hi = cache.prefix.len().saturating_sub(1);
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if (cache.prefix.get(mid).copied().unwrap_or(0) as usize) <= target {
                    lo = mid;
                } else {
                    hi = mid.saturating_sub(1);
                }
            }
            let mut count = cache.prefix.get(lo).copied().unwrap_or(0) as usize;
            let mut idx = cache.offsets.get(lo).copied().unwrap_or(0);
            while idx < bytes.len() && count < target {
                let width = utf8_char_width(bytes[idx]);
                idx = idx.saturating_add(width);
                count = count.saturating_add(1);
            }
            return idx.min(bytes.len());
        }
    }
    utf8_char_to_byte_index_scan(bytes, target)
}

fn utf8_count_prefix_blocked(bytes: &[u8], prefix_len: usize) -> i64 {
    const BLOCK: usize = 4096;
    let mut total = 0i64;
    let mut idx = 0usize;
    while idx + BLOCK <= prefix_len {
        total += count_utf8_bytes(&bytes[idx..idx + BLOCK]);
        idx += BLOCK;
    }
    if idx < prefix_len {
        total += count_utf8_bytes(&bytes[idx..prefix_len]);
    }
    total
}

#[cfg(not(target_arch = "wasm32"))]
fn count_utf8_bytes(bytes: &[u8]) -> i64 {
    simdutf::count_utf8(bytes) as i64
}

#[cfg(target_arch = "wasm32")]
fn count_utf8_bytes(bytes: &[u8]) -> i64 {
    let mut count = 0i64;
    let mut idx = 0usize;
    while idx < bytes.len() {
        let b = bytes[idx];
        let width = if b < 0x80 {
            1
        } else if b < 0xE0 {
            2
        } else if b < 0xF0 {
            3
        } else {
            4
        };
        idx = idx.saturating_add(width);
        count += 1;
    }
    count
}

#[no_mangle]
pub extern "C" fn molt_bytearray_find(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_find_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_rfind(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_rfind_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_startswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_startswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_endswith(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let none_bits = MoltObject::none().bits();
        let false_bits = MoltObject::from_bool(false).bits();
        molt_bytearray_endswith_slice(
            hay_bits,
            needle_bits,
            none_bits,
            none_bits,
            false_bits,
            false_bits,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_find_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let total = hay_bytes.len() as i64;
                let (start, end, start_raw) =
                    slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                let slice = &hay_bytes[start as usize..end as usize];
                if let Some(byte) = needle.as_int() {
                    if !(0..=255).contains(&byte) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "byte must be in range(0, 256)",
                        );
                    }
                    let idx = match memchr(byte as u8, slice) {
                        Some(pos) => start + pos as i64,
                        None => -1,
                    };
                    return MoltObject::from_int(idx).bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if needle_bytes.is_empty() {
                    if start_raw > total {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(start).bits();
                }
                let idx = bytes_find_impl(slice, needle_bytes);
                let out = if idx < 0 { -1 } else { start + idx };
                MoltObject::from_int(out).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_rfind_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let total = hay_bytes.len() as i64;
                let (start, end, start_raw) =
                    slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
                if end < start {
                    return MoltObject::from_int(-1).bits();
                }
                let slice = &hay_bytes[start as usize..end as usize];
                if let Some(byte) = needle.as_int() {
                    if !(0..=255).contains(&byte) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "byte must be in range(0, 256)",
                        );
                    }
                    let idx = match memchr::memrchr(byte as u8, slice) {
                        Some(pos) => start + pos as i64,
                        None => -1,
                    };
                    return MoltObject::from_int(idx).bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "argument should be integer or bytes-like object, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if needle_bytes.is_empty() {
                    if start_raw > total {
                        return MoltObject::from_int(-1).bits();
                    }
                    return MoltObject::from_int(end).bits();
                }
                let idx = bytes_rfind_impl(slice, needle_bytes);
                let out = if idx < 0 { -1 } else { start + idx };
                MoltObject::from_int(out).bits()
            }
        } else {
            MoltObject::none().bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_startswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let needle_bytes = match bytes_like_slice(elem_ptr) {
                            Some(slice) => slice,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, false) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                if let Some(needle_bytes) = bytes_like_slice(needle_ptr) {
                    let ok = slice_match(slice, needle_bytes, start_raw, total, false);
                    return MoltObject::from_bool(ok).bits();
                }
            }
            let msg = format!(
                "startswith first arg must be bytes or a tuple of bytes, not {}",
                type_name(_py, needle)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_endswith_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_bool(false).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            if let Some(needle_ptr) = needle.as_ptr() {
                let needle_type = object_type_id(needle_ptr);
                if needle_type == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(needle_ptr);
                    if elems.is_empty() {
                        return MoltObject::from_bool(false).bits();
                    }
                    for &elem_bits in elems.iter() {
                        let elem = obj_from_bits(elem_bits);
                        let elem_ptr = match elem.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        let needle_bytes = match bytes_like_slice(elem_ptr) {
                            Some(slice) => slice,
                            None => {
                                let msg = format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, elem)
                                );
                                return raise_exception::<_>(_py, "TypeError", &msg);
                            }
                        };
                        if slice_match(slice, needle_bytes, start_raw, total, true) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                    return MoltObject::from_bool(false).bits();
                }
                if let Some(needle_bytes) = bytes_like_slice(needle_ptr) {
                    let ok = slice_match(slice, needle_bytes, start_raw, total, true);
                    return MoltObject::from_bool(ok).bits();
                }
            }
            let msg = format!(
                "endswith first arg must be bytes or a tuple of bytes, not {}",
                type_name(_py, needle)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_count(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            if let Some(byte) = needle.as_int() {
                if !(0..=255).contains(&byte) {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "byte must be in range(0, 256)",
                    );
                }
                let count = memchr::memchr_iter(byte as u8, hay_bytes).count() as i64;
                return MoltObject::from_int(count).bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let count = bytes_count_impl(hay_bytes, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_count_slice(
    hay_bits: u64,
    needle_bits: u64,
    start_bits: u64,
    end_bits: u64,
    has_start_bits: u64,
    has_end_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let has_start = to_i64(obj_from_bits(has_start_bits)).unwrap_or(0) != 0;
        let has_end = to_i64(obj_from_bits(has_end_bits)).unwrap_or(0) != 0;
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let total = hay_bytes.len() as i64;
            let (start, end, start_raw) =
                slice_bounds_from_args(_py, start_bits, end_bits, has_start, has_end, total);
            if end < start {
                return MoltObject::from_int(0).bits();
            }
            if let Some(byte) = needle.as_int() {
                if !(0..=255).contains(&byte) {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "byte must be in range(0, 256)",
                    );
                }
                let slice = &hay_bytes[start as usize..end as usize];
                let count = memchr::memchr_iter(byte as u8, slice).count() as i64;
                return MoltObject::from_int(count).bits();
            }
            let needle_ptr = match needle.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let needle_bytes = match bytes_like_slice(needle_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "argument should be integer or bytes-like object, not '{}'",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if needle_bytes.is_empty() {
                if start_raw > total {
                    return MoltObject::from_int(0).bits();
                }
                let count = end - start + 1;
                return MoltObject::from_int(count).bits();
            }
            let slice = &hay_bytes[start as usize..end as usize];
            let count = bytes_count_impl(slice, needle_bytes);
            MoltObject::from_int(count).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_splitlines(hay_bits: u64, keepends_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let keepends = is_truthy(_py, obj_from_bits(keepends_bits));
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let list_bits =
                splitlines_bytes_to_list(_py, hay_bytes, keepends, |bytes| alloc_bytes(_py, bytes));
            list_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_splitlines(hay_bits: u64, keepends_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let keepends = is_truthy(_py, obj_from_bits(keepends_bits));
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let list_bits = splitlines_bytes_to_list(_py, hay_bytes, keepends, |bytes| {
                alloc_bytearray(_py, bytes)
            });
            list_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_splitlines(hay_bits: u64, keepends_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let keepends = is_truthy(_py, obj_from_bits(keepends_bits));
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let list_bits = splitlines_string_to_list(_py, hay_str, keepends);
            list_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_split(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_string_split_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_string_split_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let hay_bytes =
                    std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
                if needle.is_none() {
                    let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                        return MoltObject::none().bits();
                    };
                    let list_bits =
                        split_string_whitespace_to_list_maxsplit(_py, hay_str, maxsplit);
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let Some(needle_ptr) = needle.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!("must be str or None, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let needle_bytes =
                    std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits =
                    split_string_bytes_to_list_maxsplit(_py, hay_bytes, needle_bytes, maxsplit);
                let list_bits = match list_bits {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_string_rsplit(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_string_rsplit_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_string_rsplit_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let hay_bytes =
                    std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
                if needle.is_none() {
                    let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                        return MoltObject::none().bits();
                    };
                    let list_bits =
                        rsplit_string_whitespace_to_list_maxsplit(_py, hay_str, maxsplit);
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let Some(needle_ptr) = needle.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!("must be str or None, not {}", type_name(_py, needle));
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let needle_bytes =
                    std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits =
                    rsplit_string_bytes_to_list_maxsplit(_py, hay_bytes, needle_bytes, maxsplit);
                let list_bits = match list_bits {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_string_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let replacement = obj_from_bits(replacement_bits);
        let count_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(count_bits))
        );
        let count = index_i64_from_obj(_py, count_bits, &count_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_STRING {
                    return MoltObject::none().bits();
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "replace() argument 1 must be str, not {}",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(needle_ptr) != TYPE_ID_STRING {
                    let msg = format!(
                        "replace() argument 1 must be str, not {}",
                        type_name(_py, needle)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let repl_ptr = match replacement.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "replace() argument 2 must be str, not {}",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if object_type_id(repl_ptr) != TYPE_ID_STRING {
                    let msg = format!(
                        "replace() argument 2 must be str, not {}",
                        type_name(_py, replacement)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                let hay_bytes =
                    std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
                let needle_bytes =
                    std::slice::from_raw_parts(string_bytes(needle_ptr), string_len(needle_ptr));
                let repl_bytes =
                    std::slice::from_raw_parts(string_bytes(repl_ptr), string_len(repl_ptr));
                let out = match replace_string_impl(
                    _py,
                    hay_ptr,
                    hay_bytes,
                    needle_bytes,
                    repl_bytes,
                    count,
                ) {
                    Some(out) => out,
                    None => return MoltObject::none().bits(),
                };
                let ptr = alloc_string(_py, &out);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_string_encode(hay_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let encoding = match parse_codec_arg(_py, encoding_bits, "encode", "encoding", "utf-8")
            {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let errors = match parse_codec_arg(_py, errors_bits, "encode", "errors", "strict") {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let text = string_obj_to_owned(hay).unwrap_or_default();
            let out = match encode_string_with_errors(&text, &encoding, Some(&errors)) {
                Ok(bytes) => bytes,
                Err(EncodeError::UnknownEncoding(name)) => {
                    let msg = format!("unknown encoding: {name}");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                Err(EncodeError::UnknownErrorHandler(name)) => {
                    let msg = format!("unknown error handler name '{name}'");
                    return raise_exception::<_>(_py, "LookupError", &msg);
                }
                Err(EncodeError::InvalidChar {
                    encoding,
                    ch,
                    pos,
                    limit,
                }) => {
                    let escaped = unicode_escape(ch);
                    let msg = format!(
                    "'{encoding}' codec can't encode character '{escaped}' in position {pos}: ordinal not in range({limit})"
                );
                    return raise_exception::<_>(_py, "UnicodeEncodeError", &msg);
                }
            };
            let ptr = alloc_bytes(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_lower(hay_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let lowered = hay_str.to_lowercase();
            let ptr = alloc_string(_py, lowered.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_upper(hay_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let uppered = hay_str.to_uppercase();
            let ptr = alloc_string(_py, uppered.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_capitalize(hay_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let mut out = String::with_capacity(hay_str.len());
            let mut chars = hay_str.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                for ch in chars {
                    out.extend(ch.to_lowercase());
                }
            }
            let ptr = alloc_string(_py, out.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_strip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let chars = obj_from_bits(chars_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let trimmed = if chars.is_none() {
                hay_str.trim()
            } else {
                let Some(chars_ptr) = chars.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "lstrip arg must be None or str",
                    );
                };
                if object_type_id(chars_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "lstrip arg must be None or str",
                    );
                }
                let chars_bytes =
                    std::slice::from_raw_parts(string_bytes(chars_ptr), string_len(chars_ptr));
                let Ok(chars_str) = std::str::from_utf8(chars_bytes) else {
                    return MoltObject::none().bits();
                };
                if chars_str.is_empty() {
                    hay_str
                } else {
                    let mut strip_chars = HashSet::new();
                    for ch in chars_str.chars() {
                        strip_chars.insert(ch);
                    }
                    let mut start = None;
                    for (idx, ch) in hay_str.char_indices() {
                        if !strip_chars.contains(&ch) {
                            start = Some(idx);
                            break;
                        }
                    }
                    match start {
                        None => "",
                        Some(start_idx) => {
                            let mut end = None;
                            for (idx, ch) in hay_str.char_indices().rev() {
                                if !strip_chars.contains(&ch) {
                                    end = Some(idx + ch.len_utf8());
                                    break;
                                }
                            }
                            let end_idx = end.unwrap_or(start_idx);
                            &hay_str[start_idx..end_idx]
                        }
                    }
                }
            };
            let ptr = alloc_string(_py, trimmed.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn string_lstrip_chars<'a>(hay_str: &'a str, chars_str: &str) -> &'a str {
    if chars_str.is_empty() {
        return hay_str;
    }
    let mut strip_chars = HashSet::new();
    for ch in chars_str.chars() {
        strip_chars.insert(ch);
    }
    for (idx, ch) in hay_str.char_indices() {
        if !strip_chars.contains(&ch) {
            return &hay_str[idx..];
        }
    }
    ""
}

fn string_rstrip_chars<'a>(hay_str: &'a str, chars_str: &str) -> &'a str {
    if chars_str.is_empty() {
        return hay_str;
    }
    let mut strip_chars = HashSet::new();
    for ch in chars_str.chars() {
        strip_chars.insert(ch);
    }
    for (idx, ch) in hay_str.char_indices().rev() {
        if !strip_chars.contains(&ch) {
            let end = idx + ch.len_utf8();
            return &hay_str[..end];
        }
    }
    ""
}

#[no_mangle]
pub extern "C" fn molt_string_lstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let chars = obj_from_bits(chars_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let trimmed = if chars.is_none() {
                hay_str.trim_start()
            } else {
                let Some(chars_ptr) = chars.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "rstrip arg must be None or str",
                    );
                };
                if object_type_id(chars_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "rstrip arg must be None or str",
                    );
                }
                let chars_bytes =
                    std::slice::from_raw_parts(string_bytes(chars_ptr), string_len(chars_ptr));
                let Ok(chars_str) = std::str::from_utf8(chars_bytes) else {
                    return MoltObject::none().bits();
                };
                string_lstrip_chars(hay_str, chars_str)
            };
            let ptr = alloc_string(_py, trimmed.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_string_rstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let chars = obj_from_bits(chars_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_STRING {
                return MoltObject::none().bits();
            }
            let hay_bytes = std::slice::from_raw_parts(string_bytes(hay_ptr), string_len(hay_ptr));
            let Ok(hay_str) = std::str::from_utf8(hay_bytes) else {
                return MoltObject::none().bits();
            };
            let trimmed = if chars.is_none() {
                hay_str.trim_end()
            } else {
                let Some(chars_ptr) = chars.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "strip arg must be None or str");
                };
                if object_type_id(chars_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(_py, "TypeError", "strip arg must be None or str");
                }
                let chars_bytes =
                    std::slice::from_raw_parts(string_bytes(chars_ptr), string_len(chars_ptr));
                let Ok(chars_str) = std::str::from_utf8(chars_bytes) else {
                    return MoltObject::none().bits();
                };
                string_rstrip_chars(hay_str, chars_str)
            };
            let ptr = alloc_string(_py, trimmed.as_bytes());
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

fn partition_bytes_to_tuple<F>(
    _py: &PyToken<'_>,
    hay_bytes: &[u8],
    sep_bytes: &[u8],
    from_right: bool,
    mut alloc: F,
) -> Option<u64>
where
    F: FnMut(&[u8]) -> *mut u8,
{
    let idx = if from_right {
        bytes_rfind_impl(hay_bytes, sep_bytes)
    } else {
        bytes_find_impl(hay_bytes, sep_bytes)
    };
    let (head_bytes, sep_bytes, tail_bytes) = if idx < 0 {
        if from_right {
            (&[][..], &[][..], hay_bytes)
        } else {
            (hay_bytes, &[][..], &[][..])
        }
    } else {
        let idx = idx as usize;
        let end = idx + sep_bytes.len();
        (&hay_bytes[..idx], sep_bytes, &hay_bytes[end..])
    };
    let head_ptr = alloc(head_bytes);
    if head_ptr.is_null() {
        return None;
    }
    let head_bits = MoltObject::from_ptr(head_ptr).bits();
    let sep_ptr = alloc(sep_bytes);
    if sep_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        return None;
    }
    let sep_bits = MoltObject::from_ptr(sep_ptr).bits();
    let tail_ptr = alloc(tail_bytes);
    if tail_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        dec_ref_bits(_py, sep_bits);
        return None;
    }
    let tail_bits = MoltObject::from_ptr(tail_ptr).bits();
    let tuple_ptr = alloc_tuple(_py, &[head_bits, sep_bits, tail_bits]);
    if tuple_ptr.is_null() {
        dec_ref_bits(_py, head_bits);
        dec_ref_bits(_py, sep_bits);
        dec_ref_bits(_py, tail_bits);
        return None;
    }
    Some(MoltObject::from_ptr(tuple_ptr).bits())
}

#[no_mangle]
pub extern "C" fn molt_bytes_partition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let sep_bytes = match bytes_like_slice(sep_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let tuple_bits = partition_bytes_to_tuple(_py, hay_bytes, sep_bytes, false, |bytes| {
                alloc_bytes(_py, bytes)
            });
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_rpartition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let sep_bytes = match bytes_like_slice(sep_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let tuple_bits = partition_bytes_to_tuple(_py, hay_bytes, sep_bytes, true, |bytes| {
                alloc_bytes(_py, bytes)
            });
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_partition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let sep_bytes = match bytes_like_slice(sep_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let tuple_bits = partition_bytes_to_tuple(_py, hay_bytes, sep_bytes, false, |bytes| {
                alloc_bytearray(_py, bytes)
            });
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_rpartition(hay_bits: u64, sep_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let sep = obj_from_bits(sep_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let sep_ptr = match sep.as_ptr() {
                Some(ptr) => ptr,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            let sep_bytes = match bytes_like_slice(sep_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, sep)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            };
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let tuple_bits = partition_bytes_to_tuple(_py, hay_bytes, sep_bytes, true, |bytes| {
                alloc_bytearray(_py, bytes)
            });
            tuple_bits.unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_split(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytes_split_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_split_max(hay_bits: u64, needle_bits: u64, maxsplit_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                if needle.is_none() {
                    let list_bits = split_bytes_whitespace_to_list_maxsplit(
                        _py,
                        hay_bytes,
                        maxsplit,
                        |bytes| alloc_bytes(_py, bytes),
                    );
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits = match split_bytes_to_list_maxsplit(
                    _py,
                    hay_bytes,
                    needle_bytes,
                    maxsplit,
                    |bytes| alloc_bytes(_py, bytes),
                ) {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_rsplit(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytes_rsplit_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_rsplit_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                if needle.is_none() {
                    let list_bits = rsplit_bytes_whitespace_to_list_maxsplit(
                        _py,
                        hay_bytes,
                        maxsplit,
                        |bytes| alloc_bytes(_py, bytes),
                    );
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits = rsplit_bytes_to_list_maxsplit(
                    _py,
                    hay_bytes,
                    needle_bytes,
                    maxsplit,
                    |bytes| alloc_bytes(_py, bytes),
                );
                let list_bits = match list_bits {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

fn bytes_strip_impl<F>(
    _py: &PyToken<'_>,
    hay_bits: u64,
    chars_bits: u64,
    type_id: u32,
    mut alloc: F,
    left: bool,
    right: bool,
) -> u64
where
    F: FnMut(&[u8]) -> *mut u8,
{
    const ASCII_WHITESPACE: [u8; 6] = [b' ', b'\t', b'\n', b'\r', 0x0b, 0x0c];
    let hay = obj_from_bits(hay_bits);
    let chars = obj_from_bits(chars_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        let strip_bytes = if chars.is_none() {
            &ASCII_WHITESPACE[..]
        } else {
            let Some(chars_ptr) = chars.as_ptr() else {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, chars)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            match bytes_like_slice(chars_ptr) {
                Some(slice) => slice,
                None => {
                    let msg = format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, chars)
                    );
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
            }
        };
        let (start, end) = bytes_strip_range(hay_bytes, strip_bytes, left, right);
        let ptr = alloc(&hay_bytes[start..end]);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    }
}

#[no_mangle]
pub extern "C" fn molt_bytes_strip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            true,
            true,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_lstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            true,
            false,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_rstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTES,
            |bytes| alloc_bytes(_py, bytes),
            false,
            true,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_split(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytearray_split_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_split_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                if needle.is_none() {
                    let list_bits = split_bytes_whitespace_to_list_maxsplit(
                        _py,
                        hay_bytes,
                        maxsplit,
                        |bytes| alloc_bytearray(_py, bytes),
                    );
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits = match split_bytes_to_list_maxsplit(
                    _py,
                    hay_bytes,
                    needle_bytes,
                    maxsplit,
                    |bytes| alloc_bytearray(_py, bytes),
                ) {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_rsplit(hay_bits: u64, needle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let maxsplit_bits = MoltObject::from_int(-1).bits();
        molt_bytearray_rsplit_max(hay_bits, needle_bits, maxsplit_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_rsplit_max(
    hay_bits: u64,
    needle_bits: u64,
    maxsplit_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let maxsplit = split_maxsplit_from_obj(_py, maxsplit_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                if needle.is_none() {
                    let list_bits = rsplit_bytes_whitespace_to_list_maxsplit(
                        _py,
                        hay_bytes,
                        maxsplit,
                        |bytes| alloc_bytearray(_py, bytes),
                    );
                    return list_bits.unwrap_or_else(|| MoltObject::none().bits());
                }
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                if needle_bytes.is_empty() {
                    return raise_exception::<_>(_py, "ValueError", "empty separator");
                }
                let list_bits = rsplit_bytes_to_list_maxsplit(
                    _py,
                    hay_bytes,
                    needle_bytes,
                    maxsplit,
                    |bytes| alloc_bytearray(_py, bytes),
                );
                let list_bits = match list_bits {
                    Some(val) => val,
                    None => return MoltObject::none().bits(),
                };
                return list_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_strip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            true,
            true,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_lstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            true,
            false,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_rstrip(hay_bits: u64, chars_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_strip_impl(
            _py,
            hay_bits,
            chars_bits,
            TYPE_ID_BYTEARRAY,
            |bytes| alloc_bytearray(_py, bytes),
            false,
            true,
        )
    })
}

fn bytes_decode_impl(
    _py: &PyToken<'_>,
    hay_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    type_id: u32,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let encoding = match parse_codec_arg(_py, encoding_bits, "decode", "encoding", "utf-8") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let errors = match parse_codec_arg(_py, errors_bits, "decode", "errors", "strict") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let Some(kind) = normalize_encoding(&encoding) else {
            let msg = format!("unknown encoding: {encoding}");
            return raise_exception::<_>(_py, "LookupError", &msg);
        };
        let bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
        let errors_known = matches!(errors.as_str(), "strict" | "ignore" | "replace");
        let result = if errors_known {
            decode_bytes_with_errors(bytes, kind, &errors)
        } else {
            match decode_bytes_with_errors(bytes, kind, "strict") {
                Ok((text, label)) => Ok((text, label)),
                Err((_failure, label)) => Err((DecodeFailure::UnknownErrorHandler(errors), label)),
            }
        };
        let out_bits = match result {
            Ok((text, _label)) => {
                let ptr = alloc_string(_py, text.as_bytes());
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err((DecodeFailure::UnknownErrorHandler(name), _label)) => {
                let msg = format!("unknown error handler name '{name}'");
                return raise_exception::<_>(_py, "LookupError", &msg);
            }
            Err((DecodeFailure::Byte { pos, byte, message }, label)) => {
                let msg = decode_error_byte(&label, byte, pos, message);
                return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
            }
            Err((
                DecodeFailure::Range {
                    start,
                    end,
                    message,
                },
                label,
            )) => {
                let msg = decode_error_range(&label, start, end, message);
                return raise_exception::<_>(_py, "UnicodeDecodeError", &msg);
            }
        };
        out_bits
    }
}

#[no_mangle]
pub extern "C" fn molt_bytes_decode(hay_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_decode_impl(_py, hay_bits, encoding_bits, errors_bits, TYPE_ID_BYTES)
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_decode(
    hay_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_decode_impl(_py, hay_bits, encoding_bits, errors_bits, TYPE_ID_BYTEARRAY)
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let replacement = obj_from_bits(replacement_bits);
        let count_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(count_bits))
        );
        let count = index_i64_from_obj(_py, count_bits, &count_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_ptr = match replacement.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_bytes = match bytes_like_slice(repl_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let out = if count < 0 {
                    match replace_bytes_impl(hay_bytes, needle_bytes, repl_bytes) {
                        Some(out) => out,
                        None => return MoltObject::none().bits(),
                    }
                } else {
                    replace_bytes_impl_limit(hay_bytes, needle_bytes, repl_bytes, count as usize)
                };
                let ptr = alloc_bytes(_py, &out);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let replacement = obj_from_bits(replacement_bits);
        let count_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(count_bits))
        );
        let count = index_i64_from_obj(_py, count_bits, &count_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_slice(needle_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_ptr = match replacement.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_bytes = match bytes_like_slice(repl_ptr) {
                    Some(slice) => slice,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let out = if count < 0 {
                    match replace_bytes_impl(hay_bytes, needle_bytes, repl_bytes) {
                        Some(out) => out,
                        None => return MoltObject::none().bits(),
                    }
                } else {
                    replace_bytes_impl_limit(hay_bytes, needle_bytes, repl_bytes, count as usize)
                };
                let ptr = alloc_bytearray(_py, &out);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[derive(Clone, Copy)]
enum BytesCtorKind {
    Bytes,
    Bytearray,
}

impl BytesCtorKind {
    fn name(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes",
            BytesCtorKind::Bytearray => "bytearray",
        }
    }

    fn ctor_label(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes()",
            BytesCtorKind::Bytearray => "bytearray()",
        }
    }

    fn range_error(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes must be in range(0, 256)",
            BytesCtorKind::Bytearray => "byte must be in range(0, 256)",
        }
    }

    fn non_iterable_message(self, type_name: &str) -> String {
        format!("cannot convert '{}' object to {}", type_name, self.name())
    }

    fn arg_type_message(self, arg: &str, type_name: &str) -> String {
        format!(
            "{} argument '{}' must be str, not {}",
            self.ctor_label(),
            arg,
            type_name
        )
    }
}

#[derive(Clone, Copy)]
enum EncodingKind {
    Utf8,
    Latin1,
    Ascii,
    Utf16,
    Utf16LE,
    Utf16BE,
    Utf32,
    Utf32LE,
    Utf32BE,
}

impl EncodingKind {
    fn name(self) -> &'static str {
        match self {
            EncodingKind::Utf8 => "utf-8",
            EncodingKind::Latin1 => "latin-1",
            EncodingKind::Ascii => "ascii",
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
            EncodingKind::Utf8
            | EncodingKind::Utf16
            | EncodingKind::Utf16LE
            | EncodingKind::Utf16BE
            | EncodingKind::Utf32
            | EncodingKind::Utf32LE
            | EncodingKind::Utf32BE => u32::MAX,
        }
    }
}

enum EncodeError {
    UnknownEncoding(String),
    UnknownErrorHandler(String),
    InvalidChar {
        encoding: &'static str,
        ch: char,
        pos: usize,
        limit: u32,
    },
}

fn normalize_encoding(name: &str) -> Option<EncodingKind> {
    let normalized = name.to_ascii_lowercase().replace('_', "-");
    match normalized.as_str() {
        "utf-8" | "utf8" => Some(EncodingKind::Utf8),
        "latin-1" | "latin1" | "iso-8859-1" | "iso8859-1" => Some(EncodingKind::Latin1),
        "ascii" => Some(EncodingKind::Ascii),
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

fn unicode_escape(ch: char) -> String {
    let code = ch as u32;
    if code <= 0xFF {
        format!("\\x{code:02x}")
    } else if code <= 0xFFFF {
        format!("\\u{code:04x}")
    } else {
        format!("\\U{code:08x}")
    }
}

fn encode_string_with_errors(
    text: &str,
    encoding: &str,
    errors: Option<&str>,
) -> Result<Vec<u8>, EncodeError> {
    let Some(kind) = normalize_encoding(encoding) else {
        return Err(EncodeError::UnknownEncoding(encoding.to_string()));
    };
    match kind {
        EncodingKind::Utf8 => Ok(text.as_bytes().to_vec()),
        EncodingKind::Utf16 => Ok(encode_utf16(text, native_endian(), true)),
        EncodingKind::Utf16LE => Ok(encode_utf16(text, Endian::Little, false)),
        EncodingKind::Utf16BE => Ok(encode_utf16(text, Endian::Big, false)),
        EncodingKind::Utf32 => Ok(encode_utf32(text, native_endian(), true)),
        EncodingKind::Utf32LE => Ok(encode_utf32(text, Endian::Little, false)),
        EncodingKind::Utf32BE => Ok(encode_utf32(text, Endian::Big, false)),
        EncodingKind::Latin1 | EncodingKind::Ascii => {
            let limit = kind.ordinal_limit();
            let mut out = Vec::with_capacity(text.len());
            for (idx, ch) in text.chars().enumerate() {
                let code = ch as u32;
                if code < limit {
                    out.push(code as u8);
                    continue;
                }
                match errors.unwrap_or("strict") {
                    "ignore" => continue,
                    "replace" => out.push(b'?'),
                    "strict" => {
                        return Err(EncodeError::InvalidChar {
                            encoding: kind.name(),
                            ch,
                            pos: idx,
                            limit,
                        });
                    }
                    "surrogateescape" | "surrogatepass" => {
                        return Err(EncodeError::InvalidChar {
                            encoding: kind.name(),
                            ch,
                            pos: idx,
                            limit,
                        });
                    }
                    other => {
                        return Err(EncodeError::UnknownErrorHandler(other.to_string()));
                    }
                }
            }
            Ok(out)
        }
    }
}

fn decode_error_byte(label: &str, byte: u8, pos: usize, message: &str) -> String {
    format!("'{label}' codec can't decode byte 0x{byte:02x} in position {pos}: {message}")
}

fn decode_error_range(label: &str, start: usize, end: usize, message: &str) -> String {
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

fn decode_ascii_with_errors(bytes: &[u8], errors: &str) -> Result<String, DecodeFailure> {
    let mut out = String::with_capacity(bytes.len());
    for (idx, &byte) in bytes.iter().enumerate() {
        if byte <= 0x7f {
            out.push(byte as char);
            continue;
        }
        match errors {
            "ignore" => {}
            "replace" => out.push('\u{FFFD}'),
            "strict" => {
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

fn decode_utf8_bytes_with_errors(bytes: &[u8], errors: &str) -> Result<String, DecodeFailure> {
    match decode_utf8_with_errors(bytes, errors) {
        Ok(text) => Ok(text),
        Err(err) => Err(DecodeFailure::Byte {
            pos: err.pos,
            byte: err.byte,
            message: err.message,
        }),
    }
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
) -> Result<String, DecodeFailure> {
    let data = if offset > 0 { &bytes[offset..] } else { bytes };
    let mut out = String::new();
    let mut idx = 0usize;
    while idx + 1 < data.len() {
        let unit = read_u16(data, idx, endian);
        if (0xD800..=0xDBFF).contains(&unit) {
            if idx + 3 >= data.len() {
                match errors {
                    "ignore" => {}
                    "replace" => out.push('\u{FFFD}'),
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
                break;
            }
            let next = read_u16(data, idx + 2, endian);
            if (0xDC00..=0xDFFF).contains(&next) {
                let high = (unit as u32) - 0xD800;
                let low = (next as u32) - 0xDC00;
                let code = 0x10000 + ((high << 10) | low);
                if let Some(ch) = char::from_u32(code) {
                    out.push(ch);
                }
                idx += 4;
                continue;
            }
            match errors {
                "ignore" => {}
                "replace" => out.push('\u{FFFD}'),
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
                "ignore" => {}
                "replace" => out.push('\u{FFFD}'),
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
        out.push(char::from_u32(unit as u32).unwrap_or('\u{FFFD}'));
        idx += 2;
    }
    if idx < data.len() {
        match errors {
            "ignore" => {}
            "replace" => out.push('\u{FFFD}'),
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
) -> Result<String, DecodeFailure> {
    let data = if offset > 0 { &bytes[offset..] } else { bytes };
    let mut out = String::new();
    let mut idx = 0usize;
    while idx + 3 < data.len() {
        let code = read_u32(data, idx, endian);
        if (0xD800..=0xDFFF).contains(&code) {
            match errors {
                "ignore" => {}
                "replace" => out.push('\u{FFFD}'),
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
                "replace" => out.push('\u{FFFD}'),
                "strict" => {
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
        if let Some(ch) = char::from_u32(code) {
            out.push(ch);
        }
        idx += 4;
    }
    if idx < data.len() {
        match errors {
            "ignore" => {}
            "replace" => out.push('\u{FFFD}'),
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
) -> Result<(String, String), (DecodeFailure, String)> {
    match kind {
        EncodingKind::Utf8 => match decode_utf8_bytes_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "utf-8".to_string())),
            Err(err) => Err((err, "utf-8".to_string())),
        },
        EncodingKind::Ascii => match decode_ascii_with_errors(bytes, errors) {
            Ok(text) => Ok((text, "ascii".to_string())),
            Err(err) => Err((err, "ascii".to_string())),
        },
        EncodingKind::Latin1 => Ok((
            bytes.iter().map(|b| char::from(*b)).collect(),
            "latin-1".to_string(),
        )),
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

// TODO(stdlib-compat, owner:runtime, milestone:TC1, priority:P2, status:partial): add
// full codec error handlers (surrogateescape/backslashreplace/etc) once Molt strings
// can represent surrogate code points.
fn parse_codec_arg(
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

fn bytes_from_count(_py: &PyToken<'_>, len: usize, type_id: u32) -> u64 {
    if type_id == TYPE_ID_BYTEARRAY {
        let ptr = alloc_bytearray_with_len(_py, len);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    let ptr = alloc_bytes_like_with_len(_py, len, type_id);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::write_bytes(data_ptr, 0, len);
    }
    MoltObject::from_ptr(ptr).bits()
}

fn bytes_item_to_u8(_py: &PyToken<'_>, bits: u64, kind: BytesCtorKind) -> Option<u8> {
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("'{}' object cannot be interpreted as an integer", type_name);
    let val = index_i64_from_obj(_py, bits, &msg);
    if exception_pending(_py) {
        return None;
    }
    if !(0..=255).contains(&val) {
        return raise_exception::<_>(_py, "ValueError", kind.range_error());
    }
    Some(val as u8)
}

fn bytes_collect_from_iter(
    _py: &PyToken<'_>,
    iter_bits: u64,
    kind: BytesCtorKind,
) -> Option<Vec<u8>> {
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
            let val_bits = elems[0];
            let byte = bytes_item_to_u8(_py, val_bits, kind)?;
            out.push(byte);
        }
    }
    Some(out)
}

fn bytes_from_obj_impl(_py: &PyToken<'_>, bits: u64, kind: BytesCtorKind) -> u64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = to_i64(obj) {
        if i < 0 {
            return raise_exception::<_>(_py, "ValueError", "negative count");
        }
        let len = match usize::try_from(i) {
            Ok(len) => len,
            Err(_) => {
                return raise_exception::<_>(
                    _py,
                    "OverflowError",
                    "cannot fit 'int' into an index-sized integer",
                );
            }
        };
        let type_id = match kind {
            BytesCtorKind::Bytes => TYPE_ID_BYTES,
            BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
        };
        return bytes_from_count(_py, len, type_id);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "string argument without an encoding",
                );
            }
            if type_id == TYPE_ID_BYTES && matches!(kind, BytesCtorKind::Bytes) {
                inc_ref_bits(_py, bits);
                return bits;
            }
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems = seq_vec_ref(ptr);
                let mut out = Vec::with_capacity(elems.len());
                for &elem in elems.iter() {
                    let Some(byte) = bytes_item_to_u8(_py, elem, kind) else {
                        return MoltObject::none().bits();
                    };
                    out.push(byte);
                }
                let out_ptr = match kind {
                    BytesCtorKind::Bytes => alloc_bytes(_py, &out),
                    BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
                };
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if let Some(slice) = bytes_like_slice(ptr) {
                let out_ptr = match kind {
                    BytesCtorKind::Bytes => alloc_bytes(_py, slice),
                    BytesCtorKind::Bytearray => alloc_bytearray(_py, slice),
                };
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if let Some(out) = memoryview_collect_bytes(ptr) {
                    let out_ptr = match kind {
                        BytesCtorKind::Bytes => alloc_bytes(_py, &out),
                        BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
                    };
                    if out_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(out_ptr).bits();
                }
            }
            if type_id == TYPE_ID_BIGINT {
                let big = bigint_ref(ptr);
                if big.is_negative() {
                    return raise_exception::<_>(_py, "ValueError", "negative count");
                }
                let Some(len) = big.to_usize() else {
                    return raise_exception::<_>(
                        _py,
                        "OverflowError",
                        "cannot fit 'int' into an index-sized integer",
                    );
                };
                let type_id = match kind {
                    BytesCtorKind::Bytes => TYPE_ID_BYTES,
                    BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                };
                return bytes_from_count(_py, len, type_id);
            }
            let index_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.index_name, b"__index__");
            let call_bits = attr_lookup_ptr(_py, ptr, index_name_bits);
            dec_ref_bits(_py, index_name_bits);
            if let Some(call_bits) = call_bits {
                let res_bits = call_callable0(_py, call_bits);
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let res_obj = obj_from_bits(res_bits);
                if let Some(i) = to_i64(res_obj) {
                    if i < 0 {
                        return raise_exception::<_>(_py, "ValueError", "negative count");
                    }
                    let len = match usize::try_from(i) {
                        Ok(len) => len,
                        Err(_) => {
                            return raise_exception::<_>(
                                _py,
                                "OverflowError",
                                "cannot fit 'int' into an index-sized integer",
                            );
                        }
                    };
                    let type_id = match kind {
                        BytesCtorKind::Bytes => TYPE_ID_BYTES,
                        BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                    };
                    return bytes_from_count(_py, len, type_id);
                }
                if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                    let big = bigint_ref(big_ptr);
                    if big.is_negative() {
                        return raise_exception::<_>(_py, "ValueError", "negative count");
                    }
                    let Some(len) = big.to_usize() else {
                        return raise_exception::<_>(
                            _py,
                            "OverflowError",
                            "cannot fit 'int' into an index-sized integer",
                        );
                    };
                    dec_ref_bits(_py, res_bits);
                    let type_id = match kind {
                        BytesCtorKind::Bytes => TYPE_ID_BYTES,
                        BytesCtorKind::Bytearray => TYPE_ID_BYTEARRAY,
                    };
                    return bytes_from_count(_py, len, type_id);
                }
                let res_type = class_name_for_error(type_of_bits(_py, res_bits));
                if res_obj.as_ptr().is_some() {
                    dec_ref_bits(_py, res_bits);
                }
                let msg = format!("__index__ returned non-int (type {res_type})");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let iter_bits = molt_iter(bits);
    if obj_from_bits(iter_bits).is_none() {
        let type_name = class_name_for_error(type_of_bits(_py, bits));
        let msg = kind.non_iterable_message(&type_name);
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let Some(out) = bytes_collect_from_iter(_py, iter_bits, kind) else {
        return MoltObject::none().bits();
    };
    let out_ptr = match kind {
        BytesCtorKind::Bytes => alloc_bytes(_py, &out),
        BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
    };
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

fn bytes_from_str_impl(
    _py: &PyToken<'_>,
    src_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    kind: BytesCtorKind,
) -> u64 {
    let encoding_obj = obj_from_bits(encoding_bits);
    let errors_obj = obj_from_bits(errors_bits);
    let encoding = if encoding_obj.is_none() {
        None
    } else {
        let Some(encoding) = string_obj_to_owned(encoding_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, encoding_bits));
            let msg = kind.arg_type_message("encoding", &type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        Some(encoding)
    };
    let errors = if errors_obj.is_none() {
        None
    } else {
        let Some(errors) = string_obj_to_owned(errors_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, errors_bits));
            let msg = kind.arg_type_message("errors", &type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        Some(errors)
    };
    let src_obj = obj_from_bits(src_bits);
    let Some(src_ptr) = src_obj.as_ptr() else {
        if encoding.is_some() {
            return raise_exception::<_>(_py, "TypeError", "encoding without a string argument");
        }
        if errors.is_some() {
            return raise_exception::<_>(_py, "TypeError", "errors without a string argument");
        }
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(src_ptr) != TYPE_ID_STRING {
            if encoding.is_some() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "encoding without a string argument",
                );
            }
            if errors.is_some() {
                return raise_exception::<_>(_py, "TypeError", "errors without a string argument");
            }
            return MoltObject::none().bits();
        }
    }
    let Some(encoding) = encoding else {
        return raise_exception::<_>(_py, "TypeError", "string argument without an encoding");
    };
    let text = string_obj_to_owned(src_obj).unwrap_or_default();
    let out = match encode_string_with_errors(&text, &encoding, errors.as_deref()) {
        Ok(bytes) => bytes,
        Err(EncodeError::UnknownEncoding(name)) => {
            let msg = format!("unknown encoding: {name}");
            return raise_exception::<_>(_py, "LookupError", &msg);
        }
        Err(EncodeError::UnknownErrorHandler(name)) => {
            let msg = format!("unknown error handler name '{name}'");
            return raise_exception::<_>(_py, "LookupError", &msg);
        }
        Err(EncodeError::InvalidChar {
            encoding,
            ch,
            pos,
            limit,
        }) => {
            let escaped = unicode_escape(ch);
            let msg = format!(
                "'{encoding}' codec can't encode character '{escaped}' in position {pos}: ordinal not in range({limit})"
            );
            return raise_exception::<_>(_py, "UnicodeEncodeError", &msg);
        }
    };
    let out_ptr = match kind {
        BytesCtorKind::Bytes => alloc_bytes(_py, &out),
        BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
    };
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

#[no_mangle]
pub extern "C" fn molt_bytes_from_obj(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_from_obj_impl(_py, bits, BytesCtorKind::Bytes)
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_from_obj(bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_from_obj_impl(_py, bits, BytesCtorKind::Bytearray)
    })
}

#[no_mangle]
pub extern "C" fn molt_bytes_from_str(src_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_from_str_impl(
            _py,
            src_bits,
            encoding_bits,
            errors_bits,
            BytesCtorKind::Bytes,
        )
    })
}

#[no_mangle]
pub extern "C" fn molt_bytearray_from_str(
    src_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        bytes_from_str_impl(
            _py,
            src_bits,
            encoding_bits,
            errors_bits,
            BytesCtorKind::Bytearray,
        )
    })
}

#[no_mangle]
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
                )
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
        return raise_exception::<_>(_py, "TypeError", "memoryview expects a bytes-like object");
    })
}

#[no_mangle]
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
                )
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
                    )
                }
            };
            let fmt = match memoryview_format_from_str(&format_str) {
            Some(val) => val,
            None => return raise_exception::<_>(_py,
                "ValueError",
                "memoryview: destination format must be a native single character format prefixed with an optional '@'",
            ),
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
                        )
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

#[no_mangle]
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
#[no_mangle]
pub unsafe extern "C" fn molt_buffer_export(obj_bits: u64, out_ptr: *mut BufferExport) -> i32 {
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

#[no_mangle]
pub extern "C" fn molt_index(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
                        if let Some(tup_ptr) = key.as_ptr() {
                            if object_type_id(tup_ptr) == TYPE_ID_TUPLE {
                                let elems = seq_vec_ref(tup_ptr);
                                if elems.is_empty() {
                                    let val = memoryview_read_scalar(
                                        _py,
                                        base,
                                        memoryview_offset(ptr),
                                        fmt,
                                    );
                                    return val.unwrap_or_else(|| MoltObject::none().bits());
                                }
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "invalid indexing of 0-dim memory",
                        );
                    }
                    if let Some(tup_ptr) = key.as_ptr() {
                        if object_type_id(tup_ptr) == TYPE_ID_TUPLE {
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
                                    let msg =
                                        format!("index out of bounds on dimension {}", dim + 1);
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
                    }
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
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
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
                            let bytes = if type_id == TYPE_ID_STRING {
                                std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr))
                            } else {
                                std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr))
                            };
                            let len = bytes.len() as isize;
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
                                    alloc_string(_py, &bytes[s..e])
                                } else if type_id == TYPE_ID_BYTES {
                                    alloc_bytes(_py, &bytes[s..e])
                                } else {
                                    alloc_bytearray(_py, &bytes[s..e])
                                }
                            } else {
                                let indices = collect_slice_indices(start, stop, step);
                                let mut out = Vec::with_capacity(indices.len());
                                for idx in indices {
                                    out.push(bytes[idx]);
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
                        let Ok(text) = std::str::from_utf8(bytes) else {
                            return MoltObject::none().bits();
                        };
                        let mut i = idx;
                        let len = text.chars().count() as i64;
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
                        let ch = match text.chars().nth(i as usize) {
                            Some(val) => val,
                            None => {
                                return raise_exception::<_>(
                                    _py,
                                    "IndexError",
                                    "string index out of range",
                                )
                            }
                        };
                        let mut buf = [0u8; 4];
                        let out = ch.encode_utf8(&mut buf);
                        let out_ptr = alloc_string(_py, out.as_bytes());
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
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
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
                    }
                    let type_err = format!(
                        "list indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
                        return MoltObject::none().bits();
                    };
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        if std::env::var("MOLT_DEBUG_INDEX").as_deref() == Ok("1") {
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
                    inc_ref_bits(_py, val);
                    return val;
                }
                if type_id == TYPE_ID_TUPLE {
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
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
                    }
                    let type_err = format!(
                        "tuple indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
                        return MoltObject::none().bits();
                    };
                    let len = tuple_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        if std::env::var("MOLT_DEBUG_INDEX").as_deref() == Ok("1") {
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
                    let type_err = format!(
                        "range indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(idx) = index_i64_with_overflow(
                        _py,
                        key_bits,
                        &type_err,
                        Some("range object index out of range"),
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let start = range_start(ptr);
                    let stop = range_stop(ptr);
                    let step = range_step(ptr);
                    let len = range_len_i64(start, stop, step);
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "range object index out of range",
                        );
                    }
                    let val = start + step * i;
                    return MoltObject::from_int(val).bits();
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if let Some(val) = dict_get_in_place(_py, dict_ptr, key_bits) {
                        inc_ref_bits(_py, val);
                        return val;
                    }
                    if object_type_id(ptr) != TYPE_ID_DICT {
                        if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__missing__") {
                            if let Some(call_bits) =
                                attr_lookup_ptr_allow_missing(_py, ptr, name_bits)
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
                    if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__class_getitem__") {
                        if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits)
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
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_store_index(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST {
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
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
                                return raise_exception::<_>(_py,
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
                    }
                    let type_err = format!(
                        "list indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
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
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
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
                                return raise_exception::<_>(_py,
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
                        if let Some(tup_ptr) = key.as_ptr() {
                            if object_type_id(tup_ptr) == TYPE_ID_TUPLE {
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
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "invalid indexing of 0-dim memory",
                        );
                    }
                    if let Some(tup_ptr) = key.as_ptr() {
                        if object_type_id(tup_ptr) == TYPE_ID_TUPLE {
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
                                    return raise_exception::<_>(_py,
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
                                    let msg =
                                        format!("index out of bounds on dimension {}", dim + 1);
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
                            let ok = memoryview_write_scalar(
                                _py,
                                data.as_mut_slice(),
                                pos,
                                fmt,
                                val_bits,
                            );
                            if ok.is_none() {
                                return MoltObject::none().bits();
                            }
                            return obj_bits;
                        }
                    }
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
                            if ndim != 1 {
                                return raise_exception::<_>(_py,
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
                                        return raise_exception::<_>(_py,
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
                                        return raise_exception::<_>(_py,
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
                                return raise_exception::<_>(_py,
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
                                data[start..end]
                                    .copy_from_slice(&src_bytes[idx..idx + fmt.itemsize]);
                                idx += fmt.itemsize;
                                pos += step_stride;
                            }
                            return obj_bits;
                        }
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

#[no_mangle]
pub extern "C" fn molt_del_index(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST {
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
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
                    }
                    let type_err = format!(
                        "list indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
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
                    if let Some(slice_ptr) = key.as_ptr() {
                        if object_type_id(slice_ptr) == TYPE_ID_SLICE {
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

#[no_mangle]
pub extern "C" fn molt_getitem_method(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_index(obj_bits, key_bits) })
}

#[no_mangle]
pub extern "C" fn molt_setitem_method(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = molt_store_index(obj_bits, key_bits, val_bits);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_delitem_method(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = molt_del_index(obj_bits, key_bits);
        MoltObject::none().bits()
    })
}

unsafe fn eq_bool_from_bits(_py: &PyToken<'_>, lhs_bits: u64, rhs_bits: u64) -> Option<bool> {
    let res_bits = molt_eq(lhs_bits, rhs_bits);
    if exception_pending(_py) {
        return None;
    }
    Some(is_truthy(_py, obj_from_bits(res_bits)))
}

#[no_mangle]
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
                    let found = dict_find_entry(_py, order, table, item_bits).is_some();
                    return MoltObject::from_bool(found).bits();
                }
                match type_id {
                    TYPE_ID_LIST => {
                        let snapshot = list_snapshot(_py, ptr);
                        let mut found = false;
                        for &elem_bits in snapshot.iter() {
                            let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                                Some(val) => val,
                                None => {
                                    list_snapshot_release(_py, snapshot);
                                    return MoltObject::none().bits();
                                }
                            };
                            if eq {
                                found = true;
                                break;
                            }
                        }
                        list_snapshot_release(_py, snapshot);
                        return MoltObject::from_bool(found).bits();
                    }
                    TYPE_ID_TUPLE => {
                        let elems = seq_vec_ref(ptr);
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
                        let found = set_find_entry(_py, order, table, item_bits).is_some();
                        return MoltObject::from_bool(found).bits();
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
                        let Some(val) = item.as_int() else {
                            return MoltObject::from_bool(false).bits();
                        };
                        let start = range_start(ptr);
                        let stop = range_stop(ptr);
                        let step = range_step(ptr);
                        if step == 0 {
                            return MoltObject::from_bool(false).bits();
                        }
                        let in_range = if step > 0 {
                            val >= start && val < stop
                        } else {
                            val <= start && val > stop
                        };
                        if !in_range {
                            return MoltObject::from_bool(false).bits();
                        }
                        let offset = val - start;
                        let step_abs = if step < 0 { -step } else { step };
                        let aligned = offset.rem_euclid(step_abs) == 0;
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
                                )
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
                                )
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
                                if let Some(exc_ptr) = exc_obj.as_ptr() {
                                    if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                                        let kind_bits = exception_kind_bits(exc_ptr);
                                        let kind_obj = obj_from_bits(kind_bits);
                                        if let Some(kind_ptr) = kind_obj.as_ptr() {
                                            if object_type_id(kind_ptr) == TYPE_ID_STRING {
                                                let bytes = std::slice::from_raw_parts(
                                                    string_bytes(kind_ptr),
                                                    string_len(kind_ptr),
                                                );
                                                if bytes == b"IndexError" {
                                                    is_index_error = true;
                                                }
                                            }
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
        return raise_exception::<_>(
            _py,
            "TypeError",
            &format!(
                "argument of type '{}' is not iterable",
                type_name(_py, container)
            ),
        );
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
        // TODO(perf, owner:runtime, milestone:RT2, priority:P2, status:planned):
        // pre-size dict using iterable length hints.
        let dict_bits = if class_bits == builtins.dict {
            molt_dict_new(0)
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

type DictUpdateSetter = unsafe fn(&PyToken<'_>, u64, u64, u64);

pub(crate) unsafe fn dict_update_set_in_place(
    _py: &PyToken<'_>,
    dict_bits: u64,
    key_bits: u64,
    val_bits: u64,
) {
    crate::gil_assert();
    let dict_obj = obj_from_bits(dict_bits);
    let Some(dict_ptr) = dict_obj.as_ptr() else {
        return;
    };
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return;
    }
    dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
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

pub(crate) unsafe fn dict_update_apply(
    _py: &PyToken<'_>,
    target_bits: u64,
    set_fn: DictUpdateSetter,
    other_bits: u64,
) -> u64 {
    let other_obj = obj_from_bits(other_bits);
    if let Some(ptr) = other_obj.as_ptr() {
        if object_type_id(ptr) == TYPE_ID_DICT {
            let iter_bits = molt_dict_items(other_bits);
            if obj_from_bits(iter_bits).is_none() {
                return MoltObject::none().bits();
            }
            let iter = molt_iter(iter_bits);
            if obj_from_bits(iter).is_none() {
                return MoltObject::none().bits();
            }
            let mut elem_index = 0usize;
            loop {
                let pair_bits = molt_iter_next(iter);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let item_bits = elems[0];
                match dict_pair_from_item(_py, item_bits) {
                    Ok((key, val)) => {
                        set_fn(_py, target_bits, key, val);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    Err(DictSeqError::NotIterable) => {
                        let msg = format!(
                            "cannot convert dictionary update sequence element #{elem_index} to a sequence"
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                    Err(DictSeqError::BadLen(len)) => {
                        let msg = format!(
                            "dictionary update sequence element #{elem_index} has length {len}; 2 is required"
                        );
                        return raise_exception::<_>(_py, "ValueError", &msg);
                    }
                    Err(DictSeqError::Exception) => {
                        return MoltObject::none().bits();
                    }
                }
                elem_index += 1;
            }
            return MoltObject::none().bits();
        }
        if let Some(keys_bits) = attr_name_bits_from_bytes(_py, b"keys") {
            let keys_method_bits = attr_lookup_ptr(_py, ptr, keys_bits);
            dec_ref_bits(_py, keys_bits);
            if let Some(keys_method_bits) = keys_method_bits {
                let keys_iterable = call_callable0(_py, keys_method_bits);
                let keys_iter = molt_iter(keys_iterable);
                if obj_from_bits(keys_iter).is_none() {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dict.update expects a mapping or iterable",
                    );
                }
                let Some(getitem_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dict.update expects a mapping or iterable",
                    );
                };
                let getitem_method_bits = attr_lookup_ptr(_py, ptr, getitem_bits);
                dec_ref_bits(_py, getitem_bits);
                let Some(getitem_method_bits) = getitem_method_bits else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "dict.update expects a mapping or iterable",
                    );
                };
                loop {
                    let pair_bits = molt_iter_next(keys_iter);
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        return MoltObject::none().bits();
                    }
                    let elems = seq_vec_ref(pair_ptr);
                    if elems.len() < 2 {
                        return MoltObject::none().bits();
                    }
                    let done_bits = elems[1];
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        break;
                    }
                    let key_bits = elems[0];
                    let val_bits = call_callable1(_py, getitem_method_bits, key_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    set_fn(_py, target_bits, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                return MoltObject::none().bits();
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
    }
    let iter = molt_iter(other_bits);
    if obj_from_bits(iter).is_none() {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        return raise_not_iterable(_py, other_bits);
    }
    let mut elem_index = 0usize;
    loop {
        let pair_bits = molt_iter_next(iter);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
            return MoltObject::none().bits();
        }
        let elems = seq_vec_ref(pair_ptr);
        if elems.len() < 2 {
            return MoltObject::none().bits();
        }
        let done_bits = elems[1];
        if is_truthy(_py, obj_from_bits(done_bits)) {
            break;
        }
        let item_bits = elems[0];
        match dict_pair_from_item(_py, item_bits) {
            Ok((key, val)) => {
                set_fn(_py, target_bits, key, val);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
            Err(DictSeqError::NotIterable) => {
                let msg = format!(
                    "cannot convert dictionary update sequence element #{elem_index} to a sequence"
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            Err(DictSeqError::BadLen(len)) => {
                let msg = format!(
                    "dictionary update sequence element #{elem_index} has length {len}; 2 is required"
                );
                return raise_exception::<_>(_py, "ValueError", &msg);
            }
            Err(DictSeqError::Exception) => {
                return MoltObject::none().bits();
            }
        }
        elem_index += 1;
    }
    MoltObject::none().bits()
}

#[no_mangle]
pub extern "C" fn molt_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !ensure_hashable(_py, key_bits) {
            return MoltObject::none().bits();
        }
        molt_store_index(dict_bits, key_bits, val_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_get(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.get expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.get expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.get expects dict");
            }
            if !ensure_hashable(_py, key_bits) {
                return MoltObject::none().bits();
            }
            if let Some(val) = dict_get_in_place(_py, dict_ptr, key_bits) {
                inc_ref_bits(_py, val);
                return val;
            }
            inc_ref_bits(_py, default_bits);
            default_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_pop(
    dict_bits: u64,
    key_bits: u64,
    default_bits: u64,
    has_default_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let has_default = obj_from_bits(has_default_bits).as_int().unwrap_or(0) != 0;
        let Some(ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.pop expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.pop expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.pop expects dict");
            }
            if !ensure_hashable(_py, key_bits) {
                return MoltObject::none().bits();
            }
            let order = dict_order(dict_ptr);
            let table = dict_table(dict_ptr);
            if let Some(entry_idx) = dict_find_entry(_py, order, table, key_bits) {
                let key_idx = entry_idx * 2;
                let val_idx = key_idx + 1;
                let key_val = order[key_idx];
                let val_val = order[val_idx];
                inc_ref_bits(_py, val_val);
                dec_ref_bits(_py, key_val);
                dec_ref_bits(_py, val_val);
                order.drain(key_idx..=val_idx);
                let entries = order.len() / 2;
                let capacity = dict_table_capacity(entries.max(1));
                dict_rebuild(_py, order, table, capacity);
                return val_val;
            }
            if has_default {
                inc_ref_bits(_py, default_bits);
                return default_bits;
            }
        }
        raise_key_error_with_key(_py, key_bits)
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_setdefault(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let Some(ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
            }
            if !ensure_hashable(_py, key_bits) {
                return MoltObject::none().bits();
            }
            if let Some(val) = dict_get_in_place(_py, dict_ptr, key_bits) {
                inc_ref_bits(_py, val);
                return val;
            }
            dict_set_in_place(_py, dict_ptr, key_bits, default_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, default_bits);
            default_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_update(dict_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let Some(ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
            };
            dict_update_apply(_py, dict_bits, dict_update_set_in_place, other_bits)
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_clear(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
            }
            dict_clear_in_place(_py, dict_ptr);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_copy(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
            }
            let pairs = dict_order(dict_ptr).clone();
            let out_ptr = alloc_dict_with_pairs(_py, pairs.as_slice());
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_popitem(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.popitem expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.popitem expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.popitem expects dict");
            }
            let order = dict_order(dict_ptr);
            if order.len() < 2 {
                return raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty");
            }
            let key_bits = order[order.len() - 2];
            let val_bits = order[order.len() - 1];
            let item_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
            if item_ptr.is_null() {
                return MoltObject::none().bits();
            }
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, val_bits);
            order.truncate(order.len() - 2);
            let entries = order.len() / 2;
            let table = dict_table(dict_ptr);
            let capacity = dict_table_capacity(entries.max(1));
            dict_rebuild(_py, order, table, capacity);
            MoltObject::from_ptr(item_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_update_kwstar(dict_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let Some(ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
            }
            let mapping_obj = obj_from_bits(mapping_bits);
            let Some(mapping_ptr) = mapping_obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            if object_type_id(mapping_ptr) == TYPE_ID_DICT {
                let order = dict_order(mapping_ptr);
                for idx in (0..order.len()).step_by(2) {
                    let key_bits = order[idx];
                    let val_bits = order[idx + 1];
                    let key_obj = obj_from_bits(key_bits);
                    let Some(key_ptr) = key_obj.as_ptr() else {
                        return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                    };
                    if object_type_id(key_ptr) != TYPE_ID_STRING {
                        return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                    }
                    dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                return MoltObject::none().bits();
            }
            let Some(keys_bits) = attr_name_bits_from_bytes(_py, b"keys") else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            let keys_method_bits = attr_lookup_ptr(_py, mapping_ptr, keys_bits);
            dec_ref_bits(_py, keys_bits);
            let Some(keys_method_bits) = keys_method_bits else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            let keys_iterable = call_callable0(_py, keys_method_bits);
            let iter_bits = molt_iter(keys_iterable);
            if obj_from_bits(iter_bits).is_none() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            }
            let Some(getitem_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            let getitem_method_bits = attr_lookup_ptr(_py, mapping_ptr, getitem_bits);
            dec_ref_bits(_py, getitem_bits);
            let Some(getitem_method_bits) = getitem_method_bits else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "argument after ** must be a mapping",
                );
            };
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let pair_obj = obj_from_bits(pair_bits);
                let Some(pair_ptr) = pair_obj.as_ptr() else {
                    return MoltObject::none().bits();
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return MoltObject::none().bits();
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let key_bits = elems[0];
                let key_obj = obj_from_bits(key_bits);
                let Some(key_ptr) = key_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                };
                if object_type_id(key_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                }
                let val_bits = call_callable1(_py, getitem_method_bits, key_bits);
                dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
            MoltObject::none().bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_set_add(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !ensure_hashable(_py, key_bits) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    set_add_in_place(_py, ptr, key_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_add(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !ensure_hashable(_py, key_bits) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_FROZENSET {
                    set_add_in_place(_py, ptr, key_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_discard(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    set_del_in_place(_py, ptr, key_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_remove(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    if set_del_in_place(_py, ptr, key_bits) {
                        return MoltObject::none().bits();
                    }
                    return raise_exception::<_>(_py, "KeyError", "set.remove(x): x not in set");
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_pop(set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    let order = set_order(ptr);
                    if order.is_empty() {
                        return raise_exception::<_>(_py, "KeyError", "pop from an empty set");
                    }
                    let key_bits = order.pop().unwrap_or_else(|| MoltObject::none().bits());
                    let entries = order.len();
                    let table = set_table(ptr);
                    let capacity = set_table_capacity(entries.max(1));
                    set_rebuild(_py, order, table, capacity);
                    inc_ref_bits(_py, key_bits);
                    return key_bits;
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_clear(set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    set_replace_entries(_py, ptr, &[]);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_copy_method(set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            match object_type_id(ptr) {
                TYPE_ID_SET => set_like_copy_bits(_py, ptr, TYPE_ID_SET),
                TYPE_ID_FROZENSET => {
                    inc_ref_bits(_py, set_bits);
                    set_bits
                }
                _ => MoltObject::none().bits(),
            }
        }
    })
}

unsafe fn set_from_iter_bits(_py: &PyToken<'_>, other_bits: u64) -> Option<u64> {
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

pub(crate) unsafe fn frozenset_from_iter_bits(_py: &PyToken<'_>, other_bits: u64) -> Option<u64> {
    let obj = obj_from_bits(other_bits);
    if let Some(ptr) = obj.as_ptr() {
        if object_type_id(ptr) == TYPE_ID_FROZENSET {
            inc_ref_bits(_py, other_bits);
            return Some(other_bits);
        }
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

#[no_mangle]
pub extern "C" fn molt_set_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let other = obj_from_bits(other_bits);
        if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
            unsafe {
                if object_type_id(set_ptr) == TYPE_ID_SET {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                        let entries = set_order(other_ptr);
                        for entry in entries.iter().copied() {
                            set_add_in_place(_py, set_ptr, entry);
                        }
                        return MoltObject::none().bits();
                    }
                    let iter_bits = molt_iter(other_bits);
                    if obj_from_bits(iter_bits).is_none() {
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
                        set_add_in_place(_py, set_ptr, val_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_intersection_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let other = obj_from_bits(other_bits);
        if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
            unsafe {
                if object_type_id(set_ptr) == TYPE_ID_SET {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                        let other_order = set_order(other_ptr);
                        let other_table = set_table(other_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let mut new_entries = Vec::with_capacity(set_entries.len());
                        for entry in set_entries {
                            if set_find_entry(_py, other_order, other_table, entry).is_some() {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        return MoltObject::none().bits();
                    }
                    let other_set_bits = set_from_iter_bits(_py, other_bits);
                    let Some(other_set_bits) = other_set_bits else {
                        return MoltObject::none().bits();
                    };
                    let other_set = obj_from_bits(other_set_bits);
                    let Some(other_ptr) = other_set.as_ptr() else {
                        dec_ref_bits(_py, other_set_bits);
                        return MoltObject::none().bits();
                    };
                    let other_order = set_order(other_ptr);
                    let other_table = set_table(other_ptr);
                    let set_entries = set_order(set_ptr).clone();
                    let mut new_entries = Vec::with_capacity(set_entries.len());
                    for entry in set_entries {
                        if set_find_entry(_py, other_order, other_table, entry).is_some() {
                            new_entries.push(entry);
                        }
                    }
                    set_replace_entries(_py, set_ptr, &new_entries);
                    dec_ref_bits(_py, other_set_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_difference_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let other = obj_from_bits(other_bits);
        if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
            unsafe {
                if object_type_id(set_ptr) == TYPE_ID_SET {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                        let other_order = set_order(other_ptr);
                        let other_table = set_table(other_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let mut new_entries = Vec::with_capacity(set_entries.len());
                        for entry in set_entries {
                            if set_find_entry(_py, other_order, other_table, entry).is_none() {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        return MoltObject::none().bits();
                    }
                    let iter_bits = molt_iter(other_bits);
                    if obj_from_bits(iter_bits).is_none() {
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
                        set_del_in_place(_py, set_ptr, val_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_symdiff_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let other = obj_from_bits(other_bits);
        if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
            unsafe {
                if object_type_id(set_ptr) == TYPE_ID_SET {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                        let other_order = set_order(other_ptr);
                        let other_table = set_table(other_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let set_table_ptr = set_table(set_ptr);
                        let mut new_entries =
                            Vec::with_capacity(set_entries.len() + other_order.len());
                        for entry in &set_entries {
                            if set_find_entry(_py, other_order, other_table, *entry).is_none() {
                                new_entries.push(*entry);
                            }
                        }
                        for entry in other_order.iter().copied() {
                            if set_find_entry(_py, set_entries.as_slice(), set_table_ptr, entry)
                                .is_none()
                            {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        return MoltObject::none().bits();
                    }
                    let other_set_bits = set_from_iter_bits(_py, other_bits);
                    let Some(other_set_bits) = other_set_bits else {
                        return MoltObject::none().bits();
                    };
                    let other_set = obj_from_bits(other_set_bits);
                    let Some(other_ptr) = other_set.as_ptr() else {
                        dec_ref_bits(_py, other_set_bits);
                        return MoltObject::none().bits();
                    };
                    let other_order = set_order(other_ptr);
                    let other_table = set_table(other_ptr);
                    let set_entries = set_order(set_ptr).clone();
                    let set_table_ptr = set_table(set_ptr);
                    let mut new_entries = Vec::with_capacity(set_entries.len() + other_order.len());
                    for entry in &set_entries {
                        if set_find_entry(_py, other_order, other_table, *entry).is_none() {
                            new_entries.push(*entry);
                        }
                    }
                    for entry in other_order.iter().copied() {
                        if set_find_entry(_py, set_entries.as_slice(), set_table_ptr, entry)
                            .is_none()
                        {
                            new_entries.push(entry);
                        }
                    }
                    set_replace_entries(_py, set_ptr, &new_entries);
                    dec_ref_bits(_py, other_set_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_update_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_SET {
                return MoltObject::none().bits();
            }
            let Some(others_ptr) = obj_from_bits(others_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(others_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            for &other_bits in seq_vec_ref(others_ptr).iter() {
                let _ = molt_set_update(set_bits, other_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_union_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_set_union_multi(set_bits, others_bits) })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_intersection_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_set_intersection_multi(set_bits, others_bits) })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_difference_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_set_difference_multi(set_bits, others_bits) })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_symmetric_difference(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_set_symmetric_difference(set_bits, other_bits) })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_isdisjoint(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_set_isdisjoint(set_bits, other_bits) })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_issubset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_set_issubset(set_bits, other_bits) })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_issuperset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { molt_set_issuperset(set_bits, other_bits) })
}

#[no_mangle]
pub extern "C" fn molt_frozenset_copy_method(set_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(ptr) == TYPE_ID_FROZENSET {
                inc_ref_bits(_py, set_bits);
                return set_bits;
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_intersection_update_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_SET {
                return MoltObject::none().bits();
            }
            let Some(others_ptr) = obj_from_bits(others_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(others_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            for &other_bits in seq_vec_ref(others_ptr).iter() {
                let _ = molt_set_intersection_update(set_bits, other_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_difference_update_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_SET {
                return MoltObject::none().bits();
            }
            let Some(others_ptr) = obj_from_bits(others_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(others_ptr) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            for &other_bits in seq_vec_ref(others_ptr).iter() {
                let _ = molt_set_difference_update(set_bits, other_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_symmetric_difference_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = molt_set_symdiff_update(set_bits, other_bits);
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_set_union_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if !is_set_like_type(type_id) {
                return MoltObject::none().bits();
            }
            let result_type_id = set_like_result_type_id(type_id);
            let mut result_bits = set_like_copy_bits(_py, ptr, result_type_id);
            if obj_from_bits(result_bits).is_none() {
                return MoltObject::none().bits();
            }
            let Some(others_ptr) = obj_from_bits(others_bits).as_ptr() else {
                return result_bits;
            };
            if object_type_id(others_ptr) != TYPE_ID_TUPLE {
                return result_bits;
            }
            for &other_bits in seq_vec_ref(others_ptr).iter() {
                let Some((other_ptr, drop_bits)) = set_like_ptr_from_bits(_py, other_bits) else {
                    dec_ref_bits(_py, result_bits);
                    return MoltObject::none().bits();
                };
                let result_ptr = obj_from_bits(result_bits)
                    .as_ptr()
                    .unwrap_or(std::ptr::null_mut());
                if result_ptr.is_null() {
                    if let Some(bits) = drop_bits {
                        dec_ref_bits(_py, bits);
                    }
                    dec_ref_bits(_py, result_bits);
                    return MoltObject::none().bits();
                }
                let new_bits = set_like_union(_py, result_ptr, other_ptr, result_type_id);
                if let Some(bits) = drop_bits {
                    dec_ref_bits(_py, bits);
                }
                dec_ref_bits(_py, result_bits);
                result_bits = new_bits;
                if obj_from_bits(result_bits).is_none() {
                    return MoltObject::none().bits();
                }
            }
            result_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_set_intersection_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if !is_set_like_type(type_id) {
                return MoltObject::none().bits();
            }
            let result_type_id = set_like_result_type_id(type_id);
            let mut result_bits = set_like_copy_bits(_py, ptr, result_type_id);
            if obj_from_bits(result_bits).is_none() {
                return MoltObject::none().bits();
            }
            let Some(others_ptr) = obj_from_bits(others_bits).as_ptr() else {
                return result_bits;
            };
            if object_type_id(others_ptr) != TYPE_ID_TUPLE {
                return result_bits;
            }
            for &other_bits in seq_vec_ref(others_ptr).iter() {
                let Some((other_ptr, drop_bits)) = set_like_ptr_from_bits(_py, other_bits) else {
                    dec_ref_bits(_py, result_bits);
                    return MoltObject::none().bits();
                };
                let result_ptr = obj_from_bits(result_bits)
                    .as_ptr()
                    .unwrap_or(std::ptr::null_mut());
                if result_ptr.is_null() {
                    if let Some(bits) = drop_bits {
                        dec_ref_bits(_py, bits);
                    }
                    dec_ref_bits(_py, result_bits);
                    return MoltObject::none().bits();
                }
                let new_bits = set_like_intersection(_py, result_ptr, other_ptr, result_type_id);
                if let Some(bits) = drop_bits {
                    dec_ref_bits(_py, bits);
                }
                dec_ref_bits(_py, result_bits);
                result_bits = new_bits;
                if obj_from_bits(result_bits).is_none() {
                    return MoltObject::none().bits();
                }
            }
            result_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_set_difference_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if !is_set_like_type(type_id) {
                return MoltObject::none().bits();
            }
            let result_type_id = set_like_result_type_id(type_id);
            let mut result_bits = set_like_copy_bits(_py, ptr, result_type_id);
            if obj_from_bits(result_bits).is_none() {
                return MoltObject::none().bits();
            }
            let Some(others_ptr) = obj_from_bits(others_bits).as_ptr() else {
                return result_bits;
            };
            if object_type_id(others_ptr) != TYPE_ID_TUPLE {
                return result_bits;
            }
            for &other_bits in seq_vec_ref(others_ptr).iter() {
                let Some((other_ptr, drop_bits)) = set_like_ptr_from_bits(_py, other_bits) else {
                    dec_ref_bits(_py, result_bits);
                    return MoltObject::none().bits();
                };
                let result_ptr = obj_from_bits(result_bits)
                    .as_ptr()
                    .unwrap_or(std::ptr::null_mut());
                if result_ptr.is_null() {
                    if let Some(bits) = drop_bits {
                        dec_ref_bits(_py, bits);
                    }
                    dec_ref_bits(_py, result_bits);
                    return MoltObject::none().bits();
                }
                let new_bits = set_like_difference(_py, result_ptr, other_ptr, result_type_id);
                if let Some(bits) = drop_bits {
                    dec_ref_bits(_py, bits);
                }
                dec_ref_bits(_py, result_bits);
                result_bits = new_bits;
                if obj_from_bits(result_bits).is_none() {
                    return MoltObject::none().bits();
                }
            }
            result_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_set_symmetric_difference(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if !is_set_like_type(type_id) {
                return MoltObject::none().bits();
            }
            let result_type_id = set_like_result_type_id(type_id);
            let Some((other_ptr, drop_bits)) = set_like_ptr_from_bits(_py, other_bits) else {
                return MoltObject::none().bits();
            };
            let result_bits = set_like_symdiff(_py, ptr, other_ptr, result_type_id);
            if let Some(bits) = drop_bits {
                dec_ref_bits(_py, bits);
            }
            result_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_set_isdisjoint(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(ptr)) {
                return MoltObject::none().bits();
            }
            let Some((other_ptr, drop_bits)) = set_like_ptr_from_bits(_py, other_bits) else {
                return MoltObject::none().bits();
            };
            let self_order = set_order(ptr);
            let other_order = set_order(other_ptr);
            let (probe_order, probe_table, output) = if self_order.len() <= other_order.len() {
                (other_order, set_table(other_ptr), self_order)
            } else {
                (self_order, set_table(ptr), other_order)
            };
            let mut disjoint = true;
            for &entry in output.iter() {
                if set_find_entry(_py, probe_order, probe_table, entry).is_some() {
                    disjoint = false;
                    break;
                }
            }
            if let Some(bits) = drop_bits {
                dec_ref_bits(_py, bits);
            }
            MoltObject::from_bool(disjoint).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_set_issubset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(ptr)) {
                return MoltObject::none().bits();
            }
            let Some((other_ptr, drop_bits)) = set_like_ptr_from_bits(_py, other_bits) else {
                return MoltObject::none().bits();
            };
            let self_order = set_order(ptr);
            let other_order = set_order(other_ptr);
            let other_table = set_table(other_ptr);
            let mut subset = true;
            for &entry in self_order.iter() {
                if set_find_entry(_py, other_order, other_table, entry).is_none() {
                    subset = false;
                    break;
                }
            }
            if let Some(bits) = drop_bits {
                dec_ref_bits(_py, bits);
            }
            MoltObject::from_bool(subset).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_set_issuperset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(set_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(ptr)) {
                return MoltObject::none().bits();
            }
            let Some((other_ptr, drop_bits)) = set_like_ptr_from_bits(_py, other_bits) else {
                return MoltObject::none().bits();
            };
            let self_order = set_order(ptr);
            let self_table = set_table(ptr);
            let other_order = set_order(other_ptr);
            let mut superset = true;
            for &entry in other_order.iter() {
                if set_find_entry(_py, self_order, self_table, entry).is_none() {
                    superset = false;
                    break;
                }
            }
            if let Some(bits) = drop_bits {
                dec_ref_bits(_py, bits);
            }
            MoltObject::from_bool(superset).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_enumerate(iterable_bits: u64, start_bits: u64, has_start_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let has_start = is_truthy(_py, obj_from_bits(has_start_bits));
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let index_bits = if has_start {
            let start_obj = obj_from_bits(start_bits);
            let mut is_int_like = start_obj.is_int() || start_obj.is_bool();
            if !is_int_like {
                if let Some(ptr) = start_obj.as_ptr() {
                    unsafe {
                        is_int_like = object_type_id(ptr) == TYPE_ID_BIGINT;
                    }
                }
            }
            if !is_int_like {
                return raise_exception::<_>(_py, "TypeError", "enumerate() start must be an int");
            }
            start_bits
        } else {
            MoltObject::from_int(0).bits()
        };
        let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
        let enum_ptr = alloc_object(_py, total, TYPE_ID_ENUMERATE);
        if enum_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            *(enum_ptr as *mut u64) = iter_bits;
            *(enum_ptr.add(std::mem::size_of::<u64>()) as *mut u64) = index_bits;
        }
        inc_ref_bits(_py, index_bits);
        MoltObject::from_ptr(enum_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_iter(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = maybe_ptr_from_bits(iter_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let target_bits = molt_dict_keys(dict_bits);
                    if obj_from_bits(target_bits).is_none() {
                        return MoltObject::none().bits();
                    }
                    let total = std::mem::size_of::<MoltHeader>()
                        + std::mem::size_of::<u64>()
                        + std::mem::size_of::<usize>();
                    let iter_ptr = alloc_object(_py, total, TYPE_ID_ITER);
                    if iter_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    *(iter_ptr as *mut u64) = target_bits;
                    iter_set_index(iter_ptr, 0);
                    return MoltObject::from_ptr(iter_ptr).bits();
                }
                if type_id == TYPE_ID_GENERATOR {
                    inc_ref_bits(_py, iter_bits);
                    return iter_bits;
                }
                if type_id == TYPE_ID_ENUMERATE {
                    inc_ref_bits(_py, iter_bits);
                    return iter_bits;
                }
                if type_id == TYPE_ID_ITER {
                    inc_ref_bits(_py, iter_bits);
                    return iter_bits;
                }
                if type_id == TYPE_ID_CALL_ITER
                    || type_id == TYPE_ID_REVERSED
                    || type_id == TYPE_ID_ZIP
                    || type_id == TYPE_ID_MAP
                    || type_id == TYPE_ID_FILTER
                {
                    inc_ref_bits(_py, iter_bits);
                    return iter_bits;
                }
                if type_id == TYPE_ID_LIST
                    || type_id == TYPE_ID_TUPLE
                    || type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                    || type_id == TYPE_ID_DICT
                    || type_id == TYPE_ID_SET
                    || type_id == TYPE_ID_FROZENSET
                    || type_id == TYPE_ID_DICT_KEYS_VIEW
                    || type_id == TYPE_ID_DICT_VALUES_VIEW
                    || type_id == TYPE_ID_DICT_ITEMS_VIEW
                    || type_id == TYPE_ID_RANGE
                {
                    let total = std::mem::size_of::<MoltHeader>()
                        + std::mem::size_of::<u64>()
                        + std::mem::size_of::<usize>();
                    let iter_ptr = alloc_object(_py, total, TYPE_ID_ITER);
                    if iter_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    inc_ref_bits(_py, iter_bits);
                    *(iter_ptr as *mut u64) = iter_bits;
                    iter_set_index(iter_ptr, 0);
                    return MoltObject::from_ptr(iter_ptr).bits();
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__iter__") {
                    if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        let res = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        if !is_iterator_bits(_py, res) {
                            let msg = format!(
                                "iter() returned non-iterator of type '{}'",
                                type_name(_py, obj_from_bits(res))
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        return res;
                    }
                    dec_ref_bits(_py, name_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_iter_checked(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let res = molt_iter(iter_bits);
        if obj_from_bits(res).is_none() {
            if exception_pending(_py) {
                return res;
            }
            return raise_not_iterable(_py, iter_bits);
        }
        res
    })
}

#[no_mangle]
pub extern "C" fn molt_iter_sentinel(callable_bits: u64, sentinel_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(callable_bits)));
        if !callable_ok {
            return raise_exception::<_>(_py, "TypeError", "iter(v, w): v must be callable");
        }
        let total = std::mem::size_of::<MoltHeader>() + 2 * std::mem::size_of::<u64>();
        let iter_ptr = alloc_object(_py, total, TYPE_ID_CALL_ITER);
        if iter_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            *(iter_ptr as *mut u64) = callable_bits;
            *(iter_ptr.add(std::mem::size_of::<u64>()) as *mut u64) = sentinel_bits;
        }
        inc_ref_bits(_py, callable_bits);
        inc_ref_bits(_py, sentinel_bits);
        MoltObject::from_ptr(iter_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_aiter(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let obj = obj_from_bits(obj_bits);
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__aiter__") else {
                return MoltObject::none().bits();
            };
            let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
                dec_ref_bits(_py, name_bits);
                let msg = format!("'{}' object is not async iterable", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, obj_ptr, name_bits) else {
                dec_ref_bits(_py, name_bits);
                let msg = format!("'{}' object is not async iterable", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            dec_ref_bits(_py, name_bits);
            let res = call_callable0(_py, call_bits);
            dec_ref_bits(_py, call_bits);
            res
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_iter_next(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = maybe_ptr_from_bits(iter_bits) {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_GENERATOR {
                    return molt_generator_send(iter_bits, MoltObject::none().bits());
                }
                if object_type_id(ptr) == TYPE_ID_ENUMERATE {
                    let iter_bits = enumerate_target_bits(ptr);
                    let pair_bits = molt_iter_next(iter_bits);
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        return MoltObject::none().bits();
                    }
                    let elems = seq_vec_ref(pair_ptr);
                    if elems.len() < 2 {
                        return MoltObject::none().bits();
                    }
                    let val_bits = elems[0];
                    let done_bits = elems[1];
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        return pair_bits;
                    }
                    let idx_bits = enumerate_index_bits(ptr);
                    let item_ptr = alloc_tuple(_py, &[idx_bits, val_bits]);
                    if item_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let item_bits = MoltObject::from_ptr(item_ptr).bits();
                    let done_false = MoltObject::from_bool(false).bits();
                    let out_ptr = alloc_tuple(_py, &[item_bits, done_false]);
                    if out_ptr.is_null() {
                        dec_ref_bits(_py, item_bits);
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, item_bits);
                    let next_bits = molt_add(idx_bits, MoltObject::from_int(1).bits());
                    if obj_from_bits(next_bits).is_none() {
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, idx_bits);
                    enumerate_set_index_bits(ptr, next_bits);
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                if object_type_id(ptr) == TYPE_ID_CALL_ITER {
                    let call_bits = call_iter_callable_bits(ptr);
                    let sentinel_bits = call_iter_sentinel_bits(ptr);
                    let val_bits = call_callable0(_py, call_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, val_bits);
                        return MoltObject::none().bits();
                    }
                    if obj_eq(_py, obj_from_bits(val_bits), obj_from_bits(sentinel_bits)) {
                        dec_ref_bits(_py, val_bits);
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let done_bits = MoltObject::from_bool(false).bits();
                    let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                    if tuple_ptr.is_null() {
                        dec_ref_bits(_py, val_bits);
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, val_bits);
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                if object_type_id(ptr) == TYPE_ID_MAP {
                    let func_bits = map_func_bits(ptr);
                    let iters_ptr = map_iters_ptr(ptr);
                    if iters_ptr.is_null() {
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let iters = &mut *iters_ptr;
                    if iters.is_empty() {
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let mut vals = Vec::with_capacity(iters.len());
                    for &iter_bits in iters.iter() {
                        let pair_bits = molt_iter_next(iter_bits);
                        let pair_obj = obj_from_bits(pair_bits);
                        let Some(pair_ptr) = pair_obj.as_ptr() else {
                            return MoltObject::none().bits();
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
                            return generator_done_tuple(_py, MoltObject::none().bits());
                        }
                        vals.push(val_bits);
                    }
                    let res_bits = if vals.len() == 1 {
                        call_callable1(_py, func_bits, vals[0])
                    } else {
                        let builder_bits = molt_callargs_new(vals.len() as u64, 0);
                        if builder_bits == 0 {
                            return MoltObject::none().bits();
                        }
                        for &val_bits in &vals {
                            let _ = molt_callargs_push_pos(builder_bits, val_bits);
                        }
                        molt_call_bind(func_bits, builder_bits)
                    };
                    if exception_pending(_py) {
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::none().bits();
                    }
                    let done_bits = MoltObject::from_bool(false).bits();
                    let tuple_ptr = alloc_tuple(_py, &[res_bits, done_bits]);
                    if tuple_ptr.is_null() {
                        dec_ref_bits(_py, res_bits);
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                if object_type_id(ptr) == TYPE_ID_FILTER {
                    let func_bits = filter_func_bits(ptr);
                    let iter_bits = filter_iter_bits(ptr);
                    loop {
                        let pair_bits = molt_iter_next(iter_bits);
                        let pair_obj = obj_from_bits(pair_bits);
                        let Some(pair_ptr) = pair_obj.as_ptr() else {
                            return MoltObject::none().bits();
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
                            return generator_done_tuple(_py, MoltObject::none().bits());
                        }
                        let keep = if obj_from_bits(func_bits).is_none() {
                            is_truthy(_py, obj_from_bits(val_bits))
                        } else {
                            let pred_bits = call_callable1(_py, func_bits, val_bits);
                            if exception_pending(_py) {
                                dec_ref_bits(_py, pred_bits);
                                return MoltObject::none().bits();
                            }
                            let keep = is_truthy(_py, obj_from_bits(pred_bits));
                            dec_ref_bits(_py, pred_bits);
                            keep
                        };
                        if keep {
                            let done_bits = MoltObject::from_bool(false).bits();
                            let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                            if tuple_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(tuple_ptr).bits();
                        }
                    }
                }
                if object_type_id(ptr) == TYPE_ID_ZIP {
                    let iters_ptr = zip_iters_ptr(ptr);
                    if iters_ptr.is_null() {
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let iters = &mut *iters_ptr;
                    if iters.is_empty() {
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let mut vals = Vec::with_capacity(iters.len());
                    for &iter_bits in iters.iter() {
                        let pair_bits = molt_iter_next(iter_bits);
                        let pair_obj = obj_from_bits(pair_bits);
                        let Some(pair_ptr) = pair_obj.as_ptr() else {
                            return MoltObject::none().bits();
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
                            return generator_done_tuple(_py, MoltObject::none().bits());
                        }
                        vals.push(val_bits);
                    }
                    let tuple_ptr = alloc_tuple(_py, vals.as_slice());
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    let val_bits = MoltObject::from_ptr(tuple_ptr).bits();
                    let done_bits = MoltObject::from_bool(false).bits();
                    let out_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                    if out_ptr.is_null() {
                        dec_ref_bits(_py, val_bits);
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, val_bits);
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                if object_type_id(ptr) == TYPE_ID_REVERSED {
                    let target_bits = reversed_target_bits(ptr);
                    let target_obj = obj_from_bits(target_bits);
                    let idx = reversed_index(ptr);
                    let (next_idx, val_bits, needs_drop) = if let Some(target_ptr) =
                        target_obj.as_ptr()
                    {
                        let target_type = object_type_id(target_ptr);
                        if target_type == TYPE_ID_LIST || target_type == TYPE_ID_TUPLE {
                            let elems = seq_vec_ref(target_ptr);
                            let len = elems.len();
                            let idx = idx.min(len);
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                (idx - 1, Some(elems[idx - 1]), false)
                            }
                        } else if target_type == TYPE_ID_RANGE {
                            let start = range_start(target_ptr);
                            let stop = range_stop(target_ptr);
                            let step = range_step(target_ptr);
                            let len = range_len_i64(start, stop, step) as usize;
                            let idx = idx.min(len);
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                let pos = (idx - 1) as i64;
                                let val = start + step * pos;
                                let bits = MoltObject::from_int(val).bits();
                                (idx - 1, Some(bits), false)
                            }
                        } else if target_type == TYPE_ID_STRING {
                            let bytes = std::slice::from_raw_parts(
                                string_bytes(target_ptr),
                                string_len(target_ptr),
                            );
                            let idx = idx.min(bytes.len());
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                let Ok(text) = std::str::from_utf8(&bytes[..idx]) else {
                                    return MoltObject::none().bits();
                                };
                                if let Some(ch) = text.chars().next_back() {
                                    let mut buf = [0u8; 4];
                                    let out = ch.encode_utf8(&mut buf);
                                    let out_ptr = alloc_string(_py, out.as_bytes());
                                    if out_ptr.is_null() {
                                        return MoltObject::none().bits();
                                    }
                                    let val_bits = MoltObject::from_ptr(out_ptr).bits();
                                    let next_idx = idx - ch.len_utf8();
                                    (next_idx, Some(val_bits), true)
                                } else {
                                    (0, None, false)
                                }
                            }
                        } else if target_type == TYPE_ID_BYTES || target_type == TYPE_ID_BYTEARRAY {
                            let bytes = std::slice::from_raw_parts(
                                bytes_data(target_ptr),
                                bytes_len(target_ptr),
                            );
                            let idx = idx.min(bytes.len());
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                let pos = idx - 1;
                                let val_bits = MoltObject::from_int(bytes[pos] as i64).bits();
                                (idx - 1, Some(val_bits), false)
                            }
                        } else if target_type == TYPE_ID_DICT {
                            let order = dict_order(target_ptr);
                            let len = order.len() / 2;
                            let idx = idx.min(len);
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                let entry = (idx - 1) * 2;
                                (idx - 1, Some(order[entry]), false)
                            }
                        } else if target_type == TYPE_ID_DICT_KEYS_VIEW
                            || target_type == TYPE_ID_DICT_VALUES_VIEW
                            || target_type == TYPE_ID_DICT_ITEMS_VIEW
                        {
                            let len = dict_view_len(target_ptr);
                            let idx = idx.min(len);
                            if idx == 0 {
                                (0, None, false)
                            } else if let Some((key_bits, val_bits)) =
                                dict_view_entry(target_ptr, idx - 1)
                            {
                                if target_type == TYPE_ID_DICT_ITEMS_VIEW {
                                    let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                                    if tuple_ptr.is_null() {
                                        return MoltObject::none().bits();
                                    }
                                    (idx - 1, Some(MoltObject::from_ptr(tuple_ptr).bits()), true)
                                } else if target_type == TYPE_ID_DICT_KEYS_VIEW {
                                    (idx - 1, Some(key_bits), false)
                                } else {
                                    (idx - 1, Some(val_bits), false)
                                }
                            } else {
                                (0, None, false)
                            }
                        } else {
                            (0, None, false)
                        }
                    } else {
                        (0, None, false)
                    };
                    if let Some(val_bits) = val_bits {
                        reversed_set_index(ptr, next_idx);
                        let done_bits = MoltObject::from_bool(false).bits();
                        let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            if needs_drop {
                                dec_ref_bits(_py, val_bits);
                            }
                            return MoltObject::none().bits();
                        }
                        if needs_drop {
                            dec_ref_bits(_py, val_bits);
                        }
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    }
                    reversed_set_index(ptr, 0);
                    return generator_done_tuple(_py, MoltObject::none().bits());
                }
                if object_type_id(ptr) != TYPE_ID_ITER {
                    if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__next__") {
                        if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                            dec_ref_bits(_py, name_bits);
                            exception_stack_push();
                            let val_bits = call_callable0(_py, call_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                let exc_bits = molt_exception_last();
                                let kind_bits = molt_exception_kind(exc_bits);
                                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                dec_ref_bits(_py, kind_bits);
                                if kind.as_deref() == Some("StopIteration") {
                                    let value_bits =
                                        if let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) {
                                            if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                                                exception_value_bits(exc_ptr)
                                            } else {
                                                MoltObject::none().bits()
                                            }
                                        } else {
                                            MoltObject::none().bits()
                                        };
                                    molt_exception_clear();
                                    exception_stack_pop(_py);
                                    let out_bits = generator_done_tuple(_py, value_bits);
                                    dec_ref_bits(_py, exc_bits);
                                    return out_bits;
                                }
                                dec_ref_bits(_py, exc_bits);
                                exception_stack_pop(_py);
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            let done_bits = MoltObject::from_bool(false).bits();
                            let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                            if tuple_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(tuple_ptr).bits();
                        }
                        dec_ref_bits(_py, name_bits);
                    }
                    return MoltObject::none().bits();
                }
                let target_bits = iter_target_bits(ptr);
                let target_obj = obj_from_bits(target_bits);
                let idx = iter_index(ptr);
                if let Some(target_ptr) = target_obj.as_ptr() {
                    let target_type = object_type_id(target_ptr);
                    if target_type == TYPE_ID_SET || target_type == TYPE_ID_FROZENSET {
                        let table = set_table(target_ptr);
                        let order = set_order(target_ptr);
                        let mut slot = idx;
                        while slot < table.len() && table[slot] == 0 {
                            slot += 1;
                        }
                        if slot >= table.len() {
                            iter_set_index(ptr, table.len());
                            let none_bits = MoltObject::none().bits();
                            let done_bits = MoltObject::from_bool(true).bits();
                            let tuple_ptr = alloc_tuple(_py, &[none_bits, done_bits]);
                            if tuple_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(tuple_ptr).bits();
                        }
                        let entry_idx = table[slot] - 1;
                        let val_bits = order[entry_idx];
                        iter_set_index(ptr, slot + 1);
                        let done_bits = MoltObject::from_bool(false).bits();
                        let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    }
                }
                if let Some(target_ptr) = target_obj.as_ptr() {
                    let target_type = object_type_id(target_ptr);
                    if target_type == TYPE_ID_STRING {
                        let bytes = std::slice::from_raw_parts(
                            string_bytes(target_ptr),
                            string_len(target_ptr),
                        );
                        if idx >= bytes.len() {
                            let none_bits = MoltObject::none().bits();
                            let done_bits = MoltObject::from_bool(true).bits();
                            let tuple_ptr = alloc_tuple(_py, &[none_bits, done_bits]);
                            if tuple_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(tuple_ptr).bits();
                        }
                        let tail = &bytes[idx..];
                        let Ok(text) = std::str::from_utf8(tail) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ch) = text.chars().next() else {
                            let none_bits = MoltObject::none().bits();
                            let done_bits = MoltObject::from_bool(true).bits();
                            let tuple_ptr = alloc_tuple(_py, &[none_bits, done_bits]);
                            if tuple_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(tuple_ptr).bits();
                        };
                        let mut buf = [0u8; 4];
                        let out = ch.encode_utf8(&mut buf);
                        let out_ptr = alloc_string(_py, out.as_bytes());
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        let val_bits = MoltObject::from_ptr(out_ptr).bits();
                        let next_idx = idx + ch.len_utf8();
                        iter_set_index(ptr, next_idx);
                        let done_bits = MoltObject::from_bool(false).bits();
                        let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            dec_ref_bits(_py, val_bits);
                            return MoltObject::none().bits();
                        }
                        dec_ref_bits(_py, val_bits);
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    }
                    if target_type == TYPE_ID_BYTES || target_type == TYPE_ID_BYTEARRAY {
                        let bytes = std::slice::from_raw_parts(
                            bytes_data(target_ptr),
                            bytes_len(target_ptr),
                        );
                        if idx >= bytes.len() {
                            let none_bits = MoltObject::none().bits();
                            let done_bits = MoltObject::from_bool(true).bits();
                            let tuple_ptr = alloc_tuple(_py, &[none_bits, done_bits]);
                            if tuple_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(tuple_ptr).bits();
                        }
                        let val_bits = MoltObject::from_int(bytes[idx] as i64).bits();
                        iter_set_index(ptr, idx + 1);
                        let done_bits = MoltObject::from_bool(false).bits();
                        let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    }
                    if target_type == TYPE_ID_LIST {
                        let elems = seq_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            iter_set_index(ptr, ITER_EXHAUSTED);
                            let none_bits = MoltObject::none().bits();
                            let done_bits = MoltObject::from_bool(true).bits();
                            let tuple_ptr = alloc_tuple(_py, &[none_bits, done_bits]);
                            if tuple_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::from_ptr(tuple_ptr).bits();
                        }
                        let val_bits = elems[idx];
                        iter_set_index(ptr, idx + 1);
                        let done_bits = MoltObject::from_bool(false).bits();
                        let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                        if tuple_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(tuple_ptr).bits();
                    }
                }
                let (len, next_val, needs_drop) = if let Some(target_ptr) = target_obj.as_ptr() {
                    let target_type = object_type_id(target_ptr);
                    if target_type == TYPE_ID_TUPLE {
                        let elems = seq_vec_ref(target_ptr);
                        if idx >= elems.len() {
                            (elems.len(), None, false)
                        } else {
                            (elems.len(), Some(elems[idx]), false)
                        }
                    } else if target_type == TYPE_ID_RANGE {
                        let start = range_start(target_ptr);
                        let stop = range_stop(target_ptr);
                        let step = range_step(target_ptr);
                        let len = range_len_i64(start, stop, step) as usize;
                        if idx >= len {
                            (len, None, false)
                        } else {
                            let val = start + step * idx as i64;
                            let bits = MoltObject::from_int(val).bits();
                            (len, Some(bits), false)
                        }
                    } else if target_type == TYPE_ID_DICT_KEYS_VIEW
                        || target_type == TYPE_ID_DICT_VALUES_VIEW
                        || target_type == TYPE_ID_DICT_ITEMS_VIEW
                    {
                        let len = dict_view_len(target_ptr);
                        if idx >= len {
                            (len, None, false)
                        } else if let Some((key_bits, val_bits)) = dict_view_entry(target_ptr, idx)
                        {
                            if target_type == TYPE_ID_DICT_ITEMS_VIEW {
                                let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                                if tuple_ptr.is_null() {
                                    return MoltObject::none().bits();
                                }
                                (len, Some(MoltObject::from_ptr(tuple_ptr).bits()), true)
                            } else if target_type == TYPE_ID_DICT_KEYS_VIEW {
                                (len, Some(key_bits), false)
                            } else {
                                (len, Some(val_bits), false)
                            }
                        } else {
                            (len, None, false)
                        }
                    } else {
                        (0, None, false)
                    }
                } else {
                    (0, None, false)
                };

                if let Some(val_bits) = next_val {
                    iter_set_index(ptr, idx + 1);
                    let done_bits = MoltObject::from_bool(false).bits();
                    let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    if needs_drop {
                        dec_ref_bits(_py, val_bits);
                    }
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                if idx >= len {
                    iter_set_index(ptr, len);
                }
                let none_bits = MoltObject::none().bits();
                let done_bits = MoltObject::from_bool(true).bits();
                let tuple_ptr = alloc_tuple(_py, &[none_bits, done_bits]);
                if tuple_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(tuple_ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_anext(obj_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        unsafe {
            let obj = obj_from_bits(obj_bits);
            let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__anext__") else {
                return MoltObject::none().bits();
            };
            let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits) else {
                dec_ref_bits(_py, name_bits);
                let msg = format!("'{}' object is not an async iterator", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            let Some(call_bits) = attr_lookup_ptr(_py, obj_ptr, name_bits) else {
                dec_ref_bits(_py, name_bits);
                let msg = format!("'{}' object is not an async iterator", type_name(_py, obj));
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
            dec_ref_bits(_py, name_bits);
            let res = call_callable0(_py, call_bits);
            dec_ref_bits(_py, call_bits);
            res
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_keys(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.keys expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.keys expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.keys expects dict");
            }
            let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
            let view_ptr = alloc_object(_py, total, TYPE_ID_DICT_KEYS_VIEW);
            if view_ptr.is_null() {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, dict_bits);
            *(view_ptr as *mut u64) = dict_bits;
            MoltObject::from_ptr(view_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_values(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.values expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.values expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.values expects dict");
            }
            let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
            let view_ptr = alloc_object(_py, total, TYPE_ID_DICT_VALUES_VIEW);
            if view_ptr.is_null() {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, dict_bits);
            *(view_ptr as *mut u64) = dict_bits;
            MoltObject::from_ptr(view_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dict_items(dict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.items expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict.items expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.items expects dict");
            }
            let total = std::mem::size_of::<MoltHeader>() + std::mem::size_of::<u64>();
            let view_ptr = alloc_object(_py, total, TYPE_ID_DICT_ITEMS_VIEW);
            if view_ptr.is_null() {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, dict_bits);
            *(view_ptr as *mut u64) = dict_bits;
            MoltObject::from_ptr(view_ptr).bits()
        }
    })
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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
    // TODO(perf, owner:runtime, milestone:TC1, priority:P2, status:planned): avoid list_snapshot allocations in membership/count/index by using a list mutation version or iterator guard.
    let elems = seq_vec_ref(list_ptr);
    let mut out = Vec::with_capacity(elems.len());
    for &elem in elems.iter() {
        inc_ref_bits(_py, elem);
        out.push(elem);
    }
    out
}

unsafe fn list_snapshot_release(_py: &PyToken<'_>, snapshot: Vec<u64>) {
    for elem in snapshot {
        dec_ref_bits(_py, elem);
    }
}

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

#[no_mangle]
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

fn heapq_lt(_py: &PyToken<'_>, a_bits: u64, b_bits: u64) -> Option<bool> {
    let res_bits = molt_lt(a_bits, b_bits);
    if exception_pending(_py) {
        return None;
    }
    obj_from_bits(res_bits).as_bool()
}

unsafe fn heapq_siftdown(
    _py: &PyToken<'_>,
    heap: &mut [u64],
    startpos: usize,
    mut pos: usize,
) -> bool {
    let newitem = heap[pos];
    while pos > startpos {
        let parentpos = (pos - 1) / 2;
        let parent = heap[parentpos];
        let lt = match heapq_lt(_py, newitem, parent) {
            Some(val) => val,
            None => return false,
        };
        if lt {
            heap[pos] = parent;
            pos = parentpos;
            continue;
        }
        break;
    }
    heap[pos] = newitem;
    true
}

unsafe fn heapq_siftup(_py: &PyToken<'_>, heap: &mut [u64], mut pos: usize) -> bool {
    let endpos = heap.len();
    let startpos = pos;
    let newitem = heap[pos];
    let mut childpos = 2 * pos + 1;
    while childpos < endpos {
        let rightpos = childpos + 1;
        if rightpos < endpos {
            let left_lt_right = match heapq_lt(_py, heap[childpos], heap[rightpos]) {
                Some(val) => val,
                None => return false,
            };
            if !left_lt_right {
                childpos = rightpos;
            }
        }
        heap[pos] = heap[childpos];
        pos = childpos;
        childpos = 2 * pos + 1;
    }
    heap[pos] = newitem;
    heapq_siftdown(_py, heap, startpos, pos)
}

#[no_mangle]
pub extern "C" fn molt_heapq_heapify(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            let len = elems.len();
            if len < 2 {
                return MoltObject::none().bits();
            }
            for idx in (0..len / 2).rev() {
                if !heapq_siftup(_py, elems, idx) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_heapq_heappush(list_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            elems.push(item_bits);
            inc_ref_bits(_py, item_bits);
            let len = elems.len();
            if len > 1 && !heapq_siftdown(_py, elems, 0, len - 1) {
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_heapq_heappop(list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            if elems.is_empty() {
                return raise_exception::<_>(_py, "IndexError", "index out of range");
            }
            let last = elems.pop().unwrap();
            if elems.is_empty() {
                inc_ref_bits(_py, last);
                dec_ref_bits(_py, last);
                return last;
            }
            let return_bits = elems[0];
            elems[0] = last;
            if !heapq_siftup(_py, elems, 0) {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, return_bits);
            dec_ref_bits(_py, return_bits);
            return_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_heapq_heapreplace(list_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            if elems.is_empty() {
                return raise_exception::<_>(_py, "IndexError", "index out of range");
            }
            let return_bits = elems[0];
            elems[0] = item_bits;
            inc_ref_bits(_py, item_bits);
            if !heapq_siftup(_py, elems, 0) {
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, return_bits);
            dec_ref_bits(_py, return_bits);
            return_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_heapq_heappushpop(list_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return MoltObject::none().bits();
            }
            let elems = seq_vec(list_ptr);
            if elems.is_empty() {
                inc_ref_bits(_py, item_bits);
                return item_bits;
            }
            let lt = match heapq_lt(_py, elems[0], item_bits) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            if lt {
                let return_bits = elems[0];
                elems[0] = item_bits;
                inc_ref_bits(_py, item_bits);
                if !heapq_siftup(_py, elems, 0) {
                    return MoltObject::none().bits();
                }
                inc_ref_bits(_py, return_bits);
                dec_ref_bits(_py, return_bits);
                return return_bits;
            }
            inc_ref_bits(_py, item_bits);
            item_bits
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_list_count(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let list_obj = obj_from_bits(list_bits);
        if let Some(ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_LIST {
                    let snapshot = list_snapshot(_py, ptr);
                    let mut count = 0i64;
                    for &elem_bits in snapshot.iter() {
                        let eq = match eq_bool_from_bits(_py, elem_bits, val_bits) {
                            Some(val) => val,
                            None => {
                                list_snapshot_release(_py, snapshot);
                                return MoltObject::none().bits();
                            }
                        };
                        if eq {
                            count += 1;
                        }
                    }
                    list_snapshot_release(_py, snapshot);
                    return MoltObject::from_int(count).bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
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
                    let snapshot = list_snapshot(_py, ptr);
                    let len = snapshot.len() as i64;
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
                        list_snapshot_release(_py, snapshot);
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
                        for idx in start..stop {
                            let elem_bits = snapshot[idx as usize];
                            let eq = match eq_bool_from_bits(_py, elem_bits, val_bits) {
                                Some(val) => val,
                                None => {
                                    list_snapshot_release(_py, snapshot);
                                    return MoltObject::none().bits();
                                }
                            };
                            if eq {
                                list_snapshot_release(_py, snapshot);
                                return MoltObject::from_int(idx).bits();
                            }
                        }
                    }
                    list_snapshot_release(_py, snapshot);
                    return raise_exception::<_>(_py, "ValueError", "list.index(x): x not in list");
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_list_index(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let missing = missing_bits(_py);
        molt_list_index_range(list_bits, val_bits, missing, missing)
    })
}

#[no_mangle]
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

#[no_mangle]
pub extern "C" fn molt_tuple_index(tuple_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let tuple_obj = obj_from_bits(tuple_bits);
        if let Some(ptr) = tuple_obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TUPLE {
                    let elems = seq_vec_ref(ptr);
                    for (idx, elem) in elems.iter().enumerate() {
                        let eq = match eq_bool_from_bits(_py, *elem, val_bits) {
                            Some(val) => val,
                            None => return MoltObject::none().bits(),
                        };
                        if eq {
                            return MoltObject::from_int(idx as i64).bits();
                        }
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_print_obj(val: u64) {
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(val);
        if let Some(b) = obj.as_bool() {
            if b {
                println!("True");
            } else {
                println!("False");
            }
            return;
        }
        if let Some(i) = obj.as_int() {
            println!("{i}");
            return;
        }
        if let Some(f) = obj.as_float() {
            println!("{}", format_float(f));
            return;
        }
        if obj.is_none() {
            println!("None");
            return;
        }
        if obj.is_pending() {
            println!("<pending>");
            return;
        }
        if let Some(ptr) = maybe_ptr_from_bits(val) {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_STRING {
                    let len = string_len(ptr);
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                    let s = String::from_utf8_lossy(bytes);
                    println!("{s}");
                    return;
                }
                if type_id == TYPE_ID_BIGINT {
                    println!("{}", bigint_ref(ptr));
                    return;
                }
                if type_id == TYPE_ID_BYTES {
                    let len = bytes_len(ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    let s = format_bytes(bytes);
                    println!("{s}");
                    return;
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    let len = bytes_len(ptr);
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                    let s = format!("bytearray({})", format_bytes(bytes));
                    println!("{s}");
                    return;
                }
                if type_id == TYPE_ID_RANGE {
                    let start = range_start(ptr);
                    let stop = range_stop(ptr);
                    let step = range_step(ptr);
                    println!("{}", format_range(start, stop, step));
                    return;
                }
                if type_id == TYPE_ID_SLICE {
                    println!("{}", format_slice(_py, ptr));
                    return;
                }
                if type_id == TYPE_ID_EXCEPTION {
                    println!("{}", format_exception_message(_py, ptr));
                    return;
                }
                if type_id == TYPE_ID_DATACLASS {
                    println!("{}", format_dataclass(_py, ptr));
                    return;
                }
                if type_id == TYPE_ID_BUFFER2D {
                    let buf_ptr = buffer2d_ptr(ptr);
                    if buf_ptr.is_null() {
                        println!("<buffer2d>");
                        return;
                    }
                    let buf = &*buf_ptr;
                    println!("<buffer2d {}x{}>", buf.rows, buf.cols);
                    return;
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    let len = memoryview_len(ptr);
                    let stride = memoryview_stride(ptr);
                    let readonly = memoryview_readonly(ptr);
                    println!("<memoryview len={len} stride={stride} readonly={readonly}>");
                    return;
                }
                if type_id == TYPE_ID_LIST {
                    let elems = seq_vec_ref(ptr);
                    let mut out = String::from("[");
                    for (idx, elem) in elems.iter().enumerate() {
                        if idx > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(&format_obj(_py, obj_from_bits(*elem)));
                    }
                    out.push(']');
                    println!("{out}");
                    return;
                }
                if type_id == TYPE_ID_TUPLE {
                    let guard = ReprGuard::new(_py, ptr);
                    if !guard.active() {
                        println!("(...)");
                        return;
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
                    println!("{out}");
                    return;
                }
                if type_id == TYPE_ID_DICT {
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
                    println!("{out}");
                    return;
                }
                if type_id == TYPE_ID_SET {
                    let order = set_order(ptr);
                    if order.is_empty() {
                        println!("set()");
                        return;
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
                    println!("{out}");
                    return;
                }
                if type_id == TYPE_ID_FROZENSET {
                    let order = set_order(ptr);
                    if order.is_empty() {
                        println!("frozenset()");
                        return;
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
                    println!("{out}");
                    return;
                }
                if type_id == TYPE_ID_DICT_KEYS_VIEW
                    || type_id == TYPE_ID_DICT_VALUES_VIEW
                    || type_id == TYPE_ID_DICT_ITEMS_VIEW
                {
                    let dict_bits = dict_view_dict_bits(ptr);
                    let dict_obj = obj_from_bits(dict_bits);
                    if let Some(dict_ptr) = dict_obj.as_ptr() {
                        if object_type_id(dict_ptr) == TYPE_ID_DICT {
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
                            println!("{out}");
                            return;
                        }
                    }
                }
                if type_id == TYPE_ID_ITER {
                    println!("<iter>");
                    return;
                }
            }
        }
        let rendered = format_obj_str(_py, obj);
        println!("{rendered}");
    })
}

#[no_mangle]
pub extern "C" fn molt_print_newline() {
    crate::with_gil_entry!(_py, {
        println!();
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
    if f.fract() == 0.0 {
        format!("{f:.1}")
    } else {
        f.to_string()
    }
}

fn format_range(start: i64, stop: i64, step: i64) -> String {
    if step == 1 {
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

fn format_generic_alias(_py: &PyToken<'_>, ptr: *mut u8) -> String {
    unsafe {
        let origin_bits = generic_alias_origin_bits(ptr);
        let args_bits = generic_alias_args_bits(ptr);
        let origin_obj = obj_from_bits(origin_bits);
        let render_arg = |arg_bits: u64| {
            let arg_obj = obj_from_bits(arg_bits);
            if let Some(arg_ptr) = arg_obj.as_ptr() {
                if object_type_id(arg_ptr) == TYPE_ID_TYPE {
                    let name = string_obj_to_owned(obj_from_bits(class_name_bits(arg_ptr)))
                        .unwrap_or_default();
                    if !name.is_empty() {
                        return name;
                    }
                }
            }
            format_obj(_py, arg_obj)
        };
        let origin_repr = if let Some(origin_ptr) = origin_obj.as_ptr() {
            if object_type_id(origin_ptr) == TYPE_ID_TYPE {
                let name = string_obj_to_owned(obj_from_bits(class_name_bits(origin_ptr)))
                    .unwrap_or_default();
                if name.is_empty() {
                    format_obj(_py, origin_obj)
                } else {
                    name
                }
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
        if !desc.repr {
            return format!("<{}>", desc.name);
        }
        let fields = dataclass_fields_ref(ptr);
        let mut out = String::new();
        out.push_str(&desc.name);
        out.push('(');
        for (idx, name) in desc.field_names.iter().enumerate() {
            if idx > 0 {
                out.push_str(", ");
            }
            out.push_str(name);
            out.push('=');
            let val = fields
                .get(idx)
                .copied()
                .unwrap_or(MoltObject::none().bits());
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
            let mut stack = stack.borrow_mut();
            if stack.iter().any(|slot| slot.0 == ptr) {
                return false;
            }
            stack.push(PtrSlot(ptr));
            true
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
            REPR_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();
                if let Some(pos) = stack.iter().rposition(|slot| slot.0 == self.ptr) {
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

pub(crate) fn format_obj_str(_py: &PyToken<'_>, obj: MoltObject) -> String {
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
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
    if let Some(f) = obj.as_float() {
        return format_float(f);
    }
    if obj.is_none() {
        return "None".to_string();
    }
    if obj.is_pending() {
        return "<pending>".to_string();
    }
    if let Some(ptr) = maybe_ptr_from_bits(obj.bits()) {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                let len = string_len(ptr);
                let bytes = std::slice::from_raw_parts(string_bytes(ptr), len);
                let s = String::from_utf8_lossy(bytes);
                return format_string_repr(&s);
            }
            if type_id == TYPE_ID_BIGINT {
                return bigint_ref(ptr).to_string();
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
                return format_range(range_start(ptr), range_stop(ptr), range_step(ptr));
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
                return format!("<class '{name}'>");
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
                return format_dataclass(_py, ptr);
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
                let dict_bits = dict_view_dict_bits(ptr);
                let dict_obj = obj_from_bits(dict_bits);
                if let Some(dict_ptr) = dict_obj.as_ptr() {
                    if object_type_id(dict_ptr) == TYPE_ID_DICT {
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
            }
            if type_id == TYPE_ID_ITER {
                return "<iter>".to_string();
            }
            let repr_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.repr_name, b"__repr__");
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, repr_name_bits) {
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
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
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

struct FormatSpec {
    fill: char,
    align: Option<char>,
    sign: Option<char>,
    alternate: bool,
    width: Option<usize>,
    grouping: Option<char>,
    precision: Option<usize>,
    ty: Option<char>,
}

fn parse_format_spec(spec: &str) -> Result<FormatSpec, &'static str> {
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
    } else if let Some(c1) = first {
        if matches!(c1, '<' | '>' | '^' | '=') {
            align = Some(c1);
            chars.next();
        }
    }

    if let Some(ch) = chars.peek().copied() {
        if matches!(ch, '+' | '-' | ' ') {
            sign = Some(ch);
            chars.next();
        }
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

    if let Some(ch) = chars.peek().copied() {
        if ch == ',' || ch == '_' {
            grouping = Some(ch);
            chars.next();
        }
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
    if let Some(first) = exp_text.chars().next() {
        if first == '+' || first == '-' {
            sign = first;
            exp_text = &exp_text[1..];
        }
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

fn format_int_with_spec(
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, (&'static str, &'static str)> {
    if spec.precision.is_some() {
        return Err(("ValueError", "precision not allowed in integer format"));
    }
    let ty = spec.ty.unwrap_or('d');
    let mut value = if let Some(i) = obj.as_int() {
        BigInt::from(i)
    } else if let Some(b) = obj.as_bool() {
        BigInt::from(if b { 1 } else { 0 })
    } else if let Some(ptr) = bigint_ptr_from_bits(obj.bits()) {
        unsafe { bigint_ref(ptr).clone() }
    } else {
        return Err(("TypeError", "format requires int"));
    };
    if ty == 'c' {
        if value.is_negative() {
            return Err(("ValueError", "format c requires non-negative int"));
        }
        let code = value
            .to_u32()
            .ok_or(("ValueError", "format c out of range"))?;
        let ch = std::char::from_u32(code).ok_or(("ValueError", "format c out of range"))?;
        return Ok(format_string_with_spec(ch.to_string(), spec));
    }
    let base = match ty {
        'b' => 2,
        'o' => 8,
        'x' | 'X' => 16,
        'd' | 'n' => 10,
        _ => return Err(("ValueError", "unsupported int format type")),
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
    } else if let Some(sign) = spec.sign {
        if sign == '+' || sign == ' ' {
            prefix.push(sign);
        }
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

fn format_float_with_spec(
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, (&'static str, &'static str)> {
    let val = if let Some(f) = obj.as_float() {
        f
    } else if let Some(i) = obj.as_int() {
        i as f64
    } else if let Some(b) = obj.as_bool() {
        if b {
            1.0
        } else {
            0.0
        }
    } else {
        return Err(("TypeError", "format requires float"));
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
    } else if let Some(sign) = spec.sign {
        if sign == '+' || sign == ' ' {
            prefix.push(sign);
        }
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
            _ => return Err(("ValueError", "unsupported float format type")),
        }
    };
    body = normalize_exponent(&body, upper);
    if upper {
        body = body.replace('e', "E");
    }
    if spec.alternate && !body.contains('.') && !body.contains('E') && !body.contains('e') {
        body.push('.');
    }
    if let Some(sep) = spec.grouping {
        if !body.contains('e') && !body.contains('E') {
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
    }
    if ty == '%' {
        body.push('%');
    }
    Ok(apply_alignment(&prefix, &body, spec, '>'))
}

fn format_with_spec(
    _py: &PyToken<'_>,
    obj: MoltObject,
    spec: &FormatSpec,
) -> Result<String, (&'static str, &'static str)> {
    match spec.ty {
        Some('s') => Ok(format_string_with_spec(format_obj_str(_py, obj), spec)),
        Some('d') | Some('b') | Some('o') | Some('x') | Some('X') | Some('n') | Some('c') => {
            format_int_with_spec(obj, spec)
        }
        Some('f') | Some('F') | Some('e') | Some('E') | Some('g') | Some('G') | Some('%') => {
            format_float_with_spec(obj, spec)
        }
        Some(_) => Err(("ValueError", "unsupported format type")),
        None => {
            if obj.as_float().is_some() {
                format_float_with_spec(obj, spec)
            } else if obj.as_bool().is_some() {
                Ok(format_string_with_spec(format_obj_str(_py, obj), spec))
            } else if obj.as_int().is_some() || bigint_ptr_from_bits(obj.bits()).is_some() {
                format_int_with_spec(obj, spec)
            } else {
                Ok(format_string_with_spec(format_obj_str(_py, obj), spec))
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn molt_inc_ref_obj(bits: u64) {
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            unsafe { molt_inc_ref(ptr) };
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_dec_ref_obj(bits: u64) {
    crate::with_gil_entry!(_py, {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            unsafe { molt_dec_ref(ptr) };
        }
    })
}

// TODO(semantics, owner:runtime, milestone:TC2, priority:P1, status:partial): move dict
// subclass storage out of instance __dict__ so mapping contents are not exposed via attributes.
unsafe fn dict_subclass_storage_bits(_py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    let class_bits = object_class_bits(ptr);
    if class_bits == 0 {
        return None;
    }
    let builtins = builtin_classes(_py);
    if !issubclass_bits(class_bits, builtins.dict) {
        return None;
    }
    let mut dict_bits = instance_dict_bits(ptr);
    if dict_bits == 0 {
        let dict_ptr = alloc_dict_with_pairs(_py, &[]);
        if dict_ptr.is_null() {
            return None;
        }
        dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        instance_set_dict_bits(_py, ptr, dict_bits);
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    if object_type_id(dict_ptr) != TYPE_ID_DICT {
        return None;
    }
    let storage_name_bits = intern_static_name(
        _py,
        &runtime_state(_py).interned.molt_dict_data_name,
        b"__molt_dict_data__",
    );
    if let Some(storage_bits) = dict_get_in_place(_py, dict_ptr, storage_name_bits) {
        return Some(storage_bits);
    }
    let storage_ptr = alloc_dict_with_pairs(_py, &[]);
    if storage_ptr.is_null() {
        return None;
    }
    let storage_bits = MoltObject::from_ptr(storage_ptr).bits();
    dict_set_in_place(_py, dict_ptr, storage_name_bits, storage_bits);
    if exception_pending(_py) {
        dec_ref_bits(_py, storage_bits);
        return None;
    }
    dec_ref_bits(_py, storage_bits);
    dict_get_in_place(_py, dict_ptr, storage_name_bits)
}

unsafe fn dict_like_bits_from_ptr(_py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    if object_type_id(ptr) == TYPE_ID_DICT {
        return Some(MoltObject::from_ptr(ptr).bits());
    }
    if object_type_id(ptr) == TYPE_ID_OBJECT {
        return dict_subclass_storage_bits(_py, ptr);
    }
    None
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
        if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                dict_clear_in_place(_py, dict_ptr);
            }
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
    if let Some(i) = obj.as_int() {
        return i != 0;
    }
    if let Some(f) = obj.as_float() {
        return f != 0.0;
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_STRING {
                return string_len(ptr) > 0;
            }
            if type_id == TYPE_ID_BYTES {
                return bytes_len(ptr) > 0;
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
                let len = range_len_i64(range_start(ptr), range_stop(ptr), range_step(ptr));
                return len > 0;
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
                        let msg = format!(
                            "__bool__ should return bool, returned {res_type}"
                        );
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
                        let msg = format!(
                            "'{}' object cannot be interpreted as an integer",
                            res_type
                        );
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
                TYPE_ID_FUNCTION => Cow::Borrowed("function"),
                TYPE_ID_BOUND_METHOD => Cow::Borrowed("method"),
                TYPE_ID_CODE => Cow::Borrowed("code"),
                TYPE_ID_MODULE => Cow::Borrowed("module"),
                TYPE_ID_TYPE => Cow::Borrowed("type"),
                TYPE_ID_GENERIC_ALIAS => Cow::Borrowed("types.GenericAlias"),
                TYPE_ID_GENERATOR => Cow::Borrowed("generator"),
                TYPE_ID_ASYNC_GENERATOR => Cow::Borrowed("async_generator"),
                TYPE_ID_ENUMERATE => Cow::Borrowed("enumerate"),
                TYPE_ID_CALL_ITER => Cow::Borrowed("callable_iterator"),
                TYPE_ID_REVERSED => Cow::Borrowed("reversed"),
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

enum BinaryDunderOutcome {
    Value(u64),
    NotImplemented,
    Missing,
    Error,
}

unsafe fn call_dunder_raw(
    _py: &PyToken<'_>,
    raw_bits: u64,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
    arg_bits: u64,
) -> BinaryDunderOutcome {
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

unsafe fn call_binary_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
    rop_name_bits: u64,
) -> Option<u64> {
    let lhs_obj = obj_from_bits(lhs_bits);
    let rhs_obj = obj_from_bits(rhs_bits);
    let lhs_ptr = lhs_obj.as_ptr();
    let rhs_ptr = rhs_obj.as_ptr();

    let lhs_type_bits = type_of_bits(_py, lhs_bits);
    let rhs_type_bits = type_of_bits(_py, rhs_bits);
    let lhs_type_ptr = obj_from_bits(lhs_type_bits).as_ptr();
    let rhs_type_ptr = obj_from_bits(rhs_type_bits).as_ptr();

    let lhs_op_raw = lhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, op_name_bits));
    let rhs_rop_raw =
        rhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, rop_name_bits));

    let rhs_is_subclass =
        rhs_type_bits != lhs_type_bits && issubclass_bits(rhs_type_bits, lhs_type_bits);
    let prefer_rhs = rhs_is_subclass
        && rhs_rop_raw.is_some()
        && lhs_op_raw.map_or(true, |lhs_raw| lhs_raw != rhs_rop_raw.unwrap());

    let mut tried_rhs = false;
    if prefer_rhs {
        if let (Some(rhs_ptr), Some(rhs_type_ptr), Some(rhs_raw)) =
            (rhs_ptr, rhs_type_ptr, rhs_rop_raw)
        {
            tried_rhs = true;
            match call_dunder_raw(_py, rhs_raw, rhs_type_ptr, Some(rhs_ptr), lhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }
    }

    if let (Some(lhs_ptr), Some(lhs_type_ptr), Some(lhs_raw)) = (lhs_ptr, lhs_type_ptr, lhs_op_raw)
    {
        match call_dunder_raw(_py, lhs_raw, lhs_type_ptr, Some(lhs_ptr), rhs_bits) {
            BinaryDunderOutcome::Value(bits) => return Some(bits),
            BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
            BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
        }
    }

    if !tried_rhs {
        if let (Some(rhs_ptr), Some(rhs_type_ptr), Some(rhs_raw)) =
            (rhs_ptr, rhs_type_ptr, rhs_rop_raw)
        {
            match call_dunder_raw(_py, rhs_raw, rhs_type_ptr, Some(rhs_ptr), lhs_bits) {
                BinaryDunderOutcome::Value(bits) => return Some(bits),
                BinaryDunderOutcome::Error => return Some(MoltObject::none().bits()),
                BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
            }
        }
    }
    None
}

unsafe fn call_inplace_dunder(
    _py: &PyToken<'_>,
    lhs_bits: u64,
    rhs_bits: u64,
    op_name_bits: u64,
) -> Option<u64> {
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

pub(crate) fn obj_eq(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> bool {
    if let (Some(li), Some(ri)) = (to_i64(lhs), to_i64(rhs)) {
        return li == ri;
    }
    if lhs.is_none() && rhs.is_none() {
        return true;
    }
    if lhs.is_float() || rhs.is_float() {
        if let (Some(lf), Some(rf)) = (to_f64(lhs), to_f64(rhs)) {
            return lf == rf;
        }
    }
    if let (Some(l_big), Some(r_big)) = (to_bigint(lhs), to_bigint(rhs)) {
        return l_big == r_big;
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
                    let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                    let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                    return l_bytes == r_bytes;
                }
                if is_set_like_type(ltype) && is_set_like_type(rtype) {
                    let l_elems = set_order(lp);
                    let r_elems = set_order(rp);
                    if l_elems.len() != r_elems.len() {
                        return false;
                    }
                    let r_table = set_table(rp);
                    for key_bits in l_elems.iter().copied() {
                        if set_find_entry(_py, r_elems, r_table, key_bits).is_none() {
                            return false;
                        }
                    }
                    return true;
                }
                return false;
            }
            if ltype == TYPE_ID_STRING {
                let l_len = string_len(lp);
                let r_len = string_len(rp);
                if l_len != r_len {
                    return false;
                }
                let l_bytes = std::slice::from_raw_parts(string_bytes(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(string_bytes(rp), r_len);
                return l_bytes == r_bytes;
            }
            if ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY {
                let l_len = bytes_len(lp);
                let r_len = bytes_len(rp);
                if l_len != r_len {
                    return false;
                }
                let l_bytes = std::slice::from_raw_parts(bytes_data(lp), l_len);
                let r_bytes = std::slice::from_raw_parts(bytes_data(rp), r_len);
                return l_bytes == r_bytes;
            }
            if ltype == TYPE_ID_TUPLE {
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                for (l_val, r_val) in l_elems.iter().zip(r_elems.iter()) {
                    if !obj_eq(_py, obj_from_bits(*l_val), obj_from_bits(*r_val)) {
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
            if ltype == TYPE_ID_LIST {
                let l_elems = seq_vec_ref(lp);
                let r_elems = seq_vec_ref(rp);
                if l_elems.len() != r_elems.len() {
                    return false;
                }
                for (l_val, r_val) in l_elems.iter().zip(r_elems.iter()) {
                    if !obj_eq(_py, obj_from_bits(*l_val), obj_from_bits(*r_val)) {
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
                    let Some(r_entry_idx) = dict_find_entry(_py, r_pairs, r_table, key_bits) else {
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
                    if set_find_entry(_py, r_elems, r_table, key_bits).is_none() {
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
                for (l_val, r_val) in l_vals.iter().zip(r_vals.iter()) {
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
        Err(_) => random_hash_secret(),
    }
}

fn fatal_hash_seed(value: &str) -> ! {
    eprintln!(
        "Fatal Python error: PYTHONHASHSEED must be \"random\" or an integer in range [0; {PY_HASHSEED_MAX}]"
    );
    eprintln!("PYTHONHASHSEED={value}");
    std::process::exit(1);
}

fn random_hash_secret() -> HashSecret {
    let mut bytes = [0u8; 16];
    if let Err(err) = getrandom(&mut bytes) {
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
        for &byte in bytes {
            self.tail |= (byte as u64) << (8 * self.ntail);
            self.ntail += 1;
            if self.ntail == 8 {
                self.process_block(self.tail);
                self.tail = 0;
                self.ntail = 0;
            }
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
    if hash == -1 {
        -2
    } else {
        hash
    }
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
    if value == mask {
        0
    } else {
        value as u64
    }
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

fn hash_string_bytes(_py: &PyToken<'_>, bytes: &[u8]) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let secret = hash_secret(_py);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return hash_bytes_with_secret(bytes, secret);
    };
    let mut max_codepoint = 0u32;
    for ch in text.chars() {
        max_codepoint = max_codepoint.max(ch as u32);
    }
    let mut hasher = SipHasher13::new(secret.k0, secret.k1);
    if max_codepoint <= 0xff {
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

fn hash_string(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let header = unsafe { header_from_obj_ptr(ptr) };
    let cached = unsafe { (*header).state };
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let len = unsafe { string_len(ptr) };
    let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), len) };
    let hash = hash_string_bytes(_py, bytes);
    unsafe {
        (*header).state = hash.wrapping_add(1);
    }
    hash
}

fn hash_bytes_cached(_py: &PyToken<'_>, ptr: *mut u8, bytes: &[u8]) -> i64 {
    let header = unsafe { header_from_obj_ptr(ptr) };
    let cached = unsafe { (*header).state };
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let hash = hash_bytes(_py, bytes);
    unsafe {
        (*header).state = hash.wrapping_add(1);
    }
    hash
}

fn hash_int(val: i64) -> i64 {
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
        return (acc as i32) as i64;
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
        return (acc as i32) as i64;
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
        return Some((acc as i32) as i64);
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

fn hash_pointer(ptr: u64) -> i64 {
    let hash = (ptr >> 4) as i64;
    fix_hash(hash)
}

fn hash_unhashable(_py: &PyToken<'_>, obj: MoltObject) -> i64 {
    let name = type_name(_py, obj);
    let msg = format!("unhashable type: '{name}'");
    return raise_exception::<_>(_py, "TypeError", &msg);
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

fn hash_bits_signed(_py: &PyToken<'_>, bits: u64) -> i64 {
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
            if type_id == TYPE_ID_TUPLE {
                return hash_tuple(_py, ptr);
            }
            if type_id == TYPE_ID_GENERIC_ALIAS {
                return hash_generic_alias(_py, ptr);
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
        }
        return hash_pointer(ptr as u64);
    }
    hash_pointer(bits)
}

fn hash_bits(_py: &PyToken<'_>, bits: u64) -> u64 {
    hash_bits_signed(_py, bits) as u64
}

fn ensure_hashable(_py: &PyToken<'_>, key_bits: u64) -> bool {
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

fn dict_insert_entry(_py: &PyToken<'_>, order: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let key_bits = order[entry_idx * 2];
    let mut slot = (hash_bits(_py, key_bits) as usize) & mask;
    loop {
        if table[slot] == 0 {
            table[slot] = entry_idx + 1;
            return;
        }
        slot = (slot + 1) & mask;
    }
}

fn dict_rebuild(_py: &PyToken<'_>, order: &[u64], table: &mut Vec<usize>, capacity: usize) {
    table.clear();
    table.resize(capacity, 0);
    let entry_count = order.len() / 2;
    for entry_idx in 0..entry_count {
        dict_insert_entry(_py, order, table, entry_idx);
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
    let mask = table.len() - 1;
    let mut slot = (hash_bits(_py, key_bits) as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        let entry_idx = entry - 1;
        let entry_key = order[entry_idx * 2];
        if obj_eq(_py, obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
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
    loop {
        if table[slot] == 0 {
            table[slot] = entry_idx + 1;
            return;
        }
        slot = (slot + 1) & mask;
    }
}

fn set_rebuild(_py: &PyToken<'_>, order: &[u64], table: &mut Vec<usize>, capacity: usize) {
    crate::gil_assert();
    table.clear();
    table.resize(capacity, 0);
    for entry_idx in 0..order.len() {
        set_insert_entry(_py, order, table, entry_idx);
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
        let entry_idx = entry - 1;
        let entry_key = order[entry_idx];
        if obj_eq(_py, obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
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
    crate::gil_assert();
    if !ensure_hashable(_py, key_bits) {
        return;
    }
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    if let Some(entry_idx) = dict_find_entry(_py, order, table, key_bits) {
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
    }

    order.push(key_bits);
    order.push(val_bits);
    inc_ref_bits(_py, key_bits);
    inc_ref_bits(_py, val_bits);
    let entry_idx = order.len() / 2 - 1;
    dict_insert_entry(_py, order, table, entry_idx);
}

pub(crate) unsafe fn set_add_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) {
    crate::gil_assert();
    if !ensure_hashable(_py, key_bits) {
        return;
    }
    let order = set_order(ptr);
    let table = set_table(ptr);
    if set_find_entry(_py, order, table, key_bits).is_some() {
        return;
    }

    let new_entries = order.len() + 1;
    let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
    if needs_resize {
        let capacity = set_table_capacity(new_entries);
        set_rebuild(_py, order, table, capacity);
    }

    order.push(key_bits);
    inc_ref_bits(_py, key_bits);
    let entry_idx = order.len() - 1;
    set_insert_entry(_py, order, table, entry_idx);
}

pub(crate) unsafe fn dict_get_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
) -> Option<u64> {
    if !ensure_hashable(_py, key_bits) {
        return None;
    }
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    dict_find_entry(_py, order, table, key_bits).map(|idx| order[idx * 2 + 1])
}

pub(crate) unsafe fn set_del_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) -> bool {
    if !ensure_hashable(_py, key_bits) {
        return false;
    }
    let order = set_order(ptr);
    let table = set_table(ptr);
    let Some(entry_idx) = set_find_entry(_py, order, table, key_bits) else {
        return false;
    };
    let key_val = order[entry_idx];
    dec_ref_bits(_py, key_val);
    order.remove(entry_idx);
    let entries = order.len();
    let capacity = set_table_capacity(entries.max(1));
    set_rebuild(_py, order, table, capacity);
    true
}

pub(crate) unsafe fn set_replace_entries(_py: &PyToken<'_>, ptr: *mut u8, entries: &[u64]) {
    crate::gil_assert();
    let order = set_order(ptr);
    for entry in order.iter().copied() {
        dec_ref_bits(_py, entry);
    }
    order.clear();
    for entry in entries {
        inc_ref_bits(_py, *entry);
        order.push(*entry);
    }
    let table = set_table(ptr);
    let capacity = set_table_capacity(order.len().max(1));
    set_rebuild(_py, order, table, capacity);
}

pub(crate) unsafe fn dict_del_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) -> bool {
    if !ensure_hashable(_py, key_bits) {
        return false;
    }
    let order = dict_order(ptr);
    let table = dict_table(ptr);
    let Some(entry_idx) = dict_find_entry(_py, order, table, key_bits) else {
        return false;
    };
    let key_idx = entry_idx * 2;
    let val_idx = key_idx + 1;
    let key_val = order[key_idx];
    let val_val = order[val_idx];
    dec_ref_bits(_py, key_val);
    dec_ref_bits(_py, val_val);
    order.drain(key_idx..=val_idx);
    let entries = order.len() / 2;
    let capacity = dict_table_capacity(entries.max(1));
    dict_rebuild(_py, order, table, capacity);
    true
}

pub(crate) unsafe fn dict_clear_in_place(_py: &PyToken<'_>, ptr: *mut u8) {
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
