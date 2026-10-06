//! Dict and mapping operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_dict_*` is a separate linker symbol.
//! Placing them in their own compilation unit lets `wasm-ld --gc-sections`
//! drop the entire block when no dict builtins are referenced.

use crate::*;
use molt_obj_model::MoltObject;

use super::ops::{
    dict_clear_in_place, dict_del_in_place, dict_find_entry, dict_get_in_place,
    dict_increment_exact_statement, dict_like_bits_from_ptr, dict_rebuild, dict_set_in_place,
    dict_set_inline_int_in_place, dict_setdefault_in_place, dict_table_capacity, ensure_hashable,
};

#[derive(Clone, Copy)]
pub(crate) enum DictSnapshotKind {
    Keys,
    Values,
    Items,
    /// Alternating key/value references from one insertion-order observation.
    Entries,
}

/// One insertion-ordered, retained snapshot for C list results, Python list
/// extension, native argument transport, and rendering. Backing is
/// resource-accounted; no table borrow crosses an
/// allocation or a release, and partial results retire through the shared owner.
pub(crate) unsafe fn dict_snapshot<'a, 'py>(
    py: &'a PyToken<'py>,
    dict: *mut u8,
    kind: DictSnapshotKind,
) -> Option<super::seq_access::PinnedSequenceSnapshot<'a, 'py>> {
    unsafe {
        let length = dict_order(dict).len();
        let count = length / 2;
        let capacity = if matches!(kind, DictSnapshotKind::Entries) {
            length
        } else {
            count
        };
        let Some(storage) = super::backing::tracked_vec_box_with_capacity::<u64>(capacity) else {
            record_memory_error_without_allocation(py);
            return None;
        };
        let mut values = super::backing::tracked_vec_box_from_raw(storage);
        for index in 0..count {
            let (key, value) = {
                let entries = dict_order(dict);
                (entries[2 * index], entries[2 * index + 1])
            };
            let item = match kind {
                DictSnapshotKind::Entries => {
                    inc_ref_bits(py, key);
                    inc_ref_bits(py, value);
                    values.push(key);
                    values.push(value);
                    continue;
                }
                DictSnapshotKind::Keys => {
                    inc_ref_bits(py, key);
                    key
                }
                DictSnapshotKind::Values => {
                    inc_ref_bits(py, value);
                    value
                }
                DictSnapshotKind::Items => {
                    let pair = alloc_tuple(py, &[key, value]);
                    if pair.is_null() {
                        let _partial = super::seq_access::PinnedSequenceSnapshot::from_owned_values(
                            py, values,
                        );
                        return None;
                    }
                    MoltObject::from_ptr(pair).bits()
                }
            };
            values.push(item);
        }
        Some(super::seq_access::PinnedSequenceSnapshot::from_owned_values(py, values))
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_update_missing(dict_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let key_obj = obj_from_bits(key_bits);
        if dict_obj.as_ptr().is_none() || key_obj.as_ptr().is_none() {
            return MoltObject::none().bits();
        }
        unsafe {
            let Some(container_ptr) = dict_obj.as_ptr() else {
                return MoltObject::none().bits();
            };
            let Some(real_dict_bits) = dict_like_bits_from_ptr(_py, container_ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    &format!(
                        "'{}' object does not support item assignment",
                        type_name(_py, dict_obj)
                    ),
                );
            };
            let Some(real_dict_ptr) = obj_from_bits(real_dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(real_dict_ptr) != TYPE_ID_DICT {
                return MoltObject::none().bits();
            }
            let missing = missing_bits(_py);
            if val_bits == missing {
                let _ = dict_del_in_place(_py, real_dict_ptr, key_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return dict_bits;
            }
            dict_set_in_place(_py, real_dict_ptr, key_bits, val_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            dict_bits
        }
    })
}

/// Specialized `in` for dict containers (hash lookup, no type dispatch).
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_contains(container_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let container = obj_from_bits(container_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    let found = dict_find_entry(_py, dict_ptr, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_bool(found.is_some()).bits();
                }
            }
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        molt_contains(container_bits, item_bits)
    })
}

