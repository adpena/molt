//! Set and frozenset operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_set_*` / `molt_frozenset_*` is a separate
//! linker symbol so that `wasm-ld --gc-sections` can drop unused entries.

use crate::*;
use molt_obj_model::MoltObject;

use super::ops::{ensure_hashable, set_rebuild};
use super::ops_arith::{
    set_from_iter_bits, set_like_copy_bits, set_like_difference, set_like_intersection,
    set_like_ptr_from_bits, set_like_result_type_id, set_like_symdiff, set_like_union,
};

/// Specialized `in` for set/frozenset containers (hash lookup, no type dispatch).
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_contains(container_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let container = obj_from_bits(container_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
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
        }
        // Fallback for non-pointer (shouldn't happen with correct type hints)
        molt_contains(container_bits, item_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_add(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_add(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_discard(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_remove(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_pop(set_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_clear(set_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_copy_method(set_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(set_bits);
        let other = obj_from_bits(other_bits);
        let Some(set_ptr) = obj.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(set_ptr) != TYPE_ID_SET {
                return MoltObject::none().bits();
            }
            if let Some(other_ptr) = other.as_ptr() {
                let other_type = object_type_id(other_ptr);
                if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                    if other_ptr == set_ptr {
                        return MoltObject::none().bits();
                    }
                    let entries = set_order(other_ptr);
                    for entry in entries.iter().copied() {
                        set_add_in_place(_py, set_ptr, entry);
                    }
                    return MoltObject::none().bits();
                }
                if is_set_view_type(other_type) {
                    let Some(bits) = dict_view_as_set_bits(_py, other_ptr, other_type) else {
                        return MoltObject::none().bits();
                    };
                    let Some(view_set_ptr) = obj_from_bits(bits).as_ptr() else {
                        dec_ref_bits(_py, bits);
                        return MoltObject::none().bits();
                    };
                    let entries = set_order(view_set_ptr);
                    for entry in entries.iter().copied() {
                        set_add_in_place(_py, set_ptr, entry);
                    }
                    dec_ref_bits(_py, bits);
                    return MoltObject::none().bits();
                }
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
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_intersection_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(set_bits);
        let other = obj_from_bits(other_bits);
        if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
            unsafe {
                if object_type_id(set_ptr) == TYPE_ID_SET {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                        if other_ptr == set_ptr {
                            return MoltObject::none().bits();
                        }
                        let other_order = set_order(other_ptr);
                        let other_table = set_table(other_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let mut new_entries = Vec::with_capacity(set_entries.len());
                        for entry in set_entries {
                            let found = set_find_entry(_py, other_order, other_table, entry);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            if found.is_some() {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        return MoltObject::none().bits();
                    }
                    if is_set_view_type(other_type) {
                        let Some(bits) = dict_view_as_set_bits(_py, other_ptr, other_type) else {
                            return MoltObject::none().bits();
                        };
                        let Some(view_set_ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        let other_order = set_order(view_set_ptr);
                        let other_table = set_table(view_set_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let mut new_entries = Vec::with_capacity(set_entries.len());
                        for entry in set_entries {
                            let found = set_find_entry(_py, other_order, other_table, entry);
                            if exception_pending(_py) {
                                dec_ref_bits(_py, bits);
                                return MoltObject::none().bits();
                            }
                            if found.is_some() {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        dec_ref_bits(_py, bits);
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
                        let found = set_find_entry(_py, other_order, other_table, entry);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, other_set_bits);
                            return MoltObject::none().bits();
                        }
                        if found.is_some() {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_difference_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(set_bits);
        let other = obj_from_bits(other_bits);
        if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
            unsafe {
                if object_type_id(set_ptr) == TYPE_ID_SET {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                        if other_ptr == set_ptr {
                            set_replace_entries(_py, set_ptr, &[]);
                            return MoltObject::none().bits();
                        }
                        let other_order = set_order(other_ptr);
                        let other_table = set_table(other_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let mut new_entries = Vec::with_capacity(set_entries.len());
                        for entry in set_entries {
                            let found = set_find_entry(_py, other_order, other_table, entry);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            if found.is_none() {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        return MoltObject::none().bits();
                    }
                    if is_set_view_type(other_type) {
                        let Some(bits) = dict_view_as_set_bits(_py, other_ptr, other_type) else {
                            return MoltObject::none().bits();
                        };
                        let Some(view_set_ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        let other_order = set_order(view_set_ptr);
                        let other_table = set_table(view_set_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let mut new_entries = Vec::with_capacity(set_entries.len());
                        for entry in set_entries {
                            let found = set_find_entry(_py, other_order, other_table, entry);
                            if exception_pending(_py) {
                                dec_ref_bits(_py, bits);
                                return MoltObject::none().bits();
                            }
                            if found.is_none() {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        dec_ref_bits(_py, bits);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_symdiff_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(set_bits);
        let other = obj_from_bits(other_bits);
        if let (Some(set_ptr), Some(other_ptr)) = (obj.as_ptr(), other.as_ptr()) {
            unsafe {
                if object_type_id(set_ptr) == TYPE_ID_SET {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_SET || other_type == TYPE_ID_FROZENSET {
                        if other_ptr == set_ptr {
                            set_replace_entries(_py, set_ptr, &[]);
                            return MoltObject::none().bits();
                        }
                        let other_order = set_order(other_ptr);
                        let other_table = set_table(other_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let set_table_ptr = set_table(set_ptr);
                        let mut new_entries =
                            Vec::with_capacity(set_entries.len() + other_order.len());
                        for entry in &set_entries {
                            let found = set_find_entry(_py, other_order, other_table, *entry);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            if found.is_none() {
                                new_entries.push(*entry);
                            }
                        }
                        for entry in other_order.iter().copied() {
                            let found =
                                set_find_entry(_py, set_entries.as_slice(), set_table_ptr, entry);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
                            if found.is_none() {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        return MoltObject::none().bits();
                    }
                    if is_set_view_type(other_type) {
                        let Some(bits) = dict_view_as_set_bits(_py, other_ptr, other_type) else {
                            return MoltObject::none().bits();
                        };
                        let Some(view_set_ptr) = obj_from_bits(bits).as_ptr() else {
                            dec_ref_bits(_py, bits);
                            return MoltObject::none().bits();
                        };
                        let other_order = set_order(view_set_ptr);
                        let other_table = set_table(view_set_ptr);
                        let set_entries = set_order(set_ptr).clone();
                        let set_table_ptr = set_table(set_ptr);
                        let mut new_entries =
                            Vec::with_capacity(set_entries.len() + other_order.len());
                        for entry in &set_entries {
                            let found = set_find_entry(_py, other_order, other_table, *entry);
                            if exception_pending(_py) {
                                dec_ref_bits(_py, bits);
                                return MoltObject::none().bits();
                            }
                            if found.is_none() {
                                new_entries.push(*entry);
                            }
                        }
                        for entry in other_order.iter().copied() {
                            let found =
                                set_find_entry(_py, set_entries.as_slice(), set_table_ptr, entry);
                            if exception_pending(_py) {
                                dec_ref_bits(_py, bits);
                                return MoltObject::none().bits();
                            }
                            if found.is_none() {
                                new_entries.push(entry);
                            }
                        }
                        set_replace_entries(_py, set_ptr, &new_entries);
                        dec_ref_bits(_py, bits);
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
                        let found = set_find_entry(_py, other_order, other_table, *entry);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, other_set_bits);
                            return MoltObject::none().bits();
                        }
                        if found.is_none() {
                            new_entries.push(*entry);
                        }
                    }
                    for entry in other_order.iter().copied() {
                        let found =
                            set_find_entry(_py, set_entries.as_slice(), set_table_ptr, entry);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, other_set_bits);
                            return MoltObject::none().bits();
                        }
                        if found.is_none() {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_update_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_union_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_set_union_multi(set_bits, others_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_intersection_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_set_intersection_multi(set_bits, others_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_difference_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_set_difference_multi(set_bits, others_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_symmetric_difference(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_set_symmetric_difference(set_bits, other_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_isdisjoint(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_set_isdisjoint(set_bits, other_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_issubset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_set_issubset(set_bits, other_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_issuperset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_set_issuperset(set_bits, other_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_copy_method(set_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_intersection_update_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_difference_update_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_symmetric_difference_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = molt_set_symdiff_update(set_bits, other_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_union_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_intersection_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_difference_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_symmetric_difference(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_isdisjoint(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
                let found = set_find_entry(_py, probe_order, probe_table, entry);
                if exception_pending(_py) {
                    if let Some(bits) = drop_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                if found.is_some() {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_issubset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
                let found = set_find_entry(_py, other_order, other_table, entry);
                if exception_pending(_py) {
                    if let Some(bits) = drop_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                if found.is_none() {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_issuperset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
                let found = set_find_entry(_py, self_order, self_table, entry);
                if exception_pending(_py) {
                    if let Some(bits) = drop_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                if found.is_none() {
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
