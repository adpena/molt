//! Set and frozenset operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_set_*` / `molt_frozenset_*` is a separate
//! linker symbol so that `wasm-ld --gc-sections` can drop unused entries.

use crate::*;
use molt_obj_model::MoltObject;

use super::ops::{ensure_hashable, set_rebuild};
use super::ops_arith::{
    set_like_copy_bits, set_like_difference, set_like_intersection, set_like_ptr_from_bits,
    set_like_result_type_id,
};

/// Python membership admits mutable-set needles after a TypeError; the public
/// PySet_Contains API deliberately requires an already-hashable key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetContainsPolicy {
    Python,
    ExactKey,
}

pub(crate) fn set_contains(
    py: &PyToken<'_>,
    container_bits: u64,
    item_bits: u64,
    policy: SetContainsPolicy,
) -> u64 {
    use crate::builtins::exceptions::ExceptionValue;
    let Some(ptr) = obj_from_bits(container_bits)
        .as_ptr()
        .filter(|&ptr| unsafe { is_set_like_type(object_type_id(ptr)) })
    else {
        return raise_exception(
            py,
            "TypeError",
            "set containment requires a set or frozenset",
        );
    };
    let found = if ensure_hashable(py, item_bits, HashContext::SetElement) {
        unsafe { set_find_entry(py, ptr, item_bits) }
    } else {
        None
    };
    if !exception_pending(py) {
        return MoltObject::from_bool(found.is_some()).bits();
    }
    let mutable = obj_from_bits(item_bits)
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_SET });
    if policy == SetContainsPolicy::ExactKey || mutable.is_none() {
        return MoltObject::none().bits();
    }
    let error = ExceptionValue::adopt(py, molt_exception_last());
    if !crate::builtins::exceptions::exception_matches_builtin_name(py, error.bits(), "TypeError") {
        return MoltObject::none().bits();
    }
    // This is the prescribed set protocol, not a retry of a failed lookup.
    clear_exception(py);
    drop(error);
    let frozen = ExceptionValue::adopt(py, unsafe {
        set_like_copy_bits(py, mutable.unwrap(), TYPE_ID_FROZENSET)
    });
    if exception_pending(py) || obj_from_bits(frozen.bits()).is_none() {
        return MoltObject::none().bits();
    }
    let found = unsafe { set_find_entry(py, ptr, frozen.bits()) };
    if exception_pending(py) {
        MoltObject::none().bits()
    } else {
        MoltObject::from_bool(found.is_some()).bits()
    }
}

