//! Dict and mapping operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_dict_*` is a separate linker symbol.
//! Placing them in their own compilation unit lets `wasm-ld --gc-sections`
//! drop the entire block when no dict builtins are referenced.

use crate::*;
use molt_obj_model::MoltObject;

use super::ops::{
    dict_clear_in_place, dict_del_in_place, dict_find_entry, dict_get_in_place, dict_inc_in_place,
    dict_inc_prehashed_string_key_in_place, dict_like_bits_from_ptr, dict_rebuild,
    dict_set_in_place, dict_set_inline_int_in_place, dict_table_capacity, ensure_hashable,
};

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
            }
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
        let other_obj = obj_from_bits(other_bits);
        if let Some(ptr) = other_obj.as_ptr() {
            if object_type_id(ptr) == TYPE_ID_DICT {
                let iter_bits = molt_dict_items(other_bits);
                if obj_from_bits(iter_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let iter = molt_iter(iter_bits);
                dec_ref_bits(_py, iter_bits);
                if obj_from_bits(iter).is_none() {
                    return MoltObject::none().bits();
                }
                let mut elem_index = 0usize;
                loop {
                    let pair_bits = molt_iter_next(iter);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, iter);
                        return MoltObject::none().bits();
                    }
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        dec_ref_bits(_py, iter);
                        return MoltObject::none().bits();
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        dec_ref_bits(_py, pair_bits);
                        dec_ref_bits(_py, iter);
                        return MoltObject::none().bits();
                    }
                    let (item_bits, done_bits) = {
                        let elems = seq_vec_ref(pair_ptr);
                        if elems.len() < 2 {
                            dec_ref_bits(_py, pair_bits);
                            dec_ref_bits(_py, iter);
                            return MoltObject::none().bits();
                        }
                        (elems[0], elems[1])
                    };
                    if is_truthy(_py, obj_from_bits(done_bits)) {
                        dec_ref_bits(_py, pair_bits);
                        break;
                    }
                    match dict_pair_from_item(_py, item_bits) {
                        Ok((key, val)) => {
                            set_fn(_py, target_bits, key, val);
                            if exception_pending(_py) {
                                dec_ref_bits(_py, pair_bits);
                                dec_ref_bits(_py, iter);
                                return MoltObject::none().bits();
                            }
                        }
                        Err(DictSeqError::NotIterable) => {
                            dec_ref_bits(_py, pair_bits);
                            dec_ref_bits(_py, iter);
                            let msg = "object is not iterable".to_string();
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        Err(DictSeqError::BadLen(len)) => {
                            dec_ref_bits(_py, pair_bits);
                            dec_ref_bits(_py, iter);
                            let msg = format!(
                                "dictionary update sequence element #{elem_index} has length {len}; 2 is required"
                            );
                            return raise_exception::<_>(_py, "ValueError", &msg);
                        }
                        Err(DictSeqError::Exception) => {
                            dec_ref_bits(_py, pair_bits);
                            dec_ref_bits(_py, iter);
                            return MoltObject::none().bits();
                        }
                    }
                    dec_ref_bits(_py, pair_bits);
                    elem_index += 1;
                }
                dec_ref_bits(_py, iter);
                return MoltObject::none().bits();
            }
            if let Some(keys_bits) = attr_name_bits_from_bytes(_py, b"keys") {
                let keys_method_bits = attr_lookup_ptr(_py, ptr, keys_bits);
                dec_ref_bits(_py, keys_bits);
                if let Some(keys_method_bits) = keys_method_bits {
                    let keys_iterable = call_callable0(_py, keys_method_bits);
                    dec_ref_bits(_py, keys_method_bits);
                    let keys_iter = molt_iter(keys_iterable);
                    dec_ref_bits(_py, keys_iterable);
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
                        dec_ref_bits(_py, keys_iter);
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "dict.update expects a mapping or iterable",
                        );
                    };
                    loop {
                        let pair_bits = molt_iter_next(keys_iter);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, getitem_method_bits);
                            dec_ref_bits(_py, keys_iter);
                            return MoltObject::none().bits();
                        }
                        let pair_obj = obj_from_bits(pair_bits);
                        let Some(pair_ptr) = pair_obj.as_ptr() else {
                            dec_ref_bits(_py, getitem_method_bits);
                            dec_ref_bits(_py, keys_iter);
                            return MoltObject::none().bits();
                        };
                        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                            dec_ref_bits(_py, pair_bits);
                            dec_ref_bits(_py, getitem_method_bits);
                            dec_ref_bits(_py, keys_iter);
                            return MoltObject::none().bits();
                        }
                        let (key_bits, done_bits) = {
                            let elems = seq_vec_ref(pair_ptr);
                            if elems.len() < 2 {
                                dec_ref_bits(_py, pair_bits);
                                dec_ref_bits(_py, getitem_method_bits);
                                dec_ref_bits(_py, keys_iter);
                                return MoltObject::none().bits();
                            }
                            (elems[0], elems[1])
                        };
                        if is_truthy(_py, obj_from_bits(done_bits)) {
                            dec_ref_bits(_py, pair_bits);
                            break;
                        }
                        let val_bits = call_callable1(_py, getitem_method_bits, key_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, pair_bits);
                            dec_ref_bits(_py, getitem_method_bits);
                            dec_ref_bits(_py, keys_iter);
                            return MoltObject::none().bits();
                        }
                        set_fn(_py, target_bits, key_bits, val_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, pair_bits);
                            dec_ref_bits(_py, getitem_method_bits);
                            dec_ref_bits(_py, keys_iter);
                            return MoltObject::none().bits();
                        }
                        dec_ref_bits(_py, pair_bits);
                    }
                    dec_ref_bits(_py, getitem_method_bits);
                    dec_ref_bits(_py, keys_iter);
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
                dec_ref_bits(_py, iter);
                return MoltObject::none().bits();
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                dec_ref_bits(_py, iter);
                return MoltObject::none().bits();
            };
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                dec_ref_bits(_py, pair_bits);
                dec_ref_bits(_py, iter);
                return MoltObject::none().bits();
            }
            let (item_bits, done_bits) = {
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    dec_ref_bits(_py, pair_bits);
                    dec_ref_bits(_py, iter);
                    return MoltObject::none().bits();
                }
                (elems[0], elems[1])
            };
            if is_truthy(_py, obj_from_bits(done_bits)) {
                dec_ref_bits(_py, pair_bits);
                break;
            }
            match dict_pair_from_item(_py, item_bits) {
                Ok((key, val)) => {
                    set_fn(_py, target_bits, key, val);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, pair_bits);
                        dec_ref_bits(_py, iter);
                        return MoltObject::none().bits();
                    }
                }
                Err(DictSeqError::NotIterable) => {
                    dec_ref_bits(_py, pair_bits);
                    dec_ref_bits(_py, iter);
                    let msg = "object is not iterable".to_string();
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                Err(DictSeqError::BadLen(len)) => {
                    dec_ref_bits(_py, pair_bits);
                    dec_ref_bits(_py, iter);
                    let msg = format!(
                        "dictionary update sequence element #{elem_index} has length {len}; 2 is required"
                    );
                    return raise_exception::<_>(_py, "ValueError", &msg);
                }
                Err(DictSeqError::Exception) => {
                    dec_ref_bits(_py, pair_bits);
                    dec_ref_bits(_py, iter);
                    return MoltObject::none().bits();
                }
            }
            dec_ref_bits(_py, pair_bits);
            elem_index += 1;
        }
        dec_ref_bits(_py, iter);
        MoltObject::none().bits()
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
                // Fallback: not a plain dict, use the general store path.
                if !ensure_hashable(_py, key_bits) {
                    return MoltObject::none().bits();
                }
                return molt_store_index(dict_bits, key_bits, val_bits);
            };
            let Some(dict_ptr) = obj_from_bits(real_dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                if !ensure_hashable(_py, key_bits) {
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
    // Pre-materialize the key object to force pointer resolution and hash
    // caching before the dict lookup. In Cranelift-compiled binaries, NaN-boxed
    // key values can produce incorrect hash results without this step.
    {
        let key_obj = obj_from_bits(key_bits);
        if let Some(key_ptr) = key_obj.as_ptr() {
            unsafe {
                if object_type_id(key_ptr) == TYPE_ID_STRING {
                    let len = string_len(key_ptr);
                    // Force a volatile read of the first byte to prevent elision
                    if len > 0 {
                        std::ptr::read_volatile(string_bytes(key_ptr));
                    }
                }
            }
        }
    }
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_inc(dict_bits: u64, key_bits: u64, delta_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            }
            if !dict_inc_in_place(_py, dict_ptr, key_bits, delta_bits) {
                return MoltObject::none().bits();
            }
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_str_int_inc(dict_bits: u64, key_bits: u64, delta_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
        };
        unsafe {
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) else {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            }
            if let Some(done) =
                dict_inc_prehashed_string_key_in_place(_py, dict_ptr, key_bits, delta_bits)
            {
                if !done {
                    return MoltObject::none().bits();
                }
                return MoltObject::none().bits();
            }
            profile_hit_unchecked(&DICT_STR_INT_PREHASH_DEOPT_COUNT);
            if !dict_inc_in_place(_py, dict_ptr, key_bits, delta_bits) {
                return MoltObject::none().bits();
            }
            MoltObject::none().bits()
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
            let found = dict_find_entry(_py, order, table, key_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if let Some(entry_idx) = found {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_setdefault(dict_bits: u64, key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_setdefault_empty_list(dict_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
            let default_ptr = alloc_list(_py, &[]);
            if default_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let default_bits = MoltObject::from_ptr(default_ptr).bits();
            dict_set_in_place(_py, dict_ptr, key_bits, default_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, default_bits);
                return MoltObject::none().bits();
            }
            inc_ref_bits(_py, default_bits);
            default_bits
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_popitem(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_update_kwstar(dict_bits: u64, mapping_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_keys(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_values(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_items(dict_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

/// Returns the value for a key in a dict WITHOUT incrementing the refcount.
/// The dict holds the value alive. Returns 0 if the key is not found (clears
/// any KeyError). This mirrors CPython's `PyDict_GetItem()` borrowed-reference
/// semantics.
#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_getitem_borrowed(dict_bits: u64, key_bits: u64) -> u64 {
    // Pre-materialize the key to force pointer resolution and hash caching.
    {
        let key_obj = obj_from_bits(key_bits);
        if let Some(key_ptr) = key_obj.as_ptr() {
            unsafe {
                if object_type_id(key_ptr) == TYPE_ID_STRING {
                    let len = string_len(key_ptr);
                    if len > 0 {
                        std::ptr::read_volatile(string_bytes(key_ptr));
                    }
                }
            }
        }
    }
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(dict_bits);
        let Some(ptr) = obj.as_ptr() else {
            return 0;
        };
        unsafe {
            let Some(dict_raw) = dict_like_bits_from_ptr(_py, ptr) else {
                return 0;
            };
            let Some(dict_ptr) = obj_from_bits(dict_raw).as_ptr() else {
                return 0;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return 0;
            }
            if !ensure_hashable(_py, key_bits) {
                clear_exception(_py);
                return 0;
            }
            if let Some(val) = dict_get_in_place(_py, dict_ptr, key_bits) {
                // Borrowed: do NOT inc_ref
                return val;
            }
            // Key not found — clear any pending exception and return 0
            if exception_pending(_py) {
                clear_exception(_py);
            }
            0
        }
    })
}
