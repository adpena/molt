use crate::{
    HashContext, PyToken, TYPE_ID_FROZENSET, TYPE_ID_SET, TYPE_ID_TUPLE, dec_ref_bits,
    exception_pending, is_truthy, molt_frozenset_new, molt_iter, molt_iter_next, molt_set_new,
    obj_from_bits, object_type_id, raise_not_iterable, seq_vec_ref, set_add_in_place,
    set_find_entry, set_hashes, set_order, set_table,
};
use molt_obj_model::MoltObject;

pub(in crate::object) fn set_like_result_type_id(type_id: u32) -> u32 {
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

pub(in crate::object) unsafe fn set_like_union(
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
            set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
        }
        for &entry in r_elems.iter() {
            set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
        }
        res_bits
    }
}

pub(in crate::object) unsafe fn set_like_intersection(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let l_hashes = set_hashes(lhs_ptr);
        let r_hashes = set_hashes(rhs_ptr);
        let (probe_elems, probe_hashes, probe_table, output) = if l_elems.len() <= r_elems.len() {
            (r_elems, r_hashes, set_table(rhs_ptr), l_elems)
        } else {
            (l_elems, l_hashes, set_table(lhs_ptr), r_elems)
        };
        let res_bits = set_like_new_bits(result_type_id, output.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in output.iter() {
            let found = set_find_entry(_py, probe_elems, probe_hashes, probe_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_some() {
                set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(in crate::object) unsafe fn set_like_difference(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let r_hashes = set_hashes(rhs_ptr);
        let r_table = set_table(rhs_ptr);
        let res_bits = set_like_new_bits(result_type_id, l_elems.len());
        let res_ptr = obj_from_bits(res_bits)
            .as_ptr()
            .unwrap_or(std::ptr::null_mut());
        if res_ptr.is_null() {
            return MoltObject::none().bits();
        }
        for &entry in l_elems.iter() {
            let found = set_find_entry(_py, r_elems, r_hashes, r_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(in crate::object) unsafe fn set_like_symdiff(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
    unsafe {
        let l_elems = set_order(lhs_ptr);
        let r_elems = set_order(rhs_ptr);
        let l_hashes = set_hashes(lhs_ptr);
        let r_hashes = set_hashes(rhs_ptr);
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
            let found = set_find_entry(_py, r_elems, r_hashes, r_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        for &entry in r_elems.iter() {
            let found = set_find_entry(_py, l_elems, l_hashes, l_table, entry);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
            if found.is_none() {
                set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
                if exception_pending(_py) {
                    dec_ref_bits(_py, res_bits);
                    return MoltObject::none().bits();
                }
            }
        }
        res_bits
    }
}

pub(in crate::object) unsafe fn set_like_copy_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    result_type_id: u32,
) -> u64 {
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
            set_add_in_place(_py, res_ptr, entry, HashContext::SetElement);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return MoltObject::none().bits();
            }
        }
        res_bits
    }
}

/// Realize `other_bits` as a set-like pointer. When the argument is not already
/// a set/frozenset it is materialized into a temporary set, and `ctx` chooses
/// the unhashable-element error context for that materialization: probe-only
/// callers (`intersection`/`intersection_update`/`issubset`) pass
/// [`HashContext::Bare`]; all inserting callers pass [`HashContext::SetElement`].
pub(in crate::object) unsafe fn set_like_ptr_from_bits(
    _py: &PyToken<'_>,
    other_bits: u64,
    ctx: HashContext,
) -> Option<(*mut u8, Option<u64>)> {
    unsafe {
        let obj = obj_from_bits(other_bits);
        if let Some(ptr) = obj.as_ptr() {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_SET || type_id == TYPE_ID_FROZENSET {
                return Some((ptr, None));
            }
        }
        let set_bits = set_from_iter_bits(_py, other_bits, ctx)?;
        let ptr = obj_from_bits(set_bits).as_ptr()?;
        Some((ptr, Some(set_bits)))
    }
}

/// Materialize an iterable into a fresh set. `ctx` selects the
/// unhashable-element error context (see [`set_like_ptr_from_bits`]).
pub(in crate::object) unsafe fn set_from_iter_bits(
    _py: &PyToken<'_>,
    other_bits: u64,
    ctx: HashContext,
) -> Option<u64> {
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
            set_add_in_place(_py, set_ptr, val_bits, ctx);
            if exception_pending(_py) {
                dec_ref_bits(_py, set_bits);
                return None;
            }
        }
        Some(set_bits)
    }
}
