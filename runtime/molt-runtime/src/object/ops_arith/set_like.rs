use crate::*;
use crate::object::ops::{set_add_with_hash_in_place, set_find_entry_in_place_with_hash, set_pin_entry};
use molt_obj_model::MoltObject;

pub(in crate::object) fn set_like_result_type_id(type_id: u32) -> u32 {
    if type_id == TYPE_ID_FROZENSET { TYPE_ID_FROZENSET } else { TYPE_ID_SET }
}

unsafe fn set_like_new_bits(type_id: u32, capacity: usize) -> u64 {
    if type_id == TYPE_ID_FROZENSET { molt_frozenset_new(capacity as u64) }
    else { molt_set_new(capacity as u64) }
}

pub(in crate::object) unsafe fn set_like_copy_bits(py: &PyToken<'_>, source: *mut u8, kind: u32) -> u64 {
    unsafe {
        let bits = set_like_new_bits(kind, crate::builtins::containers::set_len(source));
        let Some(result) = obj_from_bits(bits).as_ptr() else { return MoltObject::none().bits(); };
        crate::object::ops::set_copy_into_empty(py, source, result);
        if exception_pending(py) { dec_ref_bits(py, bits); MoltObject::none().bits() } else { bits }
    }
}

pub(in crate::object) unsafe fn set_like_union(py: &PyToken<'_>, left: *mut u8, right: *mut u8, kind: u32) -> u64 {
    unsafe {
        let bits = set_like_copy_bits(py, left, kind);
        let Some(result) = obj_from_bits(bits).as_ptr() else { return MoltObject::none().bits(); };
        if left != right && crate::object::ops_set::set_update_iterable(
            py, result, MoltObject::from_ptr(right).bits(), HashContext::SetElement,
        ).is_err() { dec_ref_bits(py, bits); MoltObject::none().bits() } else { bits }
    }
}

pub(in crate::object) unsafe fn set_like_intersection(py: &PyToken<'_>, left: *mut u8, right: *mut u8, kind: u32) -> u64 {
    unsafe {
        if left == right { return set_like_copy_bits(py, left, kind); }
        let llen = crate::builtins::containers::set_len(left);
        let rlen = crate::builtins::containers::set_len(right);
        let bits = set_like_new_bits(kind, llen.min(rlen));
        let Some(result) = obj_from_bits(bits).as_ptr() else { return MoltObject::none().bits(); };
        let (source, probe) = if llen < rlen { (left, right) } else { (right, left) };
        let mut index = 0;
        while let Some(entry) = set_pin_entry(py, source, index) {
            let found = set_find_entry_in_place_with_hash(py, probe, entry.bits(), entry.hash());
            if !exception_pending(py) && found.is_some() {
                set_add_with_hash_in_place(py, result, entry.bits(), entry.hash());
            }
            if exception_pending(py) {
                dec_ref_bits(py, bits);
                drop(entry);
                return MoltObject::none().bits();
            }
            drop(entry);
            if exception_pending(py) { dec_ref_bits(py, bits); return MoltObject::none().bits(); }
            index += 1;
        }
        bits
    }
}

pub(in crate::object) unsafe fn set_like_difference(py: &PyToken<'_>, left: *mut u8, right: *mut u8, kind: u32) -> u64 {
    unsafe {
        let llen = crate::builtins::containers::set_len(left);
        if (llen >> 2) > crate::builtins::containers::set_len(right) {
            let bits = set_like_copy_bits(py, left, kind);
            let Some(result) = obj_from_bits(bits).as_ptr() else { return MoltObject::none().bits(); };
            if crate::object::ops_set::set_difference_update_iterable(py, result, MoltObject::from_ptr(right).bits()).is_err() {
                dec_ref_bits(py, bits);
                return MoltObject::none().bits();
            }
            return bits;
        }
        let bits = set_like_new_bits(kind, llen);
        let Some(result) = obj_from_bits(bits).as_ptr() else { return MoltObject::none().bits(); };
        let mut index = 0;
        while let Some(entry) = set_pin_entry(py, left, index) {
            let found = set_find_entry_in_place_with_hash(py, right, entry.bits(), entry.hash());
            if !exception_pending(py) && found.is_none() {
                set_add_with_hash_in_place(py, result, entry.bits(), entry.hash());
            }
            if exception_pending(py) {
                dec_ref_bits(py, bits);
                drop(entry);
                return MoltObject::none().bits();
            }
            drop(entry);
            if exception_pending(py) { dec_ref_bits(py, bits); return MoltObject::none().bits(); }
            index += 1;
        }
        bits
    }
}

pub(in crate::object) unsafe fn set_like_symdiff(py: &PyToken<'_>, left: *mut u8, right: *mut u8, kind: u32) -> u64 {
    unsafe {
        let bits = set_like_copy_bits(py, right, kind);
        let Some(result) = obj_from_bits(bits).as_ptr() else { return MoltObject::none().bits(); };
        if crate::object::ops_set::set_symdiff_update_iterable(py, result, MoltObject::from_ptr(left).bits()).is_err() {
            dec_ref_bits(py, bits);
            MoltObject::none().bits()
        } else { bits }
    }
}

/// Realize `other_bits` as a set-like pointer. When the argument is not already
/// a set/frozenset it is materialized into a temporary set. Callers use this
/// only at operation boundaries that require complete materialization.
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

/// Construction shares update's set/dict cached-hash and iterable boundaries.
pub(in crate::object) unsafe fn set_from_iter_bits(
    py: &PyToken<'_>, other_bits: u64, ctx: HashContext,
) -> Option<u64> {
    let bits = molt_set_new(0);
    let ptr = obj_from_bits(bits).as_ptr()?;
    if unsafe { crate::object::ops_set::set_update_iterable(py, ptr, other_bits, ctx) }.is_err()
        || exception_pending(py)
    { dec_ref_bits(py, bits); None } else { Some(bits) }
}