type DictUpdateSetter = unsafe fn(&PyToken<'_>, u64, u64, u64);

pub(crate) unsafe fn dict_update_set_in_place(
    _py: &PyToken<'_>,
    dict_bits: u64,
    key_bits: u64,
    val_bits: u64,
) {
    unsafe {
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
}

pub(crate) unsafe fn dict_update_apply(
    _py: &PyToken<'_>,
    target_bits: u64,
    set_fn: DictUpdateSetter,
    other_bits: u64,
) -> u64 {
    unsafe {
        let direct_target =
            std::ptr::fn_addr_eq(set_fn, dict_update_set_in_place as DictUpdateSetter)
                .then(|| obj_from_bits(target_bits).as_ptr())
                .flatten();
        if direct_target.is_some() && target_bits == other_bits {
            return MoltObject::none().bits();
        }
        let outcome = crate::object::mapping_merge::apply(
            _py,
            other_bits,
            |_, _| true,
            |key, value, hash| {
                if let Some(target) = direct_target {
                    crate::object::mapping_merge::insert_dict(_py, target, key, value, hash)
                } else {
                    set_fn(_py, target_bits, key, value);
                    !exception_pending(_py)
                }
            },
        );
        if outcome != crate::object::mapping_merge::MergeOutcome::NotMapping {
            return MoltObject::none().bits();
        }
        let Some(mut iter) = crate::object::iterable::OwnedIterator::new(_py, other_bits) else {
            return MoltObject::none().bits();
        };
        let mut elem_index = 0usize;
        loop {
            let item = match iter.next() {
                Ok(Some(item)) => item,
                Ok(None) => return MoltObject::none().bits(),
                Err(molt_runtime_core::ErrorIndicatorSet) => return MoltObject::none().bits(),
            };
            let pair = dict_pair_from_item(_py, item);
            dec_ref_bits(_py, item);
            match pair {
                Ok((key, value)) => {
                    set_fn(_py, target_bits, key, value);
                    dec_ref_bits(_py, key);
                    dec_ref_bits(_py, value);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                Err(DictSeqError::NotIterable) => {
                    let message = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
                        "object is not iterable".to_string()
                    } else {
                        format!(
                            "cannot convert dictionary update sequence element #{elem_index} to a sequence"
                        )
                    };
                    return raise_exception::<_>(_py, "TypeError", &message);
                }
                Err(DictSeqError::BadLen(len)) => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        &format!(
                            "dictionary update sequence element #{elem_index} has length {len}; 2 is required"
                        ),
                    );
                }
                Err(DictSeqError::Exception) => return MoltObject::none().bits(),
            }
            elem_index += 1;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            // Ultra-fast path: plain dict container + inline int key.
            if object_type_id(ptr) == TYPE_ID_DICT {
                let key_obj = obj_from_bits(key_bits);
                if let Some(i) = key_obj.as_int() {
                    dict_set_inline_int_in_place(_py, ptr, key_bits, i, val_bits);
                    return dict_bits;
                }
                dict_set_in_place(_py, ptr, key_bits, val_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return dict_bits;
            }
            let Some(real_dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                // Fallback: not a plain dict, use the general store path.
                if !ensure_hashable(_py, key_bits, HashContext::DictKey) {
                    return MoltObject::none().bits();
                }
                return molt_store_index(dict_bits, key_bits, val_bits);
            };
            let Some(dict_ptr) = obj_from_bits(real_dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                if !ensure_hashable(_py, key_bits, HashContext::DictKey) {
                    return MoltObject::none().bits();
                }
                return molt_store_index(dict_bits, key_bits, val_bits);
            }
            // Direct dict set -- bypasses the generic molt_store_index dispatch.
            dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            dict_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_get(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.get expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "dict.get expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.get expects dict");
            }
            let found = dict_get_in_place(_py, dict_ptr, key_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Some(val) = found {
                inc_ref_bits(_py, val);
                return val;
            }
            inc_ref_bits(_py, default_bits);
            default_bits
        }
    })
}