/// Specialized Python `in` shares the builtin descriptor's key policy.
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_contains(container_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        set_contains(py, container_bits, item_bits, SetContainsPolicy::Python)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_add(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !ensure_hashable(_py, key_bits, HashContext::SetElement) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    set_add_in_place(_py, ptr, key_bits, HashContext::SetElement);
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

/// Compiler construction entry for a probe-context temporary set. Runtime
/// intersection/subset methods use streaming probes directly.
#[unsafe(no_mangle)]
pub extern "C" fn molt_set_add_probe(set_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if !ensure_hashable(_py, key_bits, HashContext::Bare) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_SET {
                    set_add_in_place(_py, ptr, key_bits, HashContext::Bare);
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
        if !ensure_hashable(_py, key_bits, HashContext::SetElement) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(set_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_FROZENSET {
                    set_add_in_place(_py, ptr, key_bits, HashContext::SetElement);
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
                    // set_del_in_place returns false both when the key is absent
                    // and when ensure_hashable / set_find_entry already raised
                    // (e.g. an unhashable key -> TypeError). Don't clobber a
                    // pending exception with a (wrong) KeyError.
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    // CPython: set.remove(x) on a missing key raises KeyError(x)
                    // with the missing key OBJECT as the sole arg (str(e) ==
                    // repr(x)), not a descriptive string. Same on 3.12/3.13/3.14.
                    return raise_key_error_with_key(_py, key_bits);
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
                    let hashes = set_hashes(ptr);
                    hashes.pop();
                    let entries = order.len();
                    let table = set_table(ptr);
                    let capacity = set_table_capacity(entries.max(1));
                    set_rebuild(_py, order, hashes, table, capacity);
                    if order.is_empty() {
                        (*header_from_obj_ptr(ptr))
                            .fetch_and_flags(!crate::object::HEADER_FLAG_CONTAINS_REFS);
                    }
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
                    set_clear_in_place(_py, ptr);
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
                    if exact_storage(_py, ptr, TYPE_ID_FROZENSET, builtin_classes(_py).frozenset) {
                        inc_ref_bits(_py, set_bits);
                        set_bits
                    } else {
                        set_like_copy_bits(_py, ptr, TYPE_ID_FROZENSET)
                    }
                }
                _ => MoltObject::none().bits(),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Some(set) = obj_from_bits(set_bits).as_ptr() {
            unsafe {
                if object_type_id(set) == TYPE_ID_SET {
                    let _ = set_update_iterable(py, set, other_bits, HashContext::SetElement);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_intersection_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Some(set) = obj_from_bits(set_bits).as_ptr() {
            unsafe {
                if object_type_id(set) == TYPE_ID_SET {
                    let result = set_intersection_bits(py, set, other_bits, TYPE_ID_SET);
                    if !exception_pending(py)
                        && let Some(staged) = obj_from_bits(result).as_ptr()
                    {
                        super::ops::set_publish_staged(py, set, staged);
                    }
                    dec_ref_bits(py, result);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_difference_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Some(set) = obj_from_bits(set_bits).as_ptr() {
            unsafe {
                if object_type_id(set) == TYPE_ID_SET {
                    let _ = set_difference_update_iterable(py, set, other_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_symdiff_update(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Some(set) = obj_from_bits(set_bits).as_ptr() {
            unsafe {
                if object_type_id(set) == TYPE_ID_SET {
                    let _ = set_symdiff_update_iterable(py, set, other_bits);
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
            let Some(others) = crate::object::seq_access::snapshot(
                _py,
                others_ptr,
                "set operand snapshot allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
            for &other_bits in others.iter() {
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
                if exact_storage(_py, ptr, TYPE_ID_FROZENSET, builtin_classes(_py).frozenset) {
                    inc_ref_bits(_py, set_bits);
                    return set_bits;
                }
                return set_like_copy_bits(_py, ptr, TYPE_ID_FROZENSET);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_intersection_update_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if let Some(set) = obj_from_bits(set_bits).as_ptr() {
            unsafe {
                if object_type_id(set) == TYPE_ID_SET {
                    let result = molt_set_intersection_multi(set_bits, others_bits);
                    if !exception_pending(py)
                        && let Some(staged) = obj_from_bits(result).as_ptr()
                    {
                        super::ops::set_publish_staged(py, set, staged);
                    }
                    dec_ref_bits(py, result);
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
            let Some(others) = crate::object::seq_access::snapshot(
                _py,
                others_ptr,
                "set operand snapshot allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
            for &other_bits in others.iter() {
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
    crate::with_gil_entry_nopanic!(py, {
        let Some(set) = obj_from_bits(set_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(set)) {
                return MoltObject::none().bits();
            }
            let kind = set_like_result_type_id(object_type_id(set));
            let Some(args) = obj_from_bits(others_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(args) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let Some(others) = crate::object::seq_access::snapshot(
                py,
                args,
                "set operand snapshot allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
            let result_bits = set_like_copy_bits(py, set, kind);
            if exception_pending(py) || obj_from_bits(result_bits).as_ptr().is_none() {
                dec_ref_bits(py, result_bits);
                return MoltObject::none().bits();
            }
            for &other_bits in others.iter() {
                if other_bits == set_bits {
                    continue;
                }
                let result = obj_from_bits(result_bits).as_ptr().unwrap();
                if set_update_iterable(py, result, other_bits, HashContext::SetElement).is_err() {
                    dec_ref_bits(py, result_bits);
                    return MoltObject::none().bits();
                }
                if exception_pending(py) || obj_from_bits(result_bits).as_ptr().is_none() {
                    dec_ref_bits(py, result_bits);
                    return MoltObject::none().bits();
                }
            }
            result_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_intersection_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(set) = obj_from_bits(set_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(set)) {
                return MoltObject::none().bits();
            }
            let kind = set_like_result_type_id(object_type_id(set));
            let Some(args) = obj_from_bits(others_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(args) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let Some(others) = crate::object::seq_access::snapshot(
                py,
                args,
                "set operand snapshot allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
            let mut result_bits = if others.is_empty() {
                set_like_copy_bits(py, set, kind)
            } else {
                inc_ref_bits(py, set_bits);
                set_bits
            };
            if exception_pending(py) || obj_from_bits(result_bits).as_ptr().is_none() {
                dec_ref_bits(py, result_bits);
                return MoltObject::none().bits();
            }
            for &other_bits in others.iter() {
                let result = obj_from_bits(result_bits).as_ptr().unwrap();
                let next = set_intersection_bits(py, result, other_bits, kind);
                dec_ref_bits(py, result_bits);
                result_bits = next;
                if exception_pending(py) || obj_from_bits(result_bits).as_ptr().is_none() {
                    dec_ref_bits(py, result_bits);
                    return MoltObject::none().bits();
                }
            }
            result_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_difference_multi(set_bits: u64, others_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(set) = obj_from_bits(set_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(set)) {
                return MoltObject::none().bits();
            }
            let kind = set_like_result_type_id(object_type_id(set));
            let Some(args) = obj_from_bits(others_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(args) != TYPE_ID_TUPLE {
                return MoltObject::none().bits();
            }
            let Some(others) = crate::object::seq_access::snapshot(
                py,
                args,
                "set operand snapshot allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
            let mut result_bits = if others.is_empty() {
                set_like_copy_bits(py, set, kind)
            } else {
                inc_ref_bits(py, set_bits);
                set_bits
            };
            if exception_pending(py) || obj_from_bits(result_bits).as_ptr().is_none() {
                dec_ref_bits(py, result_bits);
                return MoltObject::none().bits();
            }
            for (index, &other_bits) in others.iter().enumerate() {
                let result = obj_from_bits(result_bits).as_ptr().unwrap();
                if index == 0 {
                    let next = set_difference_bits(py, result, other_bits, kind);
                    dec_ref_bits(py, result_bits);
                    result_bits = next;
                } else if set_difference_update_iterable(py, result, other_bits).is_err() {
                    dec_ref_bits(py, result_bits);
                    return MoltObject::none().bits();
                }
                if exception_pending(py) || obj_from_bits(result_bits).as_ptr().is_none() {
                    dec_ref_bits(py, result_bits);
                    return MoltObject::none().bits();
                }
            }
            result_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_symmetric_difference(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(set) = obj_from_bits(set_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(set)) {
                return MoltObject::none().bits();
            }
            // CPython constructs from other, then toggles the receiver's keys.
            let bits = new_set_result(set_like_result_type_id(object_type_id(set)));
            let Some(result) = obj_from_bits(bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if set_update_iterable(py, result, other_bits, HashContext::SetElement).is_err()
                || set_symdiff_update_iterable(py, result, set_bits).is_err()
                || exception_pending(py)
            {
                dec_ref_bits(py, bits);
                MoltObject::none().bits()
            } else {
                bits
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_isdisjoint(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(set) = obj_from_bits(set_bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(set)) {
                return MoltObject::none().bits();
            }
            if set_bits == other_bits {
                return MoltObject::from_bool(crate::builtins::containers::set_len(set) == 0)
                    .bits();
            }
            if let Some(other) = obj_from_bits(other_bits).as_ptr()
                && (exact_storage(py, other, TYPE_ID_SET, builtin_classes(py).set)
                    || exact_storage(py, other, TYPE_ID_FROZENSET, builtin_classes(py).frozenset))
            {
                let (source, probe) = if crate::builtins::containers::set_len(set)
                    < crate::builtins::containers::set_len(other)
                {
                    (set, other)
                } else {
                    (other, set)
                };
                let mut index = 0;
                while let Some(entry) = super::ops::set_pin_entry(py, source, index) {
                    let found = super::ops::set_find_entry_in_place_with_hash(
                        py,
                        probe,
                        entry.bits(),
                        entry.hash(),
                    );
                    drop(entry);
                    if exception_pending(py) {
                        return MoltObject::none().bits();
                    }
                    if found.is_some() {
                        return MoltObject::from_bool(false).bits();
                    }
                    index += 1;
                }
                MoltObject::from_bool(true).bits()
            } else {
                set_probe_iterable(py, set, other_bits, true)
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_issubset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let value = obj_from_bits(set_bits);
        let Some(set) = value.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(set)) {
                return MoltObject::none().bits();
            }
            if let Some(other) = obj_from_bits(other_bits).as_ptr()
                && is_set_like_type(object_type_id(other))
            {
                return super::ops_compare::builtin_families::family_for_value(py, value)
                    .unwrap()
                    .invoke(
                        py,
                        set_bits,
                        other_bits,
                        molt_obj_model::sequence_compare::RichCompareOp::Le,
                    );
            }
            let bits = set_intersection_bits(py, set, other_bits, TYPE_ID_SET);
            if exception_pending(py) {
                dec_ref_bits(py, bits);
                return MoltObject::none().bits();
            }
            let Some(result) = obj_from_bits(bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            let equal = crate::builtins::containers::set_len(result)
                == crate::builtins::containers::set_len(set);
            dec_ref_bits(py, bits);
            if exception_pending(py) {
                MoltObject::none().bits()
            } else {
                MoltObject::from_bool(equal).bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_issuperset(set_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let value = obj_from_bits(set_bits);
        let Some(set) = value.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if !is_set_like_type(object_type_id(set)) {
                return MoltObject::none().bits();
            }
            if let Some(other) = obj_from_bits(other_bits).as_ptr()
                && is_set_like_type(object_type_id(other))
            {
                return super::ops_compare::builtin_families::family_for_value(py, value)
                    .unwrap()
                    .invoke(
                        py,
                        set_bits,
                        other_bits,
                        molt_obj_model::sequence_compare::RichCompareOp::Ge,
                    );
            }
            set_probe_iterable(py, set, other_bits, false)
        }
    })
}

// Shared storage pins and cached-hash primitives keep callbacks safe; each
// operation below keeps its own traversal and mutation boundary.
struct SetOwned<'a, 'py> {
    py: &'a PyToken<'py>,
    bits: u64,
}
impl<'a, 'py> SetOwned<'a, 'py> {
    fn adopt(py: &'a PyToken<'py>, bits: u64) -> Self {
        Self { py, bits }
    }
    fn borrow(py: &'a PyToken<'py>, bits: u64) -> Self {
        inc_ref_bits(py, bits);
        Self { py, bits }
    }
}
impl Drop for SetOwned<'_, '_> {
    fn drop(&mut self) {
        dec_ref_bits(self.py, self.bits);
    }
}

unsafe fn exact_storage(_py: &PyToken<'_>, ptr: *mut u8, kind: u32, owner: u64) -> bool {
    unsafe {
        object_type_id(ptr) == kind && {
            let class = object_class_bits(ptr);
            class == 0 || class == owner
        }
    }
}

unsafe fn new_set_result(kind: u32) -> u64 {
    if kind == TYPE_ID_FROZENSET {
        molt_frozenset_new(0)
    } else {
        molt_set_new(0)
    }
}

unsafe fn pin_dict_key<'a, 'py>(
    py: &'a PyToken<'py>,
    dict: *mut u8,
    index: usize,
) -> Option<(SetOwned<'a, 'py>, u64)> {
    unsafe {
        let key = *dict_order(dict).get(index.checked_mul(2)?)?;
        let hash = *dict_hashes(dict).get(index)?;
        Some((SetOwned::borrow(py, key), hash))
    }
}

pub(crate) unsafe fn set_update_iterable(
    py: &PyToken<'_>,
    set: *mut u8,
    other_bits: u64,
    ctx: HashContext,
) -> Result<(), ()> {
    let _source = SetOwned::borrow(py, other_bits);
    unsafe {
        if let Some(other) = obj_from_bits(other_bits).as_ptr() {
            if is_set_like_type(object_type_id(other)) {
                if set == other {
                    return Ok(());
                }
                if crate::builtins::containers::set_len(set) == 0 {
                    super::ops::set_copy_into_empty(py, other, set);
                    return if exception_pending(py) {
                        Err(())
                    } else {
                        Ok(())
                    };
                }
                let mut index = 0;
                while let Some(entry) = super::ops::set_pin_entry(py, other, index) {
                    super::ops::set_add_with_hash_in_place(py, set, entry.bits(), entry.hash());
                    drop(entry);
                    if exception_pending(py) {
                        return Err(());
                    }
                    index += 1;
                }
                return Ok(());
            }
            if exact_storage(py, other, TYPE_ID_DICT, builtin_classes(py).dict) {
                let mut index = 0;
                while let Some((key, hash)) = pin_dict_key(py, other, index) {
                    super::ops::set_add_with_hash_in_place(py, set, key.bits, hash);
                    drop(key);
                    if exception_pending(py) {
                        return Err(());
                    }
                    index += 1;
                }
                return Ok(());
            }
        }
        let mut iter = crate::object::iterable::OwnedIterator::new(py, other_bits).ok_or(())?;
        while let Some(item) = iter.next()? {
            let item = SetOwned::adopt(py, item);
            set_add_in_place(py, set, item.bits, ctx);
            if exception_pending(py) {
                drop(iter);
                drop(item);
                return Err(());
            }
            drop(item);
            if exception_pending(py) {
                return Err(());
            }
        }
        Ok(())
    }
}

/// Intersection has a result that must retire between iterator and current
/// key on failure. A Rust loop-local item would reverse that observable order.
struct IntersectionCustody<'a, 'py> {
    iter: Option<crate::object::iterable::OwnedIterator<'a, 'py>>,
    result: Option<SetOwned<'a, 'py>>,
    key: Option<SetOwned<'a, 'py>>,
}

impl Drop for IntersectionCustody<'_, '_> {
    fn drop(&mut self) {
        drop(self.iter.take());
        drop(self.result.take());
        drop(self.key.take());
    }
}

unsafe fn set_intersection_bits(py: &PyToken<'_>, set: *mut u8, other_bits: u64, kind: u32) -> u64 {
    unsafe {
        if let Some(other) = obj_from_bits(other_bits).as_ptr()
            && is_set_like_type(object_type_id(other))
        {
            return set_like_intersection(py, set, other, kind);
        }
        let bits = new_set_result(kind);
        let Some(result) = obj_from_bits(bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        let mut custody = IntersectionCustody {
            iter: None,
            result: Some(SetOwned::adopt(py, bits)),
            key: None,
        };
        custody.iter = crate::object::iterable::OwnedIterator::new(py, other_bits);
        if custody.iter.is_none() {
            return MoltObject::none().bits();
        }
        loop {
            custody.key = match custody.iter.as_mut().unwrap().next() {
                Ok(Some(item)) => Some(SetOwned::adopt(py, item)),
                Ok(None) => break,
                Err(()) => return MoltObject::none().bits(),
            };
            let key = custody.key.as_ref().unwrap().bits;
            if !ensure_hashable(py, key, HashContext::Bare) {
                return MoltObject::none().bits();
            }
            let hash = hash_bits(py, key);
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let found = super::ops::set_find_entry_in_place_with_hash(py, set, key, hash);
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            let mut complete = false;
            if found.is_some() {
                super::ops::set_add_with_hash_in_place(py, result, key, hash);
                if exception_pending(py) {
                    return MoltObject::none().bits();
                }
                complete = crate::builtins::containers::set_len(result)
                    >= crate::builtins::containers::set_len(set);
            }
            drop(custody.key.take());
            if exception_pending(py) {
                return MoltObject::none().bits();
            }
            if complete {
                break;
            }
        }
        drop(custody.iter.take());
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        // Transfer the staged result only after iterator retirement succeeds.
        let result = custody.result.take().unwrap();
        let bits = result.bits;
        std::mem::forget(result);
        bits
    }
}

pub(in crate::object) unsafe fn set_difference_update_iterable(
    py: &PyToken<'_>,
    set: *mut u8,
    other_bits: u64,
) -> Result<(), ()> {
    let _source = SetOwned::borrow(py, other_bits);
    unsafe {
        if obj_from_bits(other_bits).as_ptr() == Some(set) {
            set_clear_in_place(py, set);
            return if exception_pending(py) {
                Err(())
            } else {
                Ok(())
            };
        }
        if let Some(other) = obj_from_bits(other_bits).as_ptr()
            && is_set_like_type(object_type_id(other))
        {
            // The size-dependent intersection is observable through callbacks.
            let temporary = if (crate::builtins::containers::set_len(other) >> 3)
                > crate::builtins::containers::set_len(set)
            {
                let bits = set_like_intersection(py, set, other, TYPE_ID_SET);
                if exception_pending(py) {
                    dec_ref_bits(py, bits);
                    return Err(());
                }
                Some(SetOwned::adopt(py, bits))
            } else {
                None
            };
            let source = temporary
                .as_ref()
                .map(|value| obj_from_bits(value.bits).as_ptr().unwrap())
                .unwrap_or(other);
            let mut index = 0;
            while let Some(entry) = super::ops::set_pin_entry(py, source, index) {
                super::ops::set_del_with_hash_in_place(py, set, entry.bits(), entry.hash());
                if exception_pending(py) {
                    drop(temporary);
                    drop(entry);
                    return Err(());
                }
                drop(entry);
                if exception_pending(py) {
                    return Err(());
                }
                index += 1;
            }
        } else {
            let mut iter = crate::object::iterable::OwnedIterator::new(py, other_bits).ok_or(())?;
            while let Some(item) = iter.next()? {
                let item = SetOwned::adopt(py, item);
                set_del_in_place(py, set, item.bits);
                if exception_pending(py) {
                    drop(iter);
                    drop(item);
                    return Err(());
                }
                drop(item);
                if exception_pending(py) {
                    return Err(());
                }
            }
        }
        Ok(())
    }
}

unsafe fn set_difference_bits(py: &PyToken<'_>, set: *mut u8, other_bits: u64, kind: u32) -> u64 {
    unsafe {
        if let Some(other) = obj_from_bits(other_bits).as_ptr() {
            if is_set_like_type(object_type_id(other)) {
                return set_like_difference(py, set, other, kind);
            }
            if exact_storage(py, other, TYPE_ID_DICT, builtin_classes(py).dict)
                && (crate::builtins::containers::set_len(set) >> 2) <= dict_len(other)
            {
                let bits = new_set_result(kind);
                let Some(result) = obj_from_bits(bits).as_ptr() else {
                    return MoltObject::none().bits();
                };
                let mut index = 0;
                while let Some(entry) = super::ops::set_pin_entry(py, set, index) {
                    let found = super::ops::dict_find_entry_with_hash(
                        py,
                        other,
                        entry.bits(),
                        entry.hash(),
                    );
                    if !exception_pending(py) && found.is_none() {
                        super::ops::set_add_with_hash_in_place(
                            py,
                            result,
                            entry.bits(),
                            entry.hash(),
                        );
                    }
                    if exception_pending(py) {
                        dec_ref_bits(py, bits);
                        drop(entry);
                        return MoltObject::none().bits();
                    }
                    drop(entry);
                    if exception_pending(py) {
                        dec_ref_bits(py, bits);
                        return MoltObject::none().bits();
                    }
                    index += 1;
                }
                return bits;
            }
        }
        let bits = set_like_copy_bits(py, set, kind);
        let Some(result) = obj_from_bits(bits).as_ptr() else {
            return MoltObject::none().bits();
        };
        if set_difference_update_iterable(py, result, other_bits).is_err() || exception_pending(py)
        {
            dec_ref_bits(py, bits);
            MoltObject::none().bits()
        } else {
            bits
        }
    }
}

unsafe fn set_toggle_entry(py: &PyToken<'_>, set: *mut u8, key: u64, hash: u64) -> Result<(), ()> {
    unsafe {
        let removed = super::ops::set_del_with_hash_in_place(py, set, key, hash);
        if exception_pending(py) {
            return Err(());
        }
        if !removed {
            super::ops::set_add_with_hash_in_place(py, set, key, hash);
        }
        if exception_pending(py) {
            Err(())
        } else {
            Ok(())
        }
    }
}

pub(in crate::object) unsafe fn set_symdiff_update_iterable(
    py: &PyToken<'_>,
    set: *mut u8,
    other_bits: u64,
) -> Result<(), ()> {
    let _source = SetOwned::borrow(py, other_bits);
    unsafe {
        if obj_from_bits(other_bits).as_ptr() == Some(set) {
            set_clear_in_place(py, set);
            return if exception_pending(py) {
                Err(())
            } else {
                Ok(())
            };
        }
        if let Some(other) = obj_from_bits(other_bits).as_ptr()
            && exact_storage(py, other, TYPE_ID_DICT, builtin_classes(py).dict)
        {
            let mut index = 0;
            while let Some((key, hash)) = pin_dict_key(py, other, index) {
                set_toggle_entry(py, set, key.bits, hash)?;
                drop(key);
                if exception_pending(py) {
                    return Err(());
                }
                index += 1;
            }
            return Ok(());
        }
        let (other, temporary) =
            set_like_ptr_from_bits(py, other_bits, HashContext::SetElement).ok_or(())?;
        let temporary = temporary.map(|bits| SetOwned::adopt(py, bits));
        let mut index = 0;
        while let Some(entry) = super::ops::set_pin_entry(py, other, index) {
            if set_toggle_entry(py, set, entry.bits(), entry.hash()).is_err() {
                drop(temporary);
                drop(entry);
                return Err(());
            }
            drop(entry);
            if exception_pending(py) {
                return Err(());
            }
            index += 1;
        }
        Ok(())
    }
}

unsafe fn set_probe_iterable(py: &PyToken<'_>, set: *mut u8, other: u64, disjoint: bool) -> u64 {
    let done = (|| -> Result<bool, ()> {
        let mut iter = crate::object::iterable::OwnedIterator::new(py, other).ok_or(())?;
        while let Some(item) = iter.next()? {
            let item = SetOwned::adopt(py, item);
            let found = unsafe { set_find_entry(py, set, item.bits) };
            drop(item);
            if exception_pending(py) {
                return Err(());
            }
            if found.is_some() == disjoint {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    match done {
        Ok(value) => MoltObject::from_bool(value).bits(),
        Err(()) => MoltObject::none().bits(),
    }
}
