//! Iterator and range operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_iter_*` / `molt_range_*` / `molt_enumerate_*`
//! etc. is a separate linker symbol so that `wasm-ld --gc-sections` can drop
//! unused entries.

use crate::object::{
    ObjectAuxPreselection, dec_ref_ptr, inc_ref_ptr, object_init_poll_fn_unpublished,
    object_init_state_unpublished,
};
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

use super::ops::{
    alloc_range_from_bigints, dict_like_bits_from_ptr, eq_bool_from_bits, list_from_iter_bits,
    range_components_bigint, range_components_i64, range_index_for_candidate, range_len_bigint,
    range_len_i128, range_lookup_candidate, range_value_at_index_i64,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_range_new(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let start_type = class_name_for_error(type_of_bits(_py, start_bits));
        let start_err = format!("'{start_type}' object cannot be interpreted as an integer");
        let Some(start) = index_bigint_from_obj(_py, start_bits, &start_err) else {
            return MoltObject::none().bits();
        };
        let stop_type = class_name_for_error(type_of_bits(_py, stop_bits));
        let stop_err = format!("'{stop_type}' object cannot be interpreted as an integer");
        let Some(stop) = index_bigint_from_obj(_py, stop_bits, &stop_err) else {
            return MoltObject::none().bits();
        };
        let step_type = class_name_for_error(type_of_bits(_py, step_bits));
        let step_err = format!("'{step_type}' object cannot be interpreted as an integer");
        let Some(step) = index_bigint_from_obj(_py, step_bits, &step_err) else {
            return MoltObject::none().bits();
        };
        if step.is_zero() {
            return raise_exception::<_>(_py, "ValueError", "range() arg 3 must not be zero");
        }
        alloc_range_from_bigints(_py, start, stop, step)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_from_range(start_bits: u64, stop_bits: u64, step_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let range_bits = molt_range_new(start_bits, stop_bits, step_bits);
        if obj_from_bits(range_bits).is_none() {
            return MoltObject::none().bits();
        }
        let Some(range_ptr) = obj_from_bits(range_bits).as_ptr() else {
            dec_ref_bits(_py, range_bits);
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(range_ptr) != TYPE_ID_RANGE {
                dec_ref_bits(_py, range_bits);
                return MoltObject::none().bits();
            }
            if let Some((start, stop, step)) = range_components_i64(range_ptr) {
                let len = range_len_i128(start, stop, step);
                if len <= 0 {
                    dec_ref_bits(_py, range_bits);
                    let list_ptr = alloc_list(_py, &[]);
                    return if list_ptr.is_null() {
                        MoltObject::none().bits()
                    } else {
                        MoltObject::from_ptr(list_ptr).bits()
                    };
                }
                if len <= usize::MAX as i128 {
                    let len_usize = len as usize;
                    let mut out = Vec::with_capacity(len_usize);
                    let mut cur = start;
                    for idx in 0..len_usize {
                        out.push(MoltObject::from_int(cur).bits());
                        if idx + 1 < len_usize {
                            let Some(next) = cur.checked_add(step) else {
                                let out_bits = list_from_iter_bits(_py, range_bits)
                                    .unwrap_or_else(|| MoltObject::none().bits());
                                dec_ref_bits(_py, range_bits);
                                return out_bits;
                            };
                            cur = next;
                        }
                    }
                    dec_ref_bits(_py, range_bits);
                    let list_ptr = alloc_list(_py, out.as_slice());
                    return if list_ptr.is_null() {
                        MoltObject::none().bits()
                    } else {
                        MoltObject::from_ptr(list_ptr).bits()
                    };
                }
            }
            let out_bits =
                list_from_iter_bits(_py, range_bits).unwrap_or_else(|| MoltObject::none().bits());
            dec_ref_bits(_py, range_bits);
            out_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_range_count(range_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let range_obj = obj_from_bits(range_bits);
        let Some(range_ptr) = range_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "count() argument must be range");
        };
        unsafe {
            if object_type_id(range_ptr) != TYPE_ID_RANGE {
                return raise_exception::<_>(_py, "TypeError", "count() argument must be range");
            }
        }

        if let Some((start, stop, step)) = range_components_bigint(range_ptr)
            && let Some(candidate) = range_lookup_candidate(_py, val_bits)
        {
            let hit = range_index_for_candidate(&start, &stop, &step, &candidate).is_some();
            return MoltObject::from_int(if hit { 1 } else { 0 }).bits();
        }

        if let Some((start, stop, step)) = range_components_i64(range_ptr) {
            let len = range_len_i128(start, stop, step);
            if len <= 0 {
                return MoltObject::from_int(0).bits();
            }
            let mut count = BigInt::from(0);
            let mut idx = 0i128;
            while idx < len {
                let Some(value) = range_value_at_index_i64(start, stop, step, idx) else {
                    break;
                };
                // Box the candidate element at full range; a range whose values
                // exceed the inline window (e.g. range(2**60, 2**60 + 3)) would
                // otherwise be compared as a truncated element. `int_bits_from_i64`
                // may return a heap BigInt, so release it after the comparison.
                let elem_bits = int_bits_from_i64(_py, value);
                let eq_opt = unsafe { eq_bool_from_bits(_py, elem_bits, val_bits) };
                dec_ref_bits(_py, elem_bits);
                let Some(eq) = eq_opt else {
                    return MoltObject::none().bits();
                };
                if eq {
                    count += 1;
                }
                idx += 1;
            }
            if let Some(i) = count.to_i64() {
                return int_bits_from_i64(_py, i);
            }
            return int_bits_from_bigint(_py, count);
        }
        MoltObject::from_int(0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_range_index(range_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let range_obj = obj_from_bits(range_bits);
        let Some(range_ptr) = range_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "index() argument must be range");
        };
        unsafe {
            if object_type_id(range_ptr) != TYPE_ID_RANGE {
                return raise_exception::<_>(_py, "TypeError", "index() argument must be range");
            }
        }

        if let Some((start, stop, step)) = range_components_bigint(range_ptr)
            && let Some(candidate) = range_lookup_candidate(_py, val_bits)
        {
            if let Some(idx) = range_index_for_candidate(&start, &stop, &step, &candidate) {
                if let Some(i) = idx.to_i64() {
                    // Full-range boxing — a large range index (e.g.
                    // range(2**62).index(2**61)) exceeds the inline window and
                    // would be silently truncated by `from_int`.
                    return int_bits_from_i64(_py, i);
                }
                return int_bits_from_bigint(_py, idx);
            }
            return raise_exception::<_>(_py, "ValueError", "sequence.index(x): x not in sequence");
        }

        if let Some((start, stop, step)) = range_components_i64(range_ptr) {
            let len = range_len_i128(start, stop, step);
            let mut idx = 0i128;
            while idx < len {
                let Some(value) = range_value_at_index_i64(start, stop, step, idx) else {
                    break;
                };
                let elem_bits = MoltObject::from_int(value).bits();
                let Some(eq) = (unsafe { eq_bool_from_bits(_py, elem_bits, val_bits) }) else {
                    return MoltObject::none().bits();
                };
                if eq {
                    if let Ok(i) = i64::try_from(idx) {
                        return MoltObject::from_int(i).bits();
                    }
                    return int_bits_from_bigint(_py, BigInt::from(idx));
                }
                idx += 1;
            }
        }
        raise_exception::<_>(_py, "ValueError", "sequence.index(x): x not in sequence")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_enumerate_builtin(iter_bits: u64, start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_next_builtin(iter_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        let pair_bits = molt_iter_next(iter_bits);
        let pair_obj = obj_from_bits(pair_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            // A non-tuple (None) result means either the underlying __next__
            // (or an internal allocation) raised -- propagate that exception
            // unchanged -- or the object is simply not an iterator. CPython's
            // next() rejects the latter with a type-qualified message *before*
            // consulting any default, so this covers next(x) and next(x, d).
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let msg = format!(
                "'{}' object is not an iterator",
                type_name(_py, obj_from_bits(iter_bits))
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let msg = format!(
                    "'{}' object is not an iterator",
                    type_name(_py, obj_from_bits(iter_bits))
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let Some((val_bits, done_bits)) = crate::object::seq_access::tuple_pair(pair_ptr)
            else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let msg = format!(
                    "'{}' object is not an iterator",
                    type_name(_py, obj_from_bits(iter_bits))
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            };
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

pub(crate) unsafe fn map_new_impl(_py: &PyToken<'_>, func_bits: u64, iterables: &[u64]) -> u64 {
    unsafe {
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
        let total = std::mem::size_of::<MoltHeader>() + MAP_PAYLOAD_SIZE;
        let map_ptr = alloc_object(_py, total, TYPE_ID_MAP);
        if map_ptr.is_null() {
            for iter_bits in iters {
                dec_ref_bits(_py, iter_bits);
            }
            return MoltObject::none().bits();
        }
        let Some(iters_ptr) =
            crate::object::backing::tracked_vec_box_from_slice(iters.as_slice(), iters.len())
        else {
            for iter_bits in iters {
                dec_ref_bits(_py, iter_bits);
            }
            dec_ref_bits(_py, MoltObject::from_ptr(map_ptr).bits());
            return raise_exception::<_>(_py, "MemoryError", "map allocation failed");
        };
        *(map_ptr as *mut u64) = func_bits;
        *(map_ptr.add(std::mem::size_of::<u64>()) as *mut *mut Vec<u64>) = iters_ptr;
        // Initialize cached-tuple slot to null (payload bytes are not
        // zero-initialized when the nursery serves the allocation).
        map_set_cached_tuple(map_ptr, std::ptr::null_mut());
        inc_ref_bits(_py, func_bits);
        MoltObject::from_ptr(map_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_map_builtin(func_bits: u64, iterables_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let iterables_obj = obj_from_bits(iterables_bits);
        let Some(iterables_ptr) = iterables_obj.as_ptr() else {
            let single = [iterables_bits];
            return unsafe { map_new_impl(_py, func_bits, &single) };
        };
        unsafe {
            if object_type_id(iterables_ptr) == TYPE_ID_TUPLE {
                let Some(iterables) = crate::object::seq_access::snapshot(
                    _py,
                    iterables_ptr,
                    "map iterable snapshot allocation failed",
                ) else {
                    return MoltObject::none().bits();
                };
                return map_new_impl(_py, func_bits, &iterables);
            }
            let single = [iterables_bits];
            map_new_impl(_py, func_bits, &single)
        }
    })
}

pub(crate) unsafe fn filter_new_impl(_py: &PyToken<'_>, func_bits: u64, iterable_bits: u64) -> u64 {
    unsafe {
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
        *(filter_ptr as *mut u64) = func_bits;
        *(filter_ptr.add(std::mem::size_of::<u64>()) as *mut u64) = iter_bits;
        inc_ref_bits(_py, func_bits);
        MoltObject::from_ptr(filter_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_filter_builtin(func_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe { filter_new_impl(_py, func_bits, iterable_bits) }
    })
}

pub(crate) unsafe fn zip_new_impl(_py: &PyToken<'_>, iterables: &[u64], strict: bool) -> u64 {
    unsafe {
        let strict_bits = MoltObject::from_bool(strict).bits();
        let mut iters = Vec::with_capacity(iterables.len());
        for &iterable_bits in iterables.iter() {
            let iter_bits = molt_iter(iterable_bits);
            if obj_from_bits(iter_bits).is_none() {
                return raise_not_iterable(_py, iterable_bits);
            }
            iters.push(iter_bits);
        }
        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<u64>();
        let zip_ptr = alloc_object(_py, total, TYPE_ID_ZIP);
        if zip_ptr.is_null() {
            for iter_bits in iters {
                dec_ref_bits(_py, iter_bits);
            }
            return MoltObject::none().bits();
        }
        let Some(iters_ptr) =
            crate::object::backing::tracked_vec_box_from_slice(iters.as_slice(), iters.len())
        else {
            for iter_bits in iters {
                dec_ref_bits(_py, iter_bits);
            }
            dec_ref_bits(_py, MoltObject::from_ptr(zip_ptr).bits());
            return raise_exception::<_>(_py, "MemoryError", "zip allocation failed");
        };
        *(zip_ptr as *mut *mut Vec<u64>) = iters_ptr;
        zip_set_strict_bits(zip_ptr, strict_bits);
        inc_ref_bits(_py, strict_bits);
        MoltObject::from_ptr(zip_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zip_builtin(iterables_bits: u64, strict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let strict = is_truthy(_py, obj_from_bits(strict_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let _strict_bits = MoltObject::from_bool(strict).bits();
        let iterables_obj = obj_from_bits(iterables_bits);
        let Some(iterables_ptr) = iterables_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "zip expects an iterable of iterables");
        };
        unsafe {
            let tid = object_type_id(iterables_ptr);
            if tid != TYPE_ID_TUPLE && tid != TYPE_ID_LIST {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "zip expects an iterable of iterables",
                );
            }
            let Some(iterables) = crate::object::seq_access::snapshot(
                _py,
                iterables_ptr,
                "zip iterable snapshot allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
            zip_new_impl(_py, &iterables, strict)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_reversed_builtin(seq_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { unsafe { reversed_new_impl(_py, seq_bits) } })
}

pub(crate) unsafe fn reversed_new_impl(_py: &PyToken<'_>, seq_bits: u64) -> u64 {
    unsafe {
        let obj = obj_from_bits(seq_bits);
        if let Some(ptr) = obj.as_ptr() {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_RANGE {
                let Some((start, stop, step)) = range_components_bigint(ptr) else {
                    return MoltObject::none().bits();
                };
                if step.is_zero() {
                    return MoltObject::none().bits();
                }
                let len = range_len_bigint(&start, &stop, &step);
                let rev_bits = if len.is_zero() {
                    alloc_range_from_bigints(_py, start.clone(), start.clone(), BigInt::from(1))
                } else {
                    let last = &start + &step * (&len - 1);
                    let rev_start = last;
                    let rev_stop = &start - &step;
                    let rev_step = -step;
                    alloc_range_from_bigints(_py, rev_start, rev_stop, rev_step)
                };
                if obj_from_bits(rev_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let iter_bits = molt_iter(rev_bits);
                dec_ref_bits(_py, rev_bits);
                return iter_bits;
            }
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
                || type_id == TYPE_ID_LIST_INT
                || type_id == TYPE_ID_LIST_BOOL
                || type_id == TYPE_ID_TUPLE
                || type_id == TYPE_ID_STRING
                || type_id == TYPE_ID_BYTES
                || type_id == TYPE_ID_BYTEARRAY
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
                } else if type_id == TYPE_ID_LIST
                    || type_id == TYPE_ID_LIST_INT
                    || type_id == TYPE_ID_LIST_BOOL
                {
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
        let msg = format!("'{}' object is not reversible", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_anext_builtin(iter_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        if default_bits == missing {
            return molt_anext(iter_bits);
        }
        let total = std::mem::size_of::<MoltHeader>() + 3 * std::mem::size_of::<u64>();
        let obj_ptr = alloc_object_zeroed_with_aux(
            _py,
            total,
            TYPE_ID_OBJECT,
            ObjectAuxPreselection::Sidecar,
        );
        if obj_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        unsafe {
            (*header_from_obj_ptr(obj_ptr)).fetch_or_flags(crate::object::HEADER_FLAG_RAW_ALLOC);
            if !object_init_poll_fn_unpublished(obj_ptr, anext_default_poll_fn_addr())
                || !object_init_state_unpublished(obj_ptr, 0)
            {
                dec_ref_bits(_py, obj_bits);
                return MoltObject::none().bits();
            }
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_enumerate(iterable_bits: u64, start_bits: u64, has_start_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let has_start = is_truthy(_py, obj_from_bits(has_start_bits));
        let start_opt = if has_start { Some(start_bits) } else { None };
        unsafe { enumerate_new_impl(_py, iterable_bits, start_opt) }
    })
}

pub(crate) unsafe fn enumerate_new_impl(
    _py: &PyToken<'_>,
    iterable_bits: u64,
    start_opt: Option<u64>,
) -> u64 {
    unsafe {
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        let index_bits = if let Some(start_bits) = start_opt {
            let start_obj = obj_from_bits(start_bits);
            let mut is_int_like = start_obj.is_int() || start_obj.is_bool();
            if !is_int_like && let Some(ptr) = start_obj.as_ptr() {
                is_int_like = object_type_id(ptr) == TYPE_ID_BIGINT;
            }
            if !is_int_like {
                let msg = format!(
                    "'{}' object cannot be interpreted as an integer",
                    type_name(_py, start_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            start_bits
        } else {
            MoltObject::from_int(0).bits()
        };
        let total = std::mem::size_of::<MoltHeader>() + ENUMERATE_PAYLOAD_SIZE;
        let enum_ptr = alloc_object(_py, total, TYPE_ID_ENUMERATE);
        if enum_ptr.is_null() {
            return MoltObject::none().bits();
        }
        *(enum_ptr as *mut u64) = iter_bits;
        *(enum_ptr.add(std::mem::size_of::<u64>()) as *mut u64) = index_bits;
        // Initialize cached-tuple slots to null (alloc_object doesn't zero
        // payload bytes when served from the nursery / pool).
        enumerate_set_cached_inner(enum_ptr, std::ptr::null_mut());
        enumerate_set_cached_outer(enum_ptr, std::ptr::null_mut());
        inc_ref_bits(_py, iter_bits);
        inc_ref_bits(_py, index_bits);
        MoltObject::from_ptr(enum_ptr).bits()
    }
}

#[inline]
fn trace_iter_arg_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_ITER_ARG").ok().as_deref(),
            Some("1")
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_iter(iter_bits: u64) -> u64 {
    iter_impl(iter_bits, false)
}

pub(crate) extern "C" fn builtin_iter_slot(iter_bits: u64) -> u64 {
    iter_impl(iter_bits, true)
}

fn iter_impl(iter_bits: u64, builtin_only: bool) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if trace_iter_arg_enabled() {
            let (frame_name, frame_line) = crate::state::tls::FRAME_STACK.with(|stack| {
                let stack = stack.borrow();
                if let Some(frame) = stack.last()
                    && let Some(code_ptr) = maybe_ptr_from_bits(frame.code_bits)
                {
                    let name_bits = unsafe { code_name_bits(code_ptr) };
                    let name = string_obj_to_owned(obj_from_bits(name_bits))
                        .unwrap_or_else(|| "<code>".to_string());
                    return (name, frame.line);
                }
                ("<no-frame>".to_string(), -1)
            });
            eprintln!(
                "[molt iter arg] frame={} line={} type={} bits=0x{:x}",
                frame_name,
                frame_line,
                type_name(_py, obj_from_bits(iter_bits)),
                iter_bits
            );
        }
        if let Some(ptr) = maybe_ptr_from_bits(iter_bits) {
            unsafe {
                let type_id = object_type_id(ptr);
                if builtin_only || crate::object::iterable::builtin_receiver(_py, ptr) {
                    if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                        let target_bits = molt_dict_keys(dict_bits);
                        if obj_from_bits(target_bits).is_none() {
                            return MoltObject::none().bits();
                        }
                        let total = std::mem::size_of::<MoltHeader>()
                            + std::mem::size_of::<u64>()
                            + std::mem::size_of::<usize>()
                            + std::mem::size_of::<*mut u8>();
                        let iter_ptr = alloc_object(_py, total, TYPE_ID_ITER);
                        if iter_ptr.is_null() {
                            dec_ref_bits(_py, target_bits);
                            return MoltObject::none().bits();
                        }
                        *(iter_ptr as *mut u64) = target_bits;
                        iter_set_index(iter_ptr, 0);
                        iter_set_cached_tuple(iter_ptr, std::ptr::null_mut());
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
                        || type_id == TYPE_ID_GLOB_ITER
                    {
                        inc_ref_bits(_py, iter_bits);
                        return iter_bits;
                    }
                    // GenericAlias (e.g. list[int]): iterate over __args__ tuple,
                    // matching CPython's types.GenericAlias.__iter__ semantics.
                    if type_id == TYPE_ID_GENERIC_ALIAS {
                        let args_bits = generic_alias_args_bits(ptr);
                        if let Some(args_ptr) = obj_from_bits(args_bits).as_ptr()
                            && object_type_id(args_ptr) == TYPE_ID_TUPLE
                        {
                            let total = std::mem::size_of::<MoltHeader>()
                                + std::mem::size_of::<u64>()
                                + std::mem::size_of::<usize>()
                                + std::mem::size_of::<*mut u8>();
                            let iter_ptr = alloc_object(_py, total, TYPE_ID_ITER);
                            if iter_ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            inc_ref_bits(_py, args_bits);
                            *(iter_ptr as *mut u64) = args_bits;
                            iter_set_index(iter_ptr, 0);
                            iter_set_cached_tuple(iter_ptr, std::ptr::null_mut());
                            return MoltObject::from_ptr(iter_ptr).bits();
                        }
                    }
                    if type_id == TYPE_ID_LIST
                        || type_id == TYPE_ID_LIST_INT
                        || type_id == TYPE_ID_LIST_BOOL
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
                            + std::mem::size_of::<usize>()
                            + std::mem::size_of::<*mut u8>();
                        let iter_ptr = alloc_object(_py, total, TYPE_ID_ITER);
                        if iter_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        inc_ref_bits(_py, iter_bits);
                        *(iter_ptr as *mut u64) = iter_bits;
                        iter_set_index(iter_ptr, 0);
                        iter_set_cached_tuple(iter_ptr, std::ptr::null_mut());
                        return MoltObject::from_ptr(iter_ptr).bits();
                    }
                }
                if !builtin_only {
                    if let Some(call_bits) =
                        crate::builtins::attr::lookup_special_method(_py, iter_bits, b"__iter__")
                    {
                        let res = call_callable0(_py, call_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, res);
                            return MoltObject::none().bits();
                        }
                        if !is_iterator_bits(_py, res) {
                            let msg = format!(
                                "iter() returned non-iterator of type '{}'",
                                type_name(_py, obj_from_bits(res))
                            );
                            dec_ref_bits(_py, res);
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        return res;
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                if !builtin_only {
                    if crate::builtins::attr::has_special_method(_py, iter_bits, b"__getitem__") {
                        let total = std::mem::size_of::<MoltHeader>()
                            + std::mem::size_of::<u64>()
                            + std::mem::size_of::<usize>()
                            + std::mem::size_of::<*mut u8>();
                        let iter_ptr = alloc_object(_py, total, TYPE_ID_ITER);
                        if iter_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        inc_ref_bits(_py, iter_bits);
                        *(iter_ptr as *mut u64) = iter_bits;
                        iter_set_index(iter_ptr, 0);
                        iter_set_cached_tuple(iter_ptr, std::ptr::null_mut());
                        return MoltObject::from_ptr(iter_ptr).bits();
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_iter_checked(iter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if obj_from_bits(iter_bits).is_none() {
            return MoltObject::none().bits();
        }
        let res = molt_iter(iter_bits);
        if obj_from_bits(res).is_none() {
            if exception_pending(_py) {
                return res;
            }
            if std::env::var("MOLT_DEBUG_ITER").as_deref() == Ok("1") {
                let iter_obj = obj_from_bits(iter_bits);
                eprintln!(
                    "molt_iter_checked: non-iterable type={} bits=0x{:x}",
                    type_name(_py, iter_obj),
                    iter_bits
                );
            }
            return raise_not_iterable(_py, iter_bits);
        }
        res
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_iter_sentinel(callable_bits: u64, sentinel_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let callable_ok = is_truthy(_py, obj_from_bits(molt_is_callable(callable_bits)));
        if !callable_ok {
            return raise_exception::<_>(_py, "TypeError", "iter(v, w): v must be callable");
        }
        let total = std::mem::size_of::<MoltHeader>() + CALL_ITER_PAYLOAD_SIZE;
        let iter_ptr = alloc_object(_py, total, TYPE_ID_CALL_ITER);
        if iter_ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            *(iter_ptr as *mut u64) = callable_bits;
            *(iter_ptr.add(std::mem::size_of::<u64>()) as *mut u64) = sentinel_bits;
            // Initialize cached-tuple slot to null.
            call_iter_set_cached_tuple(iter_ptr, std::ptr::null_mut());
        }
        inc_ref_bits(_py, callable_bits);
        inc_ref_bits(_py, sentinel_bits);
        MoltObject::from_ptr(iter_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_aiter(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

/// Build or reuse a 2-tuple from a cached slot.
///
/// When the cached tuple exists and its refcount is exactly 1 (exclusively
/// owned by the cache slot), the elements are mutated in place — zero heap
/// allocations.  Otherwise a fresh tuple is allocated and stored in the
/// cache slot for next time.
///
/// `slot_ptr` — pointer to the cache slot (`*mut *mut u8`) that stores the
///              cached tuple's data pointer.  May be null on first call.
/// `elem0`    — the element to place at index 0.
/// `elem1`    — the element to place at index 1.
/// `owns_elem0` — if true the caller holds a NEW reference to `elem0` that
///                should be consumed (the helper will dec-ref it after the
///                tuple inc-refs it).  Same semantics applies to `owns_elem1`.
///
/// Returns an owning reference (refcount bumped by +1 vs. the cache).
///
/// # Safety
/// `slot_ptr` must point to a writable `*mut u8` slot whose lifetime
/// extends until the next `cached_pair_return` call (or object dealloc).
#[inline]
unsafe fn cached_pair_return(
    _py: &PyToken<'_>,
    slot_ptr: *mut *mut u8,
    elem0: u64,
    elem1: u64,
    owns_elem0: bool,
    owns_elem1: bool,
) -> u64 {
    unsafe {
        let cached = *slot_ptr;

        if !cached.is_null() {
            if let Some((old0, old1)) =
                crate::object::seq_access::replace_unique_pair(_py, cached, elem0, elem1)
            {
                // Publish the caller's owner before releasing old elements.
                // Their finalizers can clear or replace this same cache slot;
                // rc=2 also prevents nested next() from mutating our result.
                inc_ref_ptr(_py, cached);
                dec_ref_bits(_py, old0);
                dec_ref_bits(_py, old1);
                if owns_elem0 {
                    dec_ref_bits(_py, elem0);
                }
                if owns_elem1 {
                    dec_ref_bits(_py, elem1);
                }
                return MoltObject::from_ptr(cached).bits();
            }
            // Shared or ABI-observed tuples cannot be mutated. Keep the old
            // owner alive until the replacement owns its inputs and is visible.
        }

        // Allocate a fresh tuple and cache it.
        let tuple_ptr = alloc_tuple(_py, &[elem0, elem1]);
        if tuple_ptr.is_null() {
            if owns_elem0 {
                dec_ref_bits(_py, elem0);
            }
            if owns_elem1 {
                dec_ref_bits(_py, elem1);
            }
            return MoltObject::none().bits();
        }
        // Publish both owners before any callback-bearing release. Never
        // access slot_ptr afterward: a callback may install a newer cache.
        inc_ref_ptr(_py, tuple_ptr);
        let previous = std::ptr::replace(slot_ptr, tuple_ptr);
        if !previous.is_null() {
            dec_ref_ptr(_py, previous);
        }
        if owns_elem0 {
            dec_ref_bits(_py, elem0);
        }
        if owns_elem1 {
            dec_ref_bits(_py, elem1);
        }
        // Return with the original refcount=1 as the caller's owning ref.
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

/// Release the iterator's sole cache owner and every Python edge reachable
/// through it. Terminal and exceptional control flow must pass through this
/// authority before propagating outward; normal value production instead goes
/// through `cached_pair_return`, which replaces the prior pair transactionally.
unsafe fn cached_pair_clear(_py: &PyToken<'_>, slot_ptr: *mut *mut u8) {
    unsafe {
        let cached = std::ptr::replace(slot_ptr, std::ptr::null_mut());
        if !cached.is_null() {
            dec_ref_ptr(_py, cached);
        }
    }
}

/// Build or reuse a (value, done) 2-tuple from the iterator's cached slot.
///
/// `iter_ptr` must point to live TYPE_ID_ITER data, past the header.
unsafe fn iter_pair_slot(iter_ptr: *mut u8) -> *mut *mut u8 {
    unsafe {
        iter_ptr.add(std::mem::size_of::<u64>() + std::mem::size_of::<usize>()) as *mut *mut u8
    }
}

/// Retire the target before releasing any edge that can reenter the iterator.
unsafe fn iter_finish(py: &PyToken<'_>, iter_ptr: *mut u8) {
    unsafe {
        let target = iter_target_bits(iter_ptr);
        crate::object::layout::iter_set_target_bits(iter_ptr, MoltObject::none().bits());
        iter_set_index(iter_ptr, ITER_EXHAUSTED);
        cached_pair_clear(py, iter_pair_slot(iter_ptr));
        dec_ref_bits(py, target);
    }
}

/// Return an owned pair; `owns_val` transfers the caller's value reference.
/// A completed TYPE_ID_ITER releases its target through the shared transition.
unsafe fn iter_return_cached(
    _py: &PyToken<'_>,
    iter_ptr: *mut u8,
    val_bits: u64,
    done: bool,
    owns_val: bool,
) -> u64 {
    unsafe {
        let done_bits = MoltObject::from_bool(done).bits();
        if done {
            iter_finish(_py, iter_ptr);
            let result = generator_done_tuple(_py, val_bits);
            if owns_val {
                dec_ref_bits(_py, val_bits);
            }
            return result;
        }
        cached_pair_return(
            _py,
            iter_pair_slot(iter_ptr),
            val_bits,
            done_bits,
            owns_val,
            false,
        )
    }
}

unsafe fn weak_container_iter_advance(
    _py: &PyToken<'_>,
    iter_ptr: *mut u8,
    state_ptr: *mut u8,
) -> Result<Option<u64>, ()> {
    let mut version = unsafe { crate::object::layout::iter_expected_version(iter_ptr) };
    if version == crate::object::weak_container::WEAK_ITER_VERSION_FINISHED {
        return Ok(None);
    }
    if version == crate::object::weak_container::WEAK_ITER_VERSION_UNSTARTED {
        let started = match crate::object::weak_container::weakcontainer_iter_begin(_py, state_ptr)
        {
            Ok(Some(started)) => started,
            Ok(None) => {
                unsafe {
                    crate::object::layout::iter_set_expected_version(
                        iter_ptr,
                        crate::object::weak_container::WEAK_ITER_VERSION_FINISHED,
                    )
                };
                return Ok(None);
            }
            Err(_) => return Err(()),
        };
        version = started;
        unsafe { crate::object::layout::iter_set_expected_version(iter_ptr, version) };
    }
    let cursor = unsafe { iter_index(iter_ptr) };
    let projection = unsafe { crate::object::layout::iter_projection(iter_ptr) } as u8;
    match crate::object::weak_container::weakcontainer_iter_next_value(
        _py, state_ptr, cursor, version, projection,
    ) {
        Ok((next, Some(value))) => {
            unsafe { iter_set_index(iter_ptr, next) };
            Ok(Some(value))
        }
        terminal => {
            // Pending-entry cleanup may invoke finalizers that advance this
            // iterator again. Publish termination before those callbacks and
            // pin the state: a reentrant next can release the iterator's target
            // through iter_finish while the outer cleanup still drains it.
            inc_ref_bits(_py, MoltObject::from_ptr(state_ptr).bits());
            let state_guard = PtrDropGuard::new(state_ptr);
            unsafe {
                crate::object::layout::iter_set_expected_version(
                    iter_ptr,
                    crate::object::weak_container::WEAK_ITER_VERSION_FINISHED,
                )
            };
            crate::object::weak_container::weakcontainer_iter_finish(_py, state_ptr);
            drop(state_guard);
            if terminal.is_err() || exception_pending(_py) {
                Err(())
            } else {
                Ok(None)
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_iter_next(iter_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(ptr) = maybe_ptr_from_bits(iter_bits) {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_GENERATOR {
                    let res_bits = molt_generator_send(iter_bits, MoltObject::none().bits());
                    if exception_pending(_py) {
                        return res_bits;
                    }
                    let res_obj = obj_from_bits(res_bits);
                    if let Some(res_ptr) = res_obj.as_ptr()
                        && object_type_id(res_ptr) == TYPE_ID_TUPLE
                        && let Some((_, done_bits)) = crate::object::seq_access::tuple_pair(res_ptr)
                    {
                        let done = is_truthy(_py, obj_from_bits(done_bits));
                        if done {
                            let closed_bits = MoltObject::from_bool(true).bits();
                            *(ptr.add(GEN_CLOSED_OFFSET) as *mut u64) = closed_bits;
                        }
                    }
                    return res_bits;
                }
                if object_type_id(ptr) == TYPE_ID_GLOB_ITER {
                    // Lazy glob iterator: advance the native streaming state by
                    // one path and return the `(value, done)` tuple.
                    let value_bits = crate::builtins::io_path_utils::glob_iter_next_value(_py, ptr);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if obj_from_bits(value_bits).is_none() {
                        // Exhausted: `glob_iter_next_value` returns None-bits only
                        // at end-of-stream (paths are always str/bytes, never None).
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let done_bits = MoltObject::from_bool(false).bits();
                    let tuple_ptr = alloc_tuple(_py, &[value_bits, done_bits]);
                    dec_ref_bits(_py, value_bits);
                    if tuple_ptr.is_null() {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_ptr(tuple_ptr).bits();
                }
                if object_type_id(ptr) == TYPE_ID_ENUMERATE {
                    let inner_slot = ptr.add(2 * std::mem::size_of::<u64>()) as *mut *mut u8;
                    let outer_slot = ptr
                        .add(2 * std::mem::size_of::<u64>() + std::mem::size_of::<*mut u8>())
                        as *mut *mut u8;
                    let iter_bits = enumerate_target_bits(ptr);
                    let pair_bits = molt_iter_next(iter_bits);
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        cached_pair_clear(_py, inner_slot);
                        cached_pair_clear(_py, outer_slot);
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        cached_pair_clear(_py, inner_slot);
                        cached_pair_clear(_py, outer_slot);
                        dec_ref_bits(_py, pair_bits);
                        return MoltObject::none().bits();
                    }
                    let Some((val_bits, done_bits)) =
                        crate::object::seq_access::tuple_pair(pair_ptr)
                    else {
                        cached_pair_clear(_py, inner_slot);
                        cached_pair_clear(_py, outer_slot);
                        dec_ref_bits(_py, pair_bits);
                        return MoltObject::none().bits();
                    };
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        cached_pair_clear(_py, inner_slot);
                        cached_pair_clear(_py, outer_slot);
                        return pair_bits;
                    }
                    let idx_bits = enumerate_index_bits(ptr);
                    // Build (or reuse) the inner (idx, val) user-visible tuple.
                    let item_bits =
                        cached_pair_return(_py, inner_slot, idx_bits, val_bits, false, false);
                    dec_ref_bits(_py, pair_bits);
                    if obj_from_bits(item_bits).is_none() {
                        cached_pair_clear(_py, inner_slot);
                        cached_pair_clear(_py, outer_slot);
                        return MoltObject::none().bits();
                    }
                    let done_false = MoltObject::from_bool(false).bits();
                    // Build (or reuse) the outer (item, done_false) wrapper.
                    let out_bits =
                        cached_pair_return(_py, outer_slot, item_bits, done_false, true, false);
                    if obj_from_bits(out_bits).is_none() {
                        cached_pair_clear(_py, inner_slot);
                        cached_pair_clear(_py, outer_slot);
                        return MoltObject::none().bits();
                    }
                    // Integer increment — enumerate counter is always int.
                    // molt_add is polymorphic and promotes to float if idx is float.
                    let next_bits = if let Some(i) = to_i64(obj_from_bits(idx_bits)) {
                        int_bits_from_i64(_py, i + 1)
                    } else {
                        molt_add(idx_bits, MoltObject::from_int(1).bits())
                    };
                    if obj_from_bits(next_bits).is_none() {
                        dec_ref_bits(_py, out_bits);
                        cached_pair_clear(_py, inner_slot);
                        cached_pair_clear(_py, outer_slot);
                        return MoltObject::none().bits();
                    }
                    dec_ref_bits(_py, idx_bits);
                    enumerate_set_index_bits(ptr, next_bits);
                    return out_bits;
                }
                if object_type_id(ptr) == TYPE_ID_CALL_ITER {
                    let slot_ptr = ptr.add(2 * std::mem::size_of::<u64>()) as *mut *mut u8;
                    let call_bits = call_iter_callable_bits(ptr);
                    let sentinel_bits = call_iter_sentinel_bits(ptr);
                    let val_bits = call_callable0(_py, call_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, val_bits);
                        cached_pair_clear(_py, slot_ptr);
                        return MoltObject::none().bits();
                    }
                    if obj_eq(_py, obj_from_bits(val_bits), obj_from_bits(sentinel_bits)) {
                        dec_ref_bits(_py, val_bits);
                        cached_pair_clear(_py, slot_ptr);
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let done_bits = MoltObject::from_bool(false).bits();
                    let out_bits =
                        cached_pair_return(_py, slot_ptr, val_bits, done_bits, true, false);
                    return out_bits;
                }
                if object_type_id(ptr) == TYPE_ID_MAP {
                    let func_bits = map_func_bits(ptr);
                    let iters_ptr = map_iters_ptr(ptr);
                    let slot_ptr = ptr
                        .add(std::mem::size_of::<u64>() + std::mem::size_of::<*mut Vec<u64>>())
                        as *mut *mut u8;
                    if iters_ptr.is_null() {
                        cached_pair_clear(_py, slot_ptr);
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let iters = &mut *iters_ptr;
                    if iters.is_empty() {
                        cached_pair_clear(_py, slot_ptr);
                        return generator_done_tuple(_py, MoltObject::none().bits());
                    }
                    let mut inputs = Vec::with_capacity(iters.len());
                    for &iter_bits in iters.iter() {
                        let pair_bits = molt_iter_next(iter_bits);
                        let pair_obj = obj_from_bits(pair_bits);
                        let Some(pair_ptr) = pair_obj.as_ptr() else {
                            for &(_, owner) in &inputs {
                                dec_ref_bits(_py, owner);
                            }
                            cached_pair_clear(_py, slot_ptr);
                            return MoltObject::none().bits();
                        };
                        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                            dec_ref_bits(_py, pair_bits);
                            for &(_, owner) in &inputs {
                                dec_ref_bits(_py, owner);
                            }
                            cached_pair_clear(_py, slot_ptr);
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        }
                        let Some((val_bits, done_bits)) =
                            crate::object::seq_access::tuple_pair(pair_ptr)
                        else {
                            dec_ref_bits(_py, pair_bits);
                            for &(_, owner) in &inputs {
                                dec_ref_bits(_py, owner);
                            }
                            cached_pair_clear(_py, slot_ptr);
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        };
                        if is_truthy(_py, obj_from_bits(done_bits)) {
                            dec_ref_bits(_py, pair_bits);
                            for &(_, owner) in &inputs {
                                dec_ref_bits(_py, owner);
                            }
                            cached_pair_clear(_py, slot_ptr);
                            return generator_done_tuple(_py, MoltObject::none().bits());
                        }
                        inputs.push((val_bits, pair_bits));
                    }
                    // Route map callable invocation through bind so Python
                    // function defaults are honored (e.g. def f(x, y=...)).
                    let builder_bits = molt_callargs_new(inputs.len() as u64, 0);
                    if builder_bits == 0 {
                        for &(_, owner) in &inputs {
                            dec_ref_bits(_py, owner);
                        }
                        cached_pair_clear(_py, slot_ptr);
                        return MoltObject::none().bits();
                    }
                    for &(val_bits, _) in &inputs {
                        let _ = molt_callargs_push_pos(builder_bits, val_bits);
                    }
                    for &(_, owner) in &inputs {
                        dec_ref_bits(_py, owner);
                    }
                    let res_bits = molt_call_bind(func_bits, builder_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, res_bits);
                        cached_pair_clear(_py, slot_ptr);
                        return MoltObject::none().bits();
                    }
                    let done_bits = MoltObject::from_bool(false).bits();
                    let out_bits =
                        cached_pair_return(_py, slot_ptr, res_bits, done_bits, true, false);
                    return out_bits;
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
                        let Some((val_bits, done_bits)) =
                            crate::object::seq_access::tuple_pair(pair_ptr)
                        else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        };
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
                    let strict = is_truthy(_py, obj_from_bits(zip_strict_bits(ptr)));
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let mut vals = Vec::with_capacity(iters.len());
                    if strict {
                        let mut done_flags = Vec::with_capacity(iters.len());
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
                            let Some((val_bits, done_bits)) =
                                crate::object::seq_access::tuple_pair(pair_ptr)
                            else {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    "object is not an iterator",
                                );
                            };
                            let done = is_truthy(_py, obj_from_bits(done_bits));
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            vals.push(val_bits);
                            done_flags.push(done);
                        }
                        if done_flags.iter().all(|done| *done) {
                            return generator_done_tuple(_py, MoltObject::none().bits());
                        }
                        // CPython describes the contiguous run of arguments
                        // preceding the offending one: singular "argument 1" when
                        // only the first precedes, else the plural "arguments 1-N"
                        // where N is the highest preceding 1-based argument number.
                        let preceding_phrase = |last: usize| -> String {
                            if last <= 1 {
                                "argument 1".to_string()
                            } else {
                                format!("arguments 1-{last}")
                            }
                        };
                        if done_flags.first().copied().unwrap_or(false) {
                            if let Some(idx) = done_flags[1..].iter().position(|done| !*done) {
                                let msg = format!(
                                    "zip() argument {} is longer than {}",
                                    idx + 2,
                                    preceding_phrase(idx + 1)
                                );
                                return raise_exception::<_>(_py, "ValueError", &msg);
                            }
                            return generator_done_tuple(_py, MoltObject::none().bits());
                        }
                        if let Some(idx) = done_flags[1..].iter().position(|done| *done) {
                            let msg = format!(
                                "zip() argument {} is shorter than {}",
                                idx + 2,
                                preceding_phrase(idx + 1)
                            );
                            return raise_exception::<_>(_py, "ValueError", &msg);
                        }
                    } else {
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
                            let Some((val_bits, done_bits)) =
                                crate::object::seq_access::tuple_pair(pair_ptr)
                            else {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    "object is not an iterator",
                                );
                            };
                            if is_truthy(_py, obj_from_bits(done_bits)) {
                                return generator_done_tuple(_py, MoltObject::none().bits());
                            }
                            vals.push(val_bits);
                        }
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
                            let len = crate::object::seq_access::len(target_ptr);
                            let idx = idx.min(len);
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                let mut value = 0;
                                if crate::object::seq_access::read_item_owned(
                                    target_ptr,
                                    idx - 1,
                                    &mut value,
                                ) == 0
                                {
                                    (0, None, false)
                                } else {
                                    (idx - 1, Some(value), true)
                                }
                            }
                        } else if target_type == TYPE_ID_LIST_INT {
                            let elems = crate::object::layout::list_int_vec_ref(target_ptr);
                            let len = elems.len();
                            let idx = idx.min(len);
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                (
                                    idx - 1,
                                    Some(MoltObject::from_int(elems[idx - 1]).bits()),
                                    false,
                                )
                            }
                        } else if target_type == TYPE_ID_LIST_BOOL {
                            let elems = crate::object::layout::list_bool_vec_ref(target_ptr);
                            let len = elems.len();
                            let idx = idx.min(len);
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                (
                                    idx - 1,
                                    Some(MoltObject::from_bool(elems[idx - 1] != 0).bits()),
                                    false,
                                )
                            }
                        } else if target_type == TYPE_ID_RANGE {
                            let Some((start, stop, step)) = range_components_bigint(target_ptr)
                            else {
                                return MoltObject::none().bits();
                            };
                            let len = range_len_bigint(&start, &stop, &step);
                            let len_usize = len.to_usize().unwrap_or(idx);
                            let idx = idx.min(len_usize);
                            if idx == 0 {
                                (0, None, false)
                            } else {
                                let pos = BigInt::from((idx - 1) as u64);
                                let val = start + step * pos;
                                let bits = int_bits_from_bigint(_py, val);
                                if obj_from_bits(bits).is_none() {
                                    return MoltObject::none().bits();
                                }
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
                    use crate::object::iterable::{SpecialIterationKind, SpecialIterationStep};
                    return match crate::object::iterable::special_iteration_step(
                        _py,
                        iter_bits,
                        SpecialIterationKind::Next,
                    ) {
                        Ok(SpecialIterationStep::Item(value)) => {
                            let tuple =
                                alloc_tuple(_py, &[value, MoltObject::from_bool(false).bits()]);
                            dec_ref_bits(_py, value);
                            if tuple.is_null() {
                                MoltObject::none().bits()
                            } else {
                                MoltObject::from_ptr(tuple).bits()
                            }
                        }
                        Ok(SpecialIterationStep::Exhausted(value)) => {
                            let result = generator_done_tuple(_py, value);
                            dec_ref_bits(_py, value);
                            result
                        }
                        Ok(SpecialIterationStep::Missing) => {
                            raise_exception::<_>(_py, "TypeError", "object is not an iterator")
                        }
                        Err(()) => MoltObject::none().bits(),
                    };
                }
                let target_bits = iter_target_bits(ptr);
                let target_obj = obj_from_bits(target_bits);
                let idx = iter_index(ptr);
                // Validate the target pointer before reading.
                // If the iterator or its target was freed, target_bits
                // will be garbage — return done to prevent crash.
                if !target_obj.is_ptr() {
                    return generator_done_tuple(_py, MoltObject::none().bits());
                }
                if let Some(target_ptr) = target_obj.as_ptr() {
                    let target_type = object_type_id(target_ptr);
                    if target_type == TYPE_ID_WEAK_CONTAINER_STATE {
                        return match weak_container_iter_advance(_py, ptr, target_ptr) {
                            Ok(Some(value)) => iter_return_cached(_py, ptr, value, false, true),
                            Ok(None) => {
                                iter_return_cached(_py, ptr, MoltObject::none().bits(), true, false)
                            }
                            Err(()) => MoltObject::none().bits(),
                        };
                    }
                    if target_type == TYPE_ID_SET || target_type == TYPE_ID_FROZENSET {
                        let table = set_table(target_ptr);
                        let order = set_order(target_ptr);
                        let mut slot = idx;
                        while slot < table.len() && (table[slot] == 0 || table[slot] == usize::MAX)
                        {
                            slot += 1;
                        }
                        if slot >= table.len() {
                            iter_set_index(ptr, table.len());
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let entry_idx = table[slot] - 1;
                        let val_bits = order[entry_idx];
                        iter_set_index(ptr, slot + 1);
                        return iter_return_cached(_py, ptr, val_bits, false, false);
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
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let tail = &bytes[idx..];
                        let Ok(text) = std::str::from_utf8(tail) else {
                            return MoltObject::none().bits();
                        };
                        let Some(ch) = text.chars().next() else {
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
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
                        return iter_return_cached(_py, ptr, val_bits, false, true);
                    }
                    if target_type == TYPE_ID_BYTES || target_type == TYPE_ID_BYTEARRAY {
                        let bytes = std::slice::from_raw_parts(
                            bytes_data(target_ptr),
                            bytes_len(target_ptr),
                        );
                        if idx >= bytes.len() {
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let val_bits = MoltObject::from_int(bytes[idx] as i64).bits();
                        iter_set_index(ptr, idx + 1);
                        return iter_return_cached(_py, ptr, val_bits, false, false);
                    }
                    if target_type == TYPE_ID_LIST {
                        let len = crate::object::seq_access::len(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= len {
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let Some(val) = crate::object::seq_access::pin_item(_py, target_ptr, idx)
                        else {
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        };
                        let val_bits = val.bits();
                        inc_ref_bits(_py, val_bits);
                        drop(val);
                        iter_set_index(ptr, idx + 1);
                        return iter_return_cached(_py, ptr, val_bits, false, true);
                    }
                    if target_type == TYPE_ID_LIST_INT {
                        let elems = crate::object::layout::list_int_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let val_bits = MoltObject::from_int(elems[idx]).bits();
                        iter_set_index(ptr, idx + 1);
                        return iter_return_cached(_py, ptr, val_bits, false, false);
                    }
                    if target_type == TYPE_ID_LIST_BOOL {
                        let elems = crate::object::layout::list_bool_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let val_bits = MoltObject::from_bool(elems[idx] != 0).bits();
                        iter_set_index(ptr, idx + 1);
                        return iter_return_cached(_py, ptr, val_bits, false, false);
                    }
                    if target_type == TYPE_ID_RANGE {
                        if let Some((start_i64, stop_i64, step_i64)) =
                            range_components_i64(target_ptr)
                        {
                            if idx == ITER_EXHAUSTED {
                                return iter_return_cached(
                                    _py,
                                    ptr,
                                    MoltObject::none().bits(),
                                    true,
                                    false,
                                );
                            }
                            if let Some(value) =
                                range_value_at_index_i64(start_i64, stop_i64, step_i64, idx as i128)
                            {
                                let val_bits = MoltObject::from_int(value).bits();
                                let next_idx = idx.checked_add(1).unwrap_or(ITER_EXHAUSTED);
                                iter_set_index(ptr, next_idx);
                                return iter_return_cached(_py, ptr, val_bits, false, false);
                            }
                            let len = range_len_i128(start_i64, stop_i64, step_i64);
                            let len_usize = usize::try_from(len).unwrap_or(ITER_EXHAUSTED);
                            iter_set_index(ptr, len_usize);
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let Some((start, stop, step)) = range_components_bigint(target_ptr) else {
                            return MoltObject::none().bits();
                        };
                        if idx == ITER_EXHAUSTED {
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        if step.is_zero() {
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let len = range_len_bigint(&start, &stop, &step);
                        let idx_big = BigInt::from(idx as u64);
                        if idx_big >= len {
                            let len_usize = len.to_usize().unwrap_or(ITER_EXHAUSTED);
                            iter_set_index(ptr, len_usize);
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let val = start + step * idx_big;
                        let val_bits = int_bits_from_bigint(_py, val);
                        if obj_from_bits(val_bits).is_none() {
                            return MoltObject::none().bits();
                        }
                        let next_idx = idx.checked_add(1).unwrap_or(ITER_EXHAUSTED);
                        iter_set_index(ptr, next_idx);
                        return iter_return_cached(_py, ptr, val_bits, false, true);
                    }
                    if target_type != TYPE_ID_TUPLE
                        && target_type != TYPE_ID_RANGE
                        && target_type != TYPE_ID_DICT_KEYS_VIEW
                        && target_type != TYPE_ID_DICT_VALUES_VIEW
                        && target_type != TYPE_ID_DICT_ITEMS_VIEW
                    {
                        use crate::object::iterable::{SpecialIterationKind, SpecialIterationStep};
                        let index_bits = MoltObject::from_int(idx as i64).bits();
                        return match crate::object::iterable::special_iteration_step(
                            _py,
                            target_bits,
                            SpecialIterationKind::SequenceItem(index_bits),
                        ) {
                            Ok(SpecialIterationStep::Item(value)) => {
                                iter_set_index(ptr, idx + 1);
                                iter_return_cached(_py, ptr, value, false, true)
                            }
                            Ok(SpecialIterationStep::Exhausted(value)) => {
                                iter_return_cached(_py, ptr, value, true, true)
                            }
                            Ok(SpecialIterationStep::Missing) => {
                                let message = format!(
                                    "'{}' object is not subscriptable",
                                    type_name(_py, target_obj)
                                );
                                raise_exception::<_>(_py, "TypeError", &message)
                            }
                            Err(()) => MoltObject::none().bits(),
                        };
                    }
                }
                let (len, next_val, needs_drop) = if let Some(target_ptr) = target_obj.as_ptr() {
                    let target_type = object_type_id(target_ptr);
                    if target_type == TYPE_ID_TUPLE {
                        let len = crate::object::seq_access::len(target_ptr);
                        if idx >= len {
                            (len, None, false)
                        } else {
                            (len, crate::object::seq_access::item(target_ptr, idx), false)
                        }
                    } else if target_type == TYPE_ID_RANGE {
                        (0, None, false)
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
                    return iter_return_cached(_py, ptr, val_bits, false, needs_drop);
                }
                if idx >= len {
                    iter_set_index(ptr, len);
                }
                return iter_return_cached(_py, ptr, MoltObject::none().bits(), true, false);
            }
        }
        MoltObject::none().bits()
    })
}

/// Advance an iterator without allocating a `(value, done)` tuple.
///
/// Writes `None` to `*value_out` before advancing the iterator, overwrites it
/// with the next owned value when one is available, and returns `false`-bits for
/// that value or `true`-bits when the iterator is exhausted. Returns `None`
/// bits when an exception is pending.
///
/// Fast-paths list, tuple, and i64-range iterators with zero allocation.
/// Everything else falls back to `molt_iter_next` + destructure.
///
/// # Safety
///
/// `value_out_bits` must encode writable storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_iter_next_unboxed(iter_bits: u64, value_out_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let done_true = MoltObject::from_bool(true).bits();
        let done_false = MoltObject::from_bool(false).bits();
        let no_value = MoltObject::none().bits();
        let Some(value_out) = crate::provenance::abi::mut_ptr::<u64>(value_out_bits) else {
            return raise_exception::<u64>(
                _py,
                "RuntimeError",
                "iterator output address exceeds the active address space",
            );
        };
        if value_out.is_null() {
            return raise_exception::<u64>(_py, "RuntimeError", "iterator output pointer is null");
        }

        unsafe {
            *value_out = no_value;
        }

        let Some(ptr) = maybe_ptr_from_bits(iter_bits) else {
            return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
        };

        unsafe {
            // Fast paths for TYPE_ID_ITER wrapping list/tuple/range.
            // Generators, enumerate, map, filter, zip, reversed, etc.
            // go through the slow path below.
            if object_type_id(ptr) == TYPE_ID_ITER {
                let target_bits = iter_target_bits(ptr);
                let target_obj = obj_from_bits(target_bits);
                let idx = iter_index(ptr);

                if let Some(target_ptr) = target_obj.as_ptr() {
                    let target_type = object_type_id(target_ptr);

                    // ── LIST fast path (zero alloc) ──────────────
                    // Read the current backing only for this scalar step. A list
                    // mutation between iterator calls may resize it, so no backing
                    // reference survives the call boundary.
                    if target_type == TYPE_ID_LIST {
                        let len = crate::object::seq_access::len(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= len {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        let mut val_bits = 0;
                        if crate::object::seq_access::read_item_owned(
                            target_ptr,
                            idx,
                            &mut val_bits,
                        ) == 0
                        {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        *value_out = val_bits;
                        iter_set_index(ptr, idx + 1);
                        return done_false;
                    }

                    // ── LIST_INT fast path (zero alloc) ─────────
                    // Raw i64 storage — box on read, no refcount needed.
                    if target_type == TYPE_ID_LIST_INT {
                        let elems = crate::object::layout::list_int_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        let val_bits = MoltObject::from_int(elems[idx]).bits();
                        *value_out = val_bits;
                        iter_set_index(ptr, idx + 1);
                        return done_false;
                    }

                    // ── LIST_BOOL fast path (zero alloc) ────────
                    // Raw u8 storage — box on read, no refcount needed.
                    if target_type == TYPE_ID_LIST_BOOL {
                        let elems = crate::object::layout::list_bool_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        let val_bits = MoltObject::from_bool(elems[idx] != 0).bits();
                        *value_out = val_bits;
                        iter_set_index(ptr, idx + 1);
                        return done_false;
                    }

                    // ── TUPLE fast path (zero alloc) ─────────────
                    if target_type == TYPE_ID_TUPLE {
                        let len = crate::object::seq_access::len(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= len {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        let Some(val_bits) = crate::object::seq_access::item(target_ptr, idx)
                        else {
                            iter_finish(_py, ptr);
                            return done_true;
                        };
                        inc_ref_bits(_py, val_bits);
                        *value_out = val_bits;
                        iter_set_index(ptr, idx + 1);
                        return done_false;
                    }

                    // ── RANGE i64 fast path (zero alloc) ─────────
                    if target_type == TYPE_ID_RANGE
                        && let Some((start_i64, stop_i64, step_i64)) =
                            range_components_i64(target_ptr)
                    {
                        if idx == ITER_EXHAUSTED {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        if let Some(value) =
                            range_value_at_index_i64(start_i64, stop_i64, step_i64, idx as i128)
                        {
                            let val_bits = MoltObject::from_int(value).bits();
                            *value_out = val_bits;
                            let next_idx = idx.checked_add(1).unwrap_or(ITER_EXHAUSTED);
                            iter_set_index(ptr, next_idx);
                            return done_false;
                        }
                        iter_finish(_py, ptr);
                        return done_true;
                    }
                    // BigInt range — fall through to slow path.

                    // ── DICT_KEYS_VIEW fast path (zero alloc) ──────
                    if target_type == TYPE_ID_DICT_KEYS_VIEW {
                        let len = dict_view_len(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= len {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        if let Some((key_bits, _val_bits)) = dict_view_entry(target_ptr, idx) {
                            inc_ref_bits(_py, key_bits);
                            *value_out = key_bits;
                            iter_set_index(ptr, idx + 1);
                            return done_false;
                        }
                        iter_finish(_py, ptr);
                        return done_true;
                    }

                    // ── DICT_VALUES_VIEW fast path (zero alloc) ────
                    if target_type == TYPE_ID_DICT_VALUES_VIEW {
                        let len = dict_view_len(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= len {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        if let Some((_key_bits, val_bits)) = dict_view_entry(target_ptr, idx) {
                            inc_ref_bits(_py, val_bits);
                            *value_out = val_bits;
                            iter_set_index(ptr, idx + 1);
                            return done_false;
                        }
                        iter_finish(_py, ptr);
                        return done_true;
                    }

                    // ── DICT_ITEMS_VIEW fast path (1 alloc: (k,v) tuple) ──
                    // Avoids the wrapper (value, done) tuple allocation and
                    // the is_truthy dispatch on the done flag.
                    if target_type == TYPE_ID_DICT_ITEMS_VIEW {
                        let len = dict_view_len(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= len {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        if let Some((key_bits, val_bits)) = dict_view_entry(target_ptr, idx) {
                            let tuple_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
                            if tuple_ptr.is_null() {
                                return no_value;
                            }
                            *value_out = MoltObject::from_ptr(tuple_ptr).bits();
                            iter_set_index(ptr, idx + 1);
                            return done_false;
                        }
                        iter_finish(_py, ptr);
                        return done_true;
                    }
                }
            }

            // ── Slow path: delegate to molt_iter_next ─────────────
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                if !obj_from_bits(pair_bits).is_none() {
                    dec_ref_bits(_py, pair_bits);
                }
                return no_value;
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "SystemError",
                    "iterator returned an invalid pair",
                );
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, pair_bits);
                return raise_exception::<_>(
                    _py,
                    "SystemError",
                    "iterator returned a non-tuple pair",
                );
            }
            let Some((val_bits, exhausted_bits)) = crate::object::seq_access::tuple_pair(pair_ptr)
            else {
                dec_ref_bits(_py, pair_bits);
                return raise_exception::<_>(
                    _py,
                    "SystemError",
                    "iterator returned a malformed pair",
                );
            };
            let exhausted = is_truthy(_py, obj_from_bits(exhausted_bits));
            if exception_pending(_py) {
                dec_ref_bits(_py, pair_bits);
                return no_value;
            }
            if exhausted {
                dec_ref_bits(_py, pair_bits);
                return done_true;
            }
            // Transfer ownership: inc_ref value, drop wrapper tuple.
            inc_ref_bits(_py, val_bits);
            *value_out = val_bits;
            dec_ref_bits(_py, pair_bits);
            done_false
        }
    })
}

/// Zero-allocation dict items iterator.
///
/// For `for k, v in dict.items()` loops, this writes the key and value
/// directly to caller-provided stack slots, completely avoiding the
/// intermediate `(k, v)` tuple allocation that `molt_iter_next_unboxed`
/// requires for dict items views.
///
/// Returns `false`-bits when a pair is available (key/value written),
/// `true`-bits when the iterator is exhausted.
/// Initializes both output slots to `None`, so callers may safely load them
/// after any return value without observing stale stack contents.
///
/// # Safety
///
/// `key_out` and `value_out` must point to writable storage for one `u64` each.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_iter_next_dict_items(
    iter_bits: u64,
    key_out: *mut u64,
    value_out: *mut u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let done_true = MoltObject::from_bool(true).bits();
        let done_false = MoltObject::from_bool(false).bits();
        let no_value = MoltObject::none().bits();

        unsafe {
            *key_out = no_value;
            *value_out = no_value;
        }

        let Some(ptr) = maybe_ptr_from_bits(iter_bits) else {
            return no_value;
        };

        unsafe {
            if object_type_id(ptr) == TYPE_ID_ITER {
                let target_bits = iter_target_bits(ptr);
                let target_obj = obj_from_bits(target_bits);
                let idx = iter_index(ptr);

                if let Some(target_ptr) = target_obj.as_ptr() {
                    let target_type = object_type_id(target_ptr);

                    if target_type == TYPE_ID_DICT_ITEMS_VIEW {
                        let len = dict_view_len(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= len {
                            iter_finish(_py, ptr);
                            return done_true;
                        }
                        if let Some((kb, vb)) = dict_view_entry(target_ptr, idx) {
                            // Write key and value directly — zero allocation.
                            if crate::object::refcount_opt::is_heap_ref(kb) {
                                inc_ref_bits(_py, kb);
                            }
                            if crate::object::refcount_opt::is_heap_ref(vb) {
                                inc_ref_bits(_py, vb);
                            }
                            *key_out = kb;
                            *value_out = vb;
                            iter_set_index(ptr, idx + 1);
                            return done_false;
                        }
                        iter_finish(_py, ptr);
                        return done_true;
                    }
                }
            }

            // Fallback: use molt_iter_next_unboxed and unpack the tuple.
            let mut pair_bits: u64 = 0;
            let done = molt_iter_next_unboxed(iter_bits, (&mut pair_bits as *mut u64) as u64);
            if done == done_true || done == MoltObject::none().bits() {
                return done;
            }
            // pair_bits should be a 2-tuple: (key, value).
            let pair_obj = obj_from_bits(pair_bits);
            if let Some(pair_ptr) = pair_obj.as_ptr()
                && object_type_id(pair_ptr) == TYPE_ID_TUPLE
                && let Some((kb, vb)) = crate::object::seq_access::tuple_pair(pair_ptr)
            {
                if crate::object::refcount_opt::is_heap_ref(kb) {
                    inc_ref_bits(_py, kb);
                }
                if crate::object::refcount_opt::is_heap_ref(vb) {
                    inc_ref_bits(_py, vb);
                }
                *key_out = kb;
                *value_out = vb;
                dec_ref_bits(_py, pair_bits);
                return done_false;
            }
            dec_ref_bits(_py, pair_bits);
            done_true
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_anext(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[cfg(test)]
mod tests {
    use super::{cached_pair_return, molt_iter, molt_iter_next_unboxed};
    use crate::object::HEADER_FLAG_CONTAINS_REFS;
    use crate::{
        MoltObject, alloc_dict_with_pairs, alloc_string, dec_ref_bits, header_from_obj_ptr,
        molt_dict_items, molt_unpack_sequence,
    };

    unsafe fn refcount(bits: u64) -> u32 {
        let ptr = MoltObject::from_bits(bits).as_ptr().expect("heap object");
        unsafe { (*header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    unsafe fn iterator_test_instance(py: &crate::PyToken<'_>, class: u64) -> u64 {
        let class_ptr = MoltObject::from_bits(class)
            .as_ptr()
            .expect("fixture class");
        // Iterator ownership tests need a published instance of the actual
        // class layout, not a raw zero-byte allocation with a class annotation.
        let bits = unsafe { crate::alloc_instance_for_class(py, class_ptr) };
        assert!(
            !crate::exception_pending(py),
            "fixture instance construction failed"
        );
        let ptr = MoltObject::from_bits(bits)
            .as_ptr()
            .expect("fixture instance");
        assert!(unsafe { (*header_from_obj_ptr(ptr)).gc_is_published() });
        bits
    }

    static REENTRANT_WEAK_ITER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static REENTRANT_WEAK_STATE: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);
    static REENTRANT_WEAK_RESULT: std::sync::atomic::AtomicU64 =
        std::sync::atomic::AtomicU64::new(0);

    extern "C" fn reenter_weak_iterator_on_value_drop(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(_py, {
            use std::sync::atomic::Ordering::SeqCst;
            let iter = REENTRANT_WEAK_ITER.load(SeqCst);
            let state = REENTRANT_WEAK_STATE.load(SeqCst);
            let mut item = MoltObject::none().bits();
            let done = unsafe { molt_iter_next_unboxed(iter, (&raw mut item) as usize as u64) };
            dec_ref_bits(_py, item);
            let completed = MoltObject::from_bits(done).as_bool() == Some(true)
                && !crate::exception_pending(_py);
            // The reentrant completion has released the iterator's state edge.
            // The outer terminal transition must still pin it while draining.
            let len = crate::molt_weakcontainer_len(state);
            let live =
                MoltObject::from_bits(len).as_int() == Some(0) && !crate::exception_pending(_py);
            dec_ref_bits(_py, len);
            REENTRANT_WEAK_RESULT.store(u64::from(completed) | (u64::from(live) << 1), SeqCst);
            MoltObject::none().bits()
        })
    }

    #[test]
    fn weak_iterator_completion_is_published_before_reentrant_pending_release() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            use std::sync::atomic::Ordering::SeqCst;
            unsafe {
                let key_name = MoltObject::from_ptr(alloc_string(_py, b"WeakIterKey")).bits();
                let value_name =
                    MoltObject::from_ptr(alloc_string(_py, b"WeakIterFinalizer")).bits();
                let key_class = crate::molt_class_new(key_name);
                let value_class = crate::molt_class_new(value_name);
                let finalizer_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    _py,
                    crate::provenance::abi::expose_function_address(
                        reenter_weak_iterator_on_value_drop as *const (),
                    ),
                    1,
                );
                assert!(!finalizer_ptr.is_null());
                let finalizer = MoltObject::from_ptr(finalizer_ptr).bits();
                let del_name = MoltObject::from_ptr(alloc_string(_py, b"__del__")).bits();
                crate::molt_set_attr_name(value_class, del_name, finalizer);
                let key = iterator_test_instance(_py, key_class);
                let value = iterator_test_instance(_py, value_class);
                let reference_class = crate::builtin_classes(_py).reference_type;
                let reference = crate::alloc_instance_for_class(
                    _py,
                    MoltObject::from_bits(reference_class).as_ptr().unwrap(),
                );
                assert_eq!(
                    crate::molt_weakref_register(reference, key, MoltObject::none().bits()),
                    MoltObject::from_bool(true).bits(),
                );
                let state = crate::molt_weakcontainer_new(MoltObject::from_int(1).bits());
                crate::molt_weakcontainer_store_commit(
                    state,
                    key,
                    value,
                    reference,
                    MoltObject::from_int(1).bits(),
                );
                let iter = crate::molt_weakcontainer_iter(state, MoltObject::from_int(1).bits());
                assert!(
                    !crate::exception_pending(_py),
                    "weak iterator fixture admission"
                );
                let mut item = MoltObject::none().bits();
                let first = molt_iter_next_unboxed(iter, (&raw mut item) as usize as u64);
                assert!(!crate::exception_pending(_py), "first weak iterator step");
                assert_eq!(MoltObject::from_bits(first).as_bool(), Some(false));
                dec_ref_bits(_py, item);
                // WeakKey callback removes this entry logically but defers its
                // owned value until the active iterator completes.
                crate::molt_weakcontainer_dead(state, reference);
                dec_ref_bits(_py, value);
                assert!(!crate::exception_pending(_py));
                REENTRANT_WEAK_ITER.store(iter, SeqCst);
                REENTRANT_WEAK_STATE.store(state, SeqCst);
                REENTRANT_WEAK_RESULT.store(0, SeqCst);
                dec_ref_bits(_py, state);
                let done = molt_iter_next_unboxed(iter, (&raw mut item) as usize as u64);
                assert_eq!(MoltObject::from_bits(done).as_bool(), Some(true));
                assert_eq!(REENTRANT_WEAK_RESULT.load(SeqCst), 3);
                assert!(!crate::exception_pending(_py));
                REENTRANT_WEAK_ITER.store(0, SeqCst);
                REENTRANT_WEAK_STATE.store(0, SeqCst);
                for bits in [
                    iter,
                    reference,
                    key,
                    key_class,
                    value_class,
                    finalizer,
                    del_name,
                    key_name,
                    value_name,
                ] {
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    #[test]
    fn exhaustion_retires_target_for_boxed_and_unboxed_iteration() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                for boxed in [false, true] {
                    for family in [
                        "list",
                        "list-int",
                        "list-bool",
                        "tuple",
                        "str",
                        "bytes",
                        "bytearray",
                        "range",
                        "range-finished",
                    ] {
                        let one = MoltObject::from_int(1).bits();
                        let target = match family {
                            "list" => MoltObject::from_ptr(crate::alloc_list(_py, &[one])).bits(),
                            "list-int" => MoltObject::from_ptr(
                                crate::object::builders::alloc_list_int_from_raw_slice(_py, &[1])
                                    .unwrap(),
                            )
                            .bits(),
                            "list-bool" => MoltObject::from_ptr(
                                crate::object::builders::alloc_list_bool_from_raw_slice(_py, &[1])
                                    .unwrap(),
                            )
                            .bits(),
                            "tuple" => MoltObject::from_ptr(crate::alloc_tuple(_py, &[one])).bits(),
                            "str" => MoltObject::from_ptr(crate::alloc_string(_py, b"a")).bits(),
                            "bytes" => MoltObject::from_ptr(crate::alloc_bytes(_py, b"a")).bits(),
                            "bytearray" => {
                                MoltObject::from_ptr(crate::alloc_bytearray(_py, b"a")).bits()
                            }
                            _ => super::molt_range_new(MoltObject::from_int(0).bits(), one, one),
                        };
                        let before = refcount(target);
                        let immortal =
                            (*header_from_obj_ptr(MoltObject::from_bits(target).as_ptr().unwrap()))
                                .has_flag(crate::object::HEADER_FLAG_IMMORTAL);
                        let iter = molt_iter(target);
                        assert!(
                            !crate::exception_pending(_py),
                            "{family}: iterator admission"
                        );
                        assert_eq!(refcount(target), before + u32::from(!immortal), "{family}");
                        let iter_ptr = MoltObject::from_bits(iter).as_ptr().expect("iterator");
                        if family == "range-finished" {
                            super::iter_set_index(iter_ptr, crate::ITER_EXHAUSTED);
                        }
                        for step in 0..3 {
                            let exhausted = step != 0 || family == "range-finished";
                            if boxed {
                                let pair = super::molt_iter_next(iter);
                                let pair_ptr = MoltObject::from_bits(pair).as_ptr().expect("pair");
                                let (_, done) =
                                    crate::object::seq_access::tuple_pair(pair_ptr).expect("pair");
                                assert_eq!(
                                    MoltObject::from_bits(done).as_bool(),
                                    Some(exhausted),
                                    "{family}"
                                );
                                dec_ref_bits(_py, pair);
                            } else {
                                let mut item = MoltObject::none().bits();
                                let done =
                                    molt_iter_next_unboxed(iter, (&raw mut item) as usize as u64);
                                assert_eq!(
                                    MoltObject::from_bits(done).as_bool(),
                                    Some(exhausted),
                                    "{family}"
                                );
                                if exhausted {
                                    assert!(MoltObject::from_bits(item).is_none());
                                }
                                dec_ref_bits(_py, item);
                            }
                            assert_eq!(
                                refcount(target),
                                before + u32::from(!immortal && !exhausted),
                                "{family}: exhaustion must release its target immediately"
                            );
                            // Even immortal targets must be retired; their
                            // saturated refcount cannot demonstrate this edge.
                            assert_eq!(
                                super::iter_target_bits(iter_ptr),
                                if exhausted {
                                    MoltObject::none().bits()
                                } else {
                                    target
                                },
                                "{family}: target ownership is published absent on exhaustion",
                            );
                            if exhausted {
                                assert_eq!(super::iter_index(iter_ptr), crate::ITER_EXHAUSTED);
                                assert!((*super::iter_pair_slot(iter_ptr)).is_null());
                            }
                            assert!(
                                !crate::exception_pending(_py),
                                "{family}: iterator step {step}"
                            );
                            if step == 1 && family == "list" {
                                crate::molt_list_append(target, MoltObject::from_int(1).bits());
                            }
                        }
                        dec_ref_bits(_py, iter);
                        dec_ref_bits(_py, target);
                    }
                }
            }
        });
    }

    #[test]
    fn exhausted_dict_items_iterator_releases_view_dict_and_last_pair() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                for transport in ["boxed", "value-out", "dict-items-out"] {
                    let key = MoltObject::from_int(1).bits();
                    let class_name =
                        MoltObject::from_ptr(alloc_string(_py, b"IterOwnedValue")).bits();
                    let class = crate::molt_class_new(class_name);
                    let value = iterator_test_instance(_py, class);
                    let dict =
                        MoltObject::from_ptr(alloc_dict_with_pairs(_py, &[key, value])).bits();
                    assert_eq!(refcount(value), 2, "dict owns one value edge");

                    let view = molt_dict_items(dict);
                    let iter = molt_iter(view);
                    assert!(
                        !crate::exception_pending(_py),
                        "{transport}: iterator admission"
                    );
                    let iter_ptr = MoltObject::from_bits(iter).as_ptr().expect("iterator");
                    dec_ref_bits(_py, view);

                    for step in 0..3 {
                        let mut outputs = [MoltObject::none().bits(); 2];
                        let done = if transport == "dict-items-out" {
                            super::molt_iter_next_dict_items(
                                iter,
                                outputs.as_mut_ptr(),
                                outputs.as_mut_ptr().add(1),
                            )
                        } else {
                            let mut pair = MoltObject::none().bits();
                            let mut wrapper = MoltObject::none().bits();
                            let done = if transport == "boxed" {
                                wrapper = super::molt_iter_next(iter);
                                assert!(!crate::exception_pending(_py), "boxed iterator step");
                                let wrapper_ptr =
                                    MoltObject::from_bits(wrapper).as_ptr().expect("wrapper");
                                let (borrowed_pair, done) =
                                    crate::object::seq_access::tuple_pair(wrapper_ptr)
                                        .expect("wrapper");
                                // Keep the wrapper owner while consuming this
                                // borrowed alias; do not invent an extra pair owner.
                                pair = borrowed_pair;
                                done
                            } else {
                                molt_iter_next_unboxed(iter, (&raw mut pair) as usize as u64)
                            };
                            assert!(!crate::exception_pending(_py), "{transport}: iterator step");
                            if MoltObject::from_bits(done).as_bool() == Some(false) {
                                assert_eq!(
                                    molt_unpack_sequence(
                                        pair,
                                        outputs.len() as u64,
                                        outputs.as_mut_ptr() as usize as u64,
                                    ),
                                    0,
                                );
                            }
                            if transport == "boxed" {
                                dec_ref_bits(_py, wrapper);
                            } else {
                                dec_ref_bits(_py, pair);
                            }
                            done
                        };
                        assert!(!crate::exception_pending(_py), "{transport}: step {step}");
                        assert_eq!(MoltObject::from_bits(done).as_bool(), Some(step != 0));
                        assert_eq!(
                            outputs,
                            if step == 0 {
                                [key, value]
                            } else {
                                [MoltObject::none().bits(); 2]
                            },
                            "{transport}: caller-owned output transport",
                        );
                        for output in outputs {
                            dec_ref_bits(_py, output);
                        }
                        if step != 0 {
                            assert_eq!(
                                super::iter_target_bits(iter_ptr),
                                MoltObject::none().bits()
                            );
                            assert_eq!(super::iter_index(iter_ptr), crate::ITER_EXHAUSTED);
                            assert!((*super::iter_pair_slot(iter_ptr)).is_null());
                            assert_eq!(
                                refcount(dict),
                                1,
                                "{transport}: exhaustion releases the view before iterator teardown",
                            );
                            assert_eq!(
                                refcount(value),
                                2,
                                "{transport}: only dictionary and test owners remain",
                            );
                        }
                    }

                    dec_ref_bits(_py, iter);
                    dec_ref_bits(_py, dict);
                    assert_eq!(
                        refcount(value),
                        1,
                        "dict teardown releases the last yielded value"
                    );
                    dec_ref_bits(_py, value);
                    dec_ref_bits(_py, class);
                    dec_ref_bits(_py, class_name);
                }
            }
        });
    }

    static REENTRANT_PAIR_SLOT: std::sync::atomic::AtomicPtr<*mut u8> =
        std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
    static REENTRANT_PAIR_REPLACE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static REENTRANT_PAIR_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    extern "C" fn reenter_pair_cache_on_value_drop(_self: u64) -> u64 {
        crate::with_gil_entry_nopanic!(_py, {
            use std::sync::atomic::Ordering::SeqCst;
            unsafe {
                let slot = REENTRANT_PAIR_SLOT.load(SeqCst);
                assert!(!slot.is_null());
                REENTRANT_PAIR_CALLS.fetch_add(1, SeqCst);
                if REENTRANT_PAIR_REPLACE.load(SeqCst) {
                    let nested = cached_pair_return(
                        _py,
                        slot,
                        MoltObject::from_int(99).bits(),
                        MoltObject::from_bool(false).bits(),
                        false,
                        false,
                    );
                    dec_ref_bits(_py, nested);
                } else {
                    super::cached_pair_clear(_py, slot);
                }
            }
            MoltObject::none().bits()
        })
    }

    #[test]
    fn cached_pair_result_is_owned_before_reentrant_old_element_release() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            use std::sync::atomic::Ordering::SeqCst;
            unsafe {
                for (replace, abi_view) in
                    [(false, false), (true, false), (false, true), (true, true)]
                {
                    let name = MoltObject::from_ptr(alloc_string(_py, b"PairFinalizer")).bits();
                    let class = crate::molt_class_new(name);
                    let finalizer_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                        _py,
                        crate::provenance::abi::expose_function_address(
                            reenter_pair_cache_on_value_drop as *const (),
                        ),
                        1,
                    );
                    assert!(!finalizer_ptr.is_null());
                    let finalizer = MoltObject::from_ptr(finalizer_ptr).bits();
                    let del_name = MoltObject::from_ptr(alloc_string(_py, b"__del__")).bits();
                    crate::molt_set_attr_name(class, del_name, finalizer);
                    let value = iterator_test_instance(_py, class);
                    let mut cached = std::ptr::null_mut();
                    let slot = &raw mut cached;
                    REENTRANT_PAIR_SLOT.store(slot, SeqCst);
                    REENTRANT_PAIR_REPLACE.store(replace, SeqCst);
                    REENTRANT_PAIR_CALLS.store(0, SeqCst);
                    let first = cached_pair_return(
                        _py,
                        slot,
                        value,
                        MoltObject::from_bool(false).bits(),
                        true,
                        false,
                    );
                    dec_ref_bits(_py, first);
                    if abi_view {
                        molt_cpython_abi::bridge::molt_cpython_abi_init();
                        crate::cpython_abi_hooks::register_cpython_hooks();
                        // The returned owner was dropped above. This identity
                        // is borrowed from the live cache, not a new item owner.
                        let cached_bits = MoltObject::from_ptr(cached).bits();
                        let borrowed = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                            .handle_to_borrowed_pyobj(cached_bits);
                        assert!(!borrowed.is_null());
                        assert_eq!(refcount(first), 2, "cache plus stable ABI view owner");
                        assert!(
                            (*header_from_obj_ptr(cached))
                                .has_flag(crate::object::HEADER_FLAG_HAS_ABI_VIEW)
                        );
                        assert!(!molt_cpython_abi::bridge::GLOBAL_BRIDGE.has_direct_c_refs(first));
                    }
                    let next_value = MoltObject::from_int(2).bits();
                    let result = cached_pair_return(
                        _py,
                        slot,
                        next_value,
                        MoltObject::from_bool(false).bits(),
                        false,
                        false,
                    );
                    let result_ptr = MoltObject::from_bits(result).as_ptr().expect("owned pair");
                    assert_eq!(
                        REENTRANT_PAIR_CALLS.load(SeqCst),
                        1,
                        "old element finalizer must exercise the reentrant cache transition",
                    );
                    assert_eq!(
                        crate::object::seq_access::item(result_ptr, 0),
                        Some(next_value)
                    );
                    assert_eq!(
                        refcount(result),
                        1,
                        "nested next releases the old cache owner"
                    );
                    if replace {
                        assert!(!cached.is_null());
                        assert_ne!(cached, result_ptr);
                        assert_eq!(
                            crate::object::seq_access::item(cached, 0),
                            Some(MoltObject::from_int(99).bits()),
                        );
                    } else {
                        assert!(cached.is_null());
                    }
                    assert!(!crate::exception_pending(_py));
                    dec_ref_bits(_py, result);
                    super::cached_pair_clear(_py, slot);
                    REENTRANT_PAIR_SLOT.store(std::ptr::null_mut(), SeqCst);
                    for bits in [del_name, finalizer, class, name] {
                        dec_ref_bits(_py, bits);
                    }
                }
            }
        });
    }

    #[test]
    fn cached_pair_reuse_updates_contains_refs_flag() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let mut cached = std::ptr::null_mut();
                let slot = &mut cached as *mut *mut u8;

                let first = cached_pair_return(
                    _py,
                    slot,
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_bool(false).bits(),
                    false,
                    false,
                );
                let first_ptr = MoltObject::from_bits(first).as_ptr().expect("tuple pair");
                assert_eq!(first_ptr, cached);
                let first_header = header_from_obj_ptr(first_ptr);
                assert_eq!(
                    (*first_header).load_metadata_flags() & HEADER_FLAG_CONTAINS_REFS,
                    0,
                    "primitive cached pair should not be marked as ref-containing",
                );
                dec_ref_bits(_py, first);

                let text_ptr = alloc_string(_py, b"owned");
                assert!(!text_ptr.is_null());
                let text_bits = MoltObject::from_ptr(text_ptr).bits();
                let second = cached_pair_return(
                    _py,
                    slot,
                    text_bits,
                    MoltObject::from_bool(false).bits(),
                    false,
                    false,
                );
                let second_ptr = MoltObject::from_bits(second).as_ptr().expect("tuple pair");
                assert_eq!(second_ptr, cached, "cached tuple should be reused in place");
                let second_header = header_from_obj_ptr(second_ptr);
                assert_ne!(
                    (*second_header).load_metadata_flags() & HEADER_FLAG_CONTAINS_REFS,
                    0,
                    "cached pair mutated to hold heap refs must mark the tuple for element decref",
                );
                assert_eq!(
                    crate::object::seq_access::item(second_ptr, 0),
                    Some(text_bits)
                );
                dec_ref_bits(_py, second);
                dec_ref_bits(_py, MoltObject::from_ptr(cached).bits());
                dec_ref_bits(_py, text_bits);
            }
        });
    }
}