/// `d[key] = d.get(key, 0) + delta` fused, returning whether it ran: only when
/// the statement provably runs no Python code (see
/// `dict_increment_exact_statement`). `False` has done nothing observable and
/// the caller runs the statement itself.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_str_int_inc(dict_bits: u64, key_bits: u64, delta_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match unsafe { dict_increment_exact_statement(_py, dict_bits, key_bits, delta_bits) } {
            Ok(done) => {
                if !done {
                    profile_hit_unchecked(&DICT_STR_INT_PREHASH_DEOPT_COUNT);
                }
                MoltObject::from_bool(done).bits()
            }
            Err(()) => MoltObject::none().bits(),
        }
    })
}

/// dict.pop(key, default=MISSING) — method dispatch entry point.
/// When default is MISSING, equivalent to pop(key) without a default
/// (raises KeyError if key is absent).  Otherwise, pop(key, default).
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_pop_method(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let has_default = !crate::builtins::methods::is_missing_bits(_py, default_bits);
        let actual_default = if has_default {
            default_bits
        } else {
            MoltObject::none().bits()
        };
        let flag = MoltObject::from_int(has_default as i64).bits();
        molt_dict_pop(dict_bits, key_bits, actual_default, flag)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_pop(
    dict_bits: u64,
    key_bits: u64,
    default_bits: u64,
    has_default_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let has_default = obj_from_bits(has_default_bits).as_int().unwrap_or(0) != 0;
        let Some(ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.pop expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "dict.pop expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.pop expects dict");
            }
            let found = dict_find_entry(_py, dict_ptr, key_bits);
            let order = dict_order(dict_ptr);
            let hashes = dict_hashes(dict_ptr);
            let table = dict_table(dict_ptr);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Some(entry_idx) = found {
                let key_idx = entry_idx * 2;
                let val_idx = key_idx + 1;
                let key_val = order[key_idx];
                let val_val = order[val_idx];
                inc_ref_bits(_py, val_val);
                order.drain(key_idx..=val_idx);
                hashes.remove(entry_idx);
                let entries = order.len() / 2;
                let capacity = dict_table_capacity(entries.max(1));
                dict_rebuild(_py, order, hashes, table, capacity);
                if order.is_empty() {
                    (*header_from_obj_ptr(dict_ptr))
                        .fetch_and_flags(!crate::object::HEADER_FLAG_CONTAINS_REFS);
                }
                crate::object::ops::dict_commit_structure(dict_ptr);
                dec_ref_bits(_py, key_val);
                dec_ref_bits(_py, val_val);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_setdefault(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let Some(ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
            }
            dict_setdefault_in_place(_py, dict_ptr, key_bits, Some(default_bits))
                .unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_setdefault_empty_list(dict_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let Some(ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.setdefault expects dict");
            }
            dict_setdefault_in_place(_py, dict_ptr, key_bits, None)
                .unwrap_or_else(|| MoltObject::none().bits())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_update(dict_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let dict_obj = obj_from_bits(dict_bits);
        let Some(ptr) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
            };
            dict_update_apply(_py, dict_bits, dict_update_set_in_place, other_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_clear(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_copy(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
            }
            let out_ptr = alloc_dict_with_pairs(_py, &[]);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let result = MoltObject::from_ptr(out_ptr).bits();
            dict_update_apply(_py, result, dict_update_set_in_place, dict_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, result);
                return MoltObject::none().bits();
            }
            result
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_popitem(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.popitem expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
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
            order.truncate(order.len() - 2);
            let hashes = dict_hashes(dict_ptr);
            hashes.truncate(hashes.len().saturating_sub(1));
            let entries = order.len() / 2;
            let table = dict_table(dict_ptr);
            let capacity = dict_table_capacity(entries.max(1));
            dict_rebuild(_py, order, hashes, table, capacity);
            if order.is_empty() {
                (*header_from_obj_ptr(dict_ptr))
                    .fetch_and_flags(!crate::object::HEADER_FLAG_CONTAINS_REFS);
            }
            crate::object::ops::dict_commit_structure(dict_ptr);
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, val_bits);
            MoltObject::from_ptr(item_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_update_kwstar(dict_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        unsafe {
            let Some(ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
            };
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(_py, "TypeError", "dict.update expects dict");
            };
            let Some(dict) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            // Preserve the call boundary: invalid/duplicate keywords must
            // not mutate the destination, and all keys/values are evaluated
            // before string validation.
            let keywords = alloc_dict_with_pairs(_py, &[]);
            if keywords.is_null() {
                return MoltObject::none().bits();
            }
            let _keywords_guard = PtrDropGuard::new(keywords);
            if !crate::object::mapping_merge::merge_keywords(_py, keywords, mapping_bits)
                || !crate::object::mapping_merge::validate_keywords(_py, keywords)
            {
                return MoltObject::none().bits();
            }
            dict_update_apply(
                _py,
                MoltObject::from_ptr(dict).bits(),
                dict_update_set_in_place,
                MoltObject::from_ptr(keywords).bits(),
            )
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_keys(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.keys expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_values(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.values expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_items(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.items expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
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

/// Returns the value for a key in a dict WITHOUT incrementing the refcount.
/// The dict holds the value alive. Returns 0 if the key is not found (clears
/// any KeyError). This mirrors CPython's `PyDict_GetItem()` borrowed-reference
/// semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_getitem_borrowed(dict_bits: u64, key_bits: u64) -> u64 {
    crate::c_api::PyDict_GetItem(dict_bits, key_bits)
}

struct CountElementOwned<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
}

impl<'a, 'py> CountElementOwned<'a, 'py> {
    fn adopt(py: &'a PyToken<'py>, bits: u64) -> Self {
        Self { py, bits }
    }
    fn borrow(py: &'a PyToken<'py>, bits: u64) -> Self {
        inc_ref_bits(py, bits);
        Self { py, bits }
    }
}

impl Drop for CountElementOwned<'_, '_> {
    fn drop(&mut self) {
        dec_ref_bits(self.py, self.bits);
    }
}

/// Terminal cleanup is a Python-visible sequence, not reverse local scope:
/// iterator, current key, current new count, then the bound get callable.
struct CountElementsCustody<'a, 'py> {
    iter: Option<crate::object::iterable::OwnedIterator<'a, 'py>>,
    key: Option<CountElementOwned<'a, 'py>>,
    new_value: Option<CountElementOwned<'a, 'py>>,
    bound_get: Option<CountElementOwned<'a, 'py>>,
}

impl Drop for CountElementsCustody<'_, '_> {
    fn drop(&mut self) {
        drop(self.iter.take());
        drop(self.key.take());
        drop(self.new_value.take());
        drop(self.bound_get.take());
    }
}

/// CPython's _count_elements fast path admits inherited dict descriptors by
/// actual namespace identity. Instance shadows and callable metadata are not
/// slot identity; genuine class overrides require normal mapping operations.
fn count_elements_dict_storage(
    py: &PyToken<'_>,
    mapping: u64,
    get_name: u64,
) -> Result<Option<*mut u8>, ()> {
    let class = CountElementOwned::borrow(py, type_of_bits(py, mapping));
    let Some(class_ptr) = obj_from_bits(class.bits).as_ptr() else {
        return Ok(None);
    };
    let dict = builtin_classes(py).dict;
    let dict_ptr = obj_from_bits(dict).as_ptr().ok_or(())?;
    let set_name =
        CountElementOwned::adopt(py, attr_name_bits_from_bytes(py, b"__setitem__").ok_or(())?);
    let raw = |owner, name| unsafe {
        crate::builtins::attr::class_attr_lookup_raw_mro(py, owner, name)
            .map(|bits| CountElementOwned::borrow(py, bits))
    };
    let mapping_get = raw(class_ptr, get_name);
    if exception_pending(py) {
        return Err(());
    }
    let dict_get = raw(dict_ptr, get_name);
    if exception_pending(py) {
        return Err(());
    }
    let mapping_setitem = raw(class_ptr, set_name.bits);
    if exception_pending(py) {
        return Err(());
    }
    let dict_setitem = raw(dict_ptr, set_name.bits);
    if exception_pending(py) {
        return Err(());
    }
    let same = |left: &Option<CountElementOwned<'_, '_>>,
                right: &Option<CountElementOwned<'_, '_>>| {
        matches!((left, right), (Some(left), Some(right)) if left.bits == right.bits)
    };
    if same(&mapping_get, &dict_get)
        && same(&mapping_setitem, &dict_setitem)
        && let Some(ptr) = obj_from_bits(mapping).as_ptr()
        && unsafe { object_type_id(ptr) == TYPE_ID_DICT }
    {
        return Ok(Some(ptr));
    }
    Ok(None)
}

/// Stream counts into the canonical mapping. No Counter registry, mirrored
/// key index, borrowed table or registry lock survives a Python callback.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_count_elements(mapping: u64, iterable: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _mapping = CountElementOwned::borrow(py, mapping);
        let _iterable = CountElementOwned::borrow(py, iterable);
        let Some(iter) = crate::object::iterable::OwnedIterator::new(py, iterable) else {
            return MoltObject::none().bits();
        };
        let mut custody = CountElementsCustody {
            iter: Some(iter),
            key: None,
            new_value: None,
            bound_get: None,
        };
        let Some(name) = attr_name_bits_from_bytes(py, b"get") else {
            return MoltObject::none().bits();
        };
        let name = CountElementOwned::adopt(py, name);
        let Ok(storage) = count_elements_dict_storage(py, mapping, name.bits) else {
            return MoltObject::none().bits();
        };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if storage.is_none() {
            custody.bound_get = Some(CountElementOwned::adopt(
                py,
                molt_get_attr_name(mapping, name.bits),
            ));
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
        }
        let zero = MoltObject::from_int(0).bits();
        let one = MoltObject::from_int(1).bits();
        loop {
            custody.key = match custody.iter.as_mut().unwrap().next() {
                Ok(Some(key)) => Some(CountElementOwned::adopt(py, key)),
                Ok(None) | Err(molt_runtime_core::ErrorIndicatorSet) => break,
            };
            let key = custody.key.as_ref().unwrap().bits;
            if let Some(dict) = storage {
                let hash = hash_bits(py, key);
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                let found = unsafe { super::ops::dict_find_entry_with_hash(py, dict, key, hash) };
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                let next = if let Some(index) = found {
                    let old =
                        CountElementOwned::borrow(py, unsafe { dict_order(dict)[index * 2 + 1] });
                    custody.new_value = Some(CountElementOwned::adopt(py, molt_add(old.bits, one)));
                    drop(old);
                    custody.new_value.as_ref().unwrap().bits
                } else {
                    one
                };
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                unsafe {
                    super::ops::dict_set_with_hash_in_place(py, dict, key, next, hash);
                }
            } else {
                let old = CountElementOwned::adopt(py, unsafe {
                    call_callable2(py, custody.bound_get.as_ref().unwrap().bits, key, zero)
                });
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                custody.new_value = Some(CountElementOwned::adopt(py, molt_add(old.bits, one)));
                drop(old);
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                // StoreIndex returns the borrowed mapping on success.
                let _ = molt_store_index(mapping, key, custody.new_value.as_ref().unwrap().bits);
            }
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            // Successful per-item retirement differs from terminal cleanup.
            drop(custody.new_value.take());
            drop(custody.key.take());
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
        }
        drop(custody);
        MoltObject::none().bits()
    })
}
