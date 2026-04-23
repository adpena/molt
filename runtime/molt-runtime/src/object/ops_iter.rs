//! Iterator and range operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_iter_*` / `molt_range_*` / `molt_enumerate_*`
//! etc. is a separate linker symbol so that `wasm-ld --gc-sections` can drop
//! unused entries.

use crate::object::{dec_ref_ptr, inc_ref_ptr};
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
                let elem_bits = MoltObject::from_int(value).bits();
                let Some(eq) = (unsafe { eq_bool_from_bits(_py, elem_bits, val_bits) }) else {
                    return MoltObject::none().bits();
                };
                if eq {
                    count += 1;
                }
                idx += 1;
            }
            if let Some(i) = count.to_i64() {
                return MoltObject::from_int(i).bits();
            }
            return int_bits_from_bigint(_py, count);
        }
        MoltObject::from_int(0).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_range_index(range_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
                    return MoltObject::from_int(i).bits();
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

#[unsafe(no_mangle)]
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
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_map_builtin(func_bits: u64, iterables_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let iterables_obj = obj_from_bits(iterables_bits);
        let Some(iterables_ptr) = iterables_obj.as_ptr() else {
            let single = [iterables_bits];
            return unsafe { map_new_impl(_py, func_bits, &single) };
        };
        unsafe {
            if object_type_id(iterables_ptr) == TYPE_ID_TUPLE {
                let iterables = seq_vec_ref(iterables_ptr);
                return map_new_impl(_py, func_bits, iterables.as_slice());
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
    crate::with_gil_entry!(_py, {
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
        let iters_ptr = Box::into_raw(Box::new(iters));
        *(zip_ptr as *mut *mut Vec<u64>) = iters_ptr;
        zip_set_strict_bits(zip_ptr, strict_bits);
        inc_ref_bits(_py, strict_bits);
        MoltObject::from_ptr(zip_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_zip_builtin(iterables_bits: u64, strict_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
            let iterables = seq_vec_ref(iterables_ptr);
            zip_new_impl(_py, iterables.as_slice(), strict)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_reversed_builtin(seq_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, { unsafe { reversed_new_impl(_py, seq_bits) } })
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
                } else if type_id == TYPE_ID_LIST || type_id == TYPE_ID_LIST_INT || type_id == TYPE_ID_LIST_BOOL {
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
            super::object_set_poll_fn(obj_ptr, anext_default_poll_fn_addr());
            super::object_set_state(obj_ptr, 0);
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
    crate::with_gil_entry!(_py, {
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
        *(enum_ptr as *mut u64) = iter_bits;
        *(enum_ptr.add(std::mem::size_of::<u64>()) as *mut u64) = index_bits;
        inc_ref_bits(_py, iter_bits);
        inc_ref_bits(_py, index_bits);
        MoltObject::from_ptr(enum_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_iter(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if matches!(
            std::env::var("MOLT_TRACE_ITER_ARG").ok().as_deref(),
            Some("1")
        ) {
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
                        if res == iter_bits {
                            // __iter__ returning self must hand out a new reference.
                            inc_ref_bits(_py, res);
                        }
                        return res;
                    }
                    dec_ref_bits(_py, name_bits);
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") {
                    if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits) {
                        dec_ref_bits(_py, call_bits);
                        dec_ref_bits(_py, name_bits);
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
                    dec_ref_bits(_py, name_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_iter_checked(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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

#[unsafe(no_mangle)]
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

/// Build or reuse a (value, done) 2-tuple from the iterator's cached slot.
///
/// When the cached tuple exists and its refcount is exactly 1 (exclusively
/// owned by the iterator), the elements are mutated in place — zero heap
/// allocations.  Otherwise a fresh tuple is allocated and cached for next
/// time.
///
/// `iter_ptr` — data pointer of the TYPE_ID_ITER object (past the header).
/// `val_bits` — the value element to place at index 0.
/// `done`     — whether the iterator is exhausted.
/// `owns_val` — if true the caller holds a NEW reference to `val_bits`
///              that should be consumed (i.e. the helper will dec-ref it
///              after the tuple inc-refs it).
///
/// # Safety
/// `iter_ptr` must point to valid TYPE_ID_ITER data.
unsafe fn iter_return_cached(
    _py: &PyToken<'_>,
    iter_ptr: *mut u8,
    val_bits: u64,
    done: bool,
    owns_val: bool,
) -> u64 {
    unsafe {
        let done_bits = MoltObject::from_bool(done).bits();
        let cached = iter_cached_tuple(iter_ptr);

        if !cached.is_null() {
            let header_ptr = cached.sub(std::mem::size_of::<MoltHeader>()) as *const MoltHeader;
            let rc = (*header_ptr)
                .ref_count
                .load(std::sync::atomic::Ordering::Relaxed);
            if rc == 1 {
                // Exclusively owned — reuse by mutating elements in place.
                let vec = seq_vec(cached);
                // Dec-ref old elements before overwriting.
                let old0 = vec[0];
                let old1 = vec[1];
                // Write new elements and inc-ref them (mirroring alloc_tuple
                // semantics where elements are inc-ref'd on insertion).
                vec[0] = val_bits;
                vec[1] = done_bits;
                inc_ref_bits(_py, val_bits);
                inc_ref_bits(_py, done_bits);
                // Now drop old refs.
                dec_ref_bits(_py, old0);
                dec_ref_bits(_py, old1);
                if owns_val {
                    dec_ref_bits(_py, val_bits);
                }
                // Bump refcount so the caller receives an owning reference
                // (cache keeps rc=1, caller gets +1 → rc=2).
                inc_ref_ptr(_py, cached);
                return MoltObject::from_ptr(cached).bits();
            }
            // Someone else holds a reference to the old cached tuple; drop
            // our cache reference and fall through to allocate a new one.
            dec_ref_ptr(_py, cached);
            iter_set_cached_tuple(iter_ptr, std::ptr::null_mut());
        }

        // Allocate a fresh tuple and cache it.
        let tuple_ptr = alloc_tuple(_py, &[val_bits, done_bits]);
        if tuple_ptr.is_null() {
            if owns_val {
                dec_ref_bits(_py, val_bits);
            }
            return MoltObject::none().bits();
        }
        if owns_val {
            dec_ref_bits(_py, val_bits);
        }
        // Cache: inc-ref so the tuple stays alive past the caller's dec-ref.
        inc_ref_ptr(_py, tuple_ptr);
        iter_set_cached_tuple(iter_ptr, tuple_ptr);
        // Return with the original refcount=1 as the caller's owning ref.
        MoltObject::from_ptr(tuple_ptr).bits()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_iter_next(iter_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
                    {
                        let elems = seq_vec_ref(res_ptr);
                        if elems.len() >= 2 {
                            let done = is_truthy(_py, obj_from_bits(elems[1]));
                            if done {
                                let closed_bits = MoltObject::from_bool(true).bits();
                                *(ptr.add(GEN_CLOSED_OFFSET) as *mut u64) = closed_bits;
                            }
                        }
                    }
                    return res_bits;
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
                    // Integer increment — enumerate counter is always int.
                    // molt_add is polymorphic and promotes to float if idx is float.
                    let next_bits = if let Some(i) = to_i64(obj_from_bits(idx_bits)) {
                        int_bits_from_i64(_py, i + 1)
                    } else {
                        molt_add(idx_bits, MoltObject::from_int(1).bits())
                    };
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
                    // Route map callable invocation through bind so Python
                    // function defaults are honored (e.g. def f(x, y=...)).
                    let builder_bits = molt_callargs_new(vals.len() as u64, 0);
                    if builder_bits == 0 {
                        return MoltObject::none().bits();
                    }
                    for &val_bits in &vals {
                        let _ = molt_callargs_push_pos(builder_bits, val_bits);
                    }
                    let res_bits = molt_call_bind(func_bits, builder_bits);
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
                        if done_flags.first().copied().unwrap_or(false) {
                            if let Some(idx) = done_flags[1..].iter().position(|done| !*done) {
                                let msg =
                                    format!("zip() argument {} is longer than argument 1", idx + 2);
                                return raise_exception::<_>(_py, "ValueError", &msg);
                            }
                            return generator_done_tuple(_py, MoltObject::none().bits());
                        }
                        if let Some(idx) = done_flags[1..].iter().position(|done| *done) {
                            let msg =
                                format!("zip() argument {} is shorter than argument 1", idx + 2);
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
                // Validate the target pointer before reading.
                // If the iterator or its target was freed, target_bits
                // will be garbage — return done to prevent crash.
                if !target_obj.is_ptr() {
                    return generator_done_tuple(_py, MoltObject::none().bits());
                }
                if let Some(target_ptr) = target_obj.as_ptr() {
                    let target_type = object_type_id(target_ptr);
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
                        let elems = seq_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            iter_set_index(ptr, ITER_EXHAUSTED);
                            return iter_return_cached(
                                _py,
                                ptr,
                                MoltObject::none().bits(),
                                true,
                                false,
                            );
                        }
                        let val_bits = elems[idx];
                        iter_set_index(ptr, idx + 1);
                        return iter_return_cached(_py, ptr, val_bits, false, false);
                    }
                    if target_type == TYPE_ID_LIST_INT {
                        let elems = crate::object::layout::list_int_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            iter_set_index(ptr, ITER_EXHAUSTED);
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
                            iter_set_index(ptr, ITER_EXHAUSTED);
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
                        && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__")
                    {
                        if let Some(call_bits) =
                            attr_lookup_ptr_allow_missing(_py, target_ptr, name_bits)
                        {
                            dec_ref_bits(_py, name_bits);
                            exception_stack_push();
                            let idx_bits = MoltObject::from_int(idx as i64).bits();
                            let val_bits = call_callable1(_py, call_bits, idx_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                let exc_bits = molt_exception_last();
                                let kind_bits = molt_exception_kind(exc_bits);
                                let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                                dec_ref_bits(_py, kind_bits);
                                if kind.as_deref() == Some("IndexError") {
                                    molt_exception_clear();
                                    exception_stack_pop(_py);
                                    dec_ref_bits(_py, exc_bits);
                                    return iter_return_cached(
                                        _py,
                                        ptr,
                                        MoltObject::none().bits(),
                                        true,
                                        false,
                                    );
                                }
                                dec_ref_bits(_py, exc_bits);
                                exception_stack_pop(_py);
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            iter_set_index(ptr, idx + 1);
                            return iter_return_cached(_py, ptr, val_bits, false, false);
                        }
                        dec_ref_bits(_py, name_bits);
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
/// Writes the next value to `*value_out` and returns `false`-bits when a
/// value is available, or `true`-bits when the iterator is exhausted.
/// Returns `None` bits when an exception is pending.
///
/// Fast-paths list, tuple, and i64-range iterators with zero allocation.
/// Everything else falls back to `molt_iter_next` + destructure.
///
/// # Safety
///
/// `value_out` must point to writable storage for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_iter_next_unboxed(iter_bits: u64, value_out: *mut u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let done_true = MoltObject::from_bool(true).bits();
        let done_false = MoltObject::from_bool(false).bits();

        let Some(ptr) = maybe_ptr_from_bits(iter_bits) else {
            return MoltObject::none().bits();
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
                    // NOTE: If the list is mutated during iteration (e.g. list.append()
                    // inside a for-loop), the backing Vec may reallocate, making
                    // `seq_vec_ref`'s pointer stale.  This matches CPython's behaviour:
                    // mutating a list while iterating is undefined / raises
                    // RuntimeError only in some cases.  A proper fix would add a
                    // version counter to the list layout (cf. CPython 3.12
                    // ma_version_tag) and check it on each iter_next call.
                    if target_type == TYPE_ID_LIST {
                        let elems = seq_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            iter_set_index(ptr, ITER_EXHAUSTED);
                            return done_true;
                        }
                        let val_bits = elems[idx];
                        inc_ref_bits(_py, val_bits);
                        *value_out = val_bits;
                        iter_set_index(ptr, idx + 1);
                        return done_false;
                    }

                    // ── LIST_INT fast path (zero alloc) ─────────
                    // Raw i64 storage — box on read, no refcount needed.
                    if target_type == TYPE_ID_LIST_INT {
                        let elems = crate::object::layout::list_int_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            iter_set_index(ptr, ITER_EXHAUSTED);
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
                            iter_set_index(ptr, ITER_EXHAUSTED);
                            return done_true;
                        }
                        let val_bits = MoltObject::from_bool(elems[idx] != 0).bits();
                        *value_out = val_bits;
                        iter_set_index(ptr, idx + 1);
                        return done_false;
                    }

                    // ── TUPLE fast path (zero alloc) ─────────────
                    if target_type == TYPE_ID_TUPLE {
                        let elems = seq_vec_ref(target_ptr);
                        if idx == ITER_EXHAUSTED || idx >= elems.len() {
                            iter_set_index(ptr, ITER_EXHAUSTED);
                            return done_true;
                        }
                        let val_bits = elems[idx];
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
                        let len = range_len_i128(start_i64, stop_i64, step_i64);
                        let len_usize = usize::try_from(len).unwrap_or(ITER_EXHAUSTED);
                        iter_set_index(ptr, len_usize);
                        return done_true;
                    }
                    // BigInt range — fall through to slow path.
                }
            }

            // ── Slow path: delegate to molt_iter_next ─────────────
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                if !obj_from_bits(pair_bits).is_none() {
                    dec_ref_bits(_py, pair_bits);
                }
                return MoltObject::none().bits();
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return done_true;
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, pair_bits);
                return done_true;
            }
            let elems = seq_vec_ref(pair_ptr);
            if elems.len() < 2 {
                dec_ref_bits(_py, pair_bits);
                return done_true;
            }
            let val_bits = elems[0];
            let exhausted_bits = elems[1];
            if is_truthy(_py, obj_from_bits(exhausted_bits)) {
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

#[unsafe(no_mangle)]
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
