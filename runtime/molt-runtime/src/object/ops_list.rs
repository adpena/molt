//\! List, tuple, and bisect operations — extracted from ops.rs.

use super::ops::{eq_bool_from_bits, is_truthy};
use super::ops_arith::repeat_sequence;
use super::ops_compare::{
    CompareBoolOutcome, CompareOp, CompareOutcome, compare_builtin_bool, compare_objects,
    compare_type_error, rich_compare_bool,
};
use crate::*;
use molt_obj_model::MoltObject;
use num_traits::{Signed, ToPrimitive};
use std::cmp::Ordering;

struct SortItem {
    key_bits: u64,
    value_bits: u64,
}

enum SortError {
    NotComparable(u64, u64),
    Exception,
}

#[inline]
pub(crate) unsafe fn promote_specialized_list_to_list(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_LIST_INT => promote_list_int_to_list(_py, ptr),
            TYPE_ID_LIST_BOOL => promote_list_bool_to_list(_py, ptr),
            _ => {}
        }
    }
}

/// Promote a `TYPE_ID_LIST_INT` object to a regular `TYPE_ID_LIST` in-place.
///
/// Converts the compact i64 storage to a `Vec<u64>` of NaN-boxed ints and
/// rewrites the header type_id. After promotion, all standard list operations
/// work without specialized code paths.
pub(crate) unsafe fn promote_list_int_to_list(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        if object_type_id(ptr) != TYPE_ID_LIST_INT {
            return;
        }
        let int_storage_ptr = crate::object::layout::list_int_storage_ptr(ptr);
        if int_storage_ptr.is_null() {
            return;
        }
        let int_storage = *Box::from_raw(int_storage_ptr);
        let int_vec = int_storage.into_vec();
        let mut boxed_vec: Vec<u64> = Vec::with_capacity(int_vec.len());
        for raw in int_vec {
            boxed_vec.push(MoltObject::from_int(raw).bits());
        }
        let vec_ptr = Box::into_raw(Box::new(boxed_vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        let header = header_from_obj_ptr(ptr);
        (*header).type_id = TYPE_ID_LIST;
    }
}

/// Promote a `TYPE_ID_LIST_BOOL` object to a regular `TYPE_ID_LIST` in-place.
///
/// Converts the compact u8 storage to a `Vec<u64>` of NaN-boxed bools and
/// rewrites the header type_id. After promotion, all standard list operations
/// work without specialized code paths.
///
/// No-op if the object is not `TYPE_ID_LIST_BOOL`.
///
/// # Safety
/// Caller must hold the GIL.  `ptr` must point to a valid object data area.
pub(crate) unsafe fn promote_list_bool_to_list(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        if object_type_id(ptr) != TYPE_ID_LIST_BOOL {
            return;
        }
        let bool_storage_ptr = crate::object::layout::list_bool_storage_ptr(ptr);
        if bool_storage_ptr.is_null() {
            return;
        }
        let bool_storage = *Box::from_raw(bool_storage_ptr);
        let bool_vec = bool_storage.into_vec();
        // Convert u8 bools to NaN-boxed bools.
        let mut boxed_vec: Vec<u64> = Vec::with_capacity(bool_vec.len());
        for &b in &bool_vec {
            boxed_vec.push(MoltObject::from_bool(b != 0).bits());
        }
        drop(bool_vec);
        // Store the new Vec<u64> in the data area (same layout as TYPE_ID_LIST).
        let vec_ptr = Box::into_raw(Box::new(boxed_vec));
        *(ptr as *mut *mut Vec<u64>) = vec_ptr;
        // Rewrite the header type_id.
        let header = header_from_obj_ptr(ptr);
        (*header).type_id = TYPE_ID_LIST;
        // No HEADER_FLAG_CONTAINS_REFS needed — bools are not heap refs.
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_append(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(list_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                // Julia-inspired container monomorphization: if the list is
                // currently a specialized type and the appended value matches,
                // keep the compact representation instead of promoting to
                // the generic TYPE_ID_LIST. This preserves the specialized
                // layout for comprehension-built homogeneous lists that
                // accumulate elements one at a time.
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_LIST_INT {
                    let val_obj = obj_from_bits(val_bits);
                    if let Some(int_val) = val_obj.as_int() {
                        // Fast path: append directly to ListIntStorage.
                        // No NaN-boxing, no promotion, no IncRef (i64 is
                        // not a heap reference).
                        let storage = &mut *crate::object::layout::list_int_storage_ptr(ptr);
                        storage.push(int_val);
                        return MoltObject::none().bits();
                    }
                    // Value is not an int — fall through to promote + append.
                } else if tid == TYPE_ID_LIST_BOOL {
                    let val_obj = obj_from_bits(val_bits);
                    if let Some(bool_val) = val_obj.as_bool() {
                        // Fast path: append directly to ListBoolStorage.
                        // No NaN-boxing, no promotion, no IncRef (bools are
                        // inline NaN-boxed values, not heap references).
                        let storage = &mut *crate::object::layout::list_bool_storage_ptr(ptr);
                        storage.push(bool_val as u8);
                        return MoltObject::none().bits();
                    }
                    // Value is not a bool — fall through to promote + append.
                }
                promote_specialized_list_to_list(_py, ptr);
                if object_type_id(ptr) == TYPE_ID_LIST {
                    let elems = seq_vec(ptr);
                    elems.push(val_bits);
                    inc_ref_bits(_py, val_bits);
                    if crate::object::refcount_opt::is_heap_ref(val_bits) {
                        (*header_from_obj_ptr(ptr)).flags |=
                            crate::object::HEADER_FLAG_CONTAINS_REFS;
                    }
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
                promote_specialized_list_to_list(_py, ptr);
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
                promote_specialized_list_to_list(_py, list_ptr);
                if object_type_id(list_ptr) != TYPE_ID_LIST {
                    return MoltObject::none().bits();
                }
                let list_elems = seq_vec(list_ptr);
                let other_obj = obj_from_bits(other_bits);
                if let Some(other_ptr) = other_obj.as_ptr() {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_LIST || other_type == TYPE_ID_TUPLE {
                        // Inherit contains_refs from source container.
                        let src_has_refs = ((*header_from_obj_ptr(other_ptr)).flags
                            & crate::object::HEADER_FLAG_CONTAINS_REFS)
                            != 0;
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
                        if src_has_refs {
                            (*header_from_obj_ptr(list_ptr)).flags |=
                                crate::object::HEADER_FLAG_CONTAINS_REFS;
                        }
                        return MoltObject::none().bits();
                    }
                    if other_type == TYPE_ID_DICT {
                        let src_has_refs = ((*header_from_obj_ptr(other_ptr)).flags
                            & crate::object::HEADER_FLAG_CONTAINS_REFS)
                            != 0;
                        let order = dict_order(other_ptr);
                        for idx in (0..order.len()).step_by(2) {
                            let key_bits = order[idx];
                            list_elems.push(key_bits);
                            inc_ref_bits(_py, key_bits);
                        }
                        if src_has_refs {
                            (*header_from_obj_ptr(list_ptr)).flags |=
                                crate::object::HEADER_FLAG_CONTAINS_REFS;
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
                                    // Newly created tuples are always heap pointers.
                                    (*header_from_obj_ptr(list_ptr)).flags |=
                                        crate::object::HEADER_FLAG_CONTAINS_REFS;
                                } else {
                                    let item = if other_type == TYPE_ID_DICT_KEYS_VIEW {
                                        key_bits
                                    } else {
                                        val_bits
                                    };
                                    list_elems.push(item);
                                    inc_ref_bits(_py, item);
                                    if crate::object::refcount_opt::is_heap_ref(item) {
                                        (*header_from_obj_ptr(list_ptr)).flags |=
                                            crate::object::HEADER_FLAG_CONTAINS_REFS;
                                    }
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
                    if crate::object::refcount_opt::is_heap_ref(val_bits) {
                        (*header_from_obj_ptr(list_ptr)).flags |=
                            crate::object::HEADER_FLAG_CONTAINS_REFS;
                    }
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
                promote_specialized_list_to_list(_py, list_ptr);
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
                    if crate::object::refcount_opt::is_heap_ref(val_bits) {
                        (*header_from_obj_ptr(list_ptr)).flags |=
                            crate::object::HEADER_FLAG_CONTAINS_REFS;
                    }
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

pub(crate) unsafe fn list_elem_at(list_ptr: *mut u8, idx: usize) -> Option<u64> {
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
                promote_specialized_list_to_list(_py, list_ptr);
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
                promote_specialized_list_to_list(_py, list_ptr);
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
            let tid = object_type_id(list_ptr);
            if tid != TYPE_ID_LIST && tid != TYPE_ID_LIST_BOOL && tid != TYPE_ID_LIST_INT {
                // For TYPE_ID_OBJECT (user-defined subclasses), verify
                // the class actually inherits from list via MRO check.
                if tid == crate::object::TYPE_ID_OBJECT {
                    let val_type = crate::builtins::type_ops::type_of_bits(_py, list_bits);
                    let list_type = crate::builtins::classes::builtin_classes(_py).list;
                    if !crate::builtins::type_ops::issubclass_bits(val_type, list_type) {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "list.__init__ expects list",
                        );
                    }
                } else {
                    return raise_exception::<_>(_py, "TypeError", "list.__init__ expects list");
                }
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
                if object_type_id(list_ptr) == TYPE_ID_LIST_BOOL {
                    // Copy as a new ListBoolStorage (preserves compact representation).
                    let elems = crate::object::layout::list_bool_vec_ref(list_ptr);
                    let new_vec: Vec<u8> = elems.as_slice().to_vec();
                    let storage_ptr = crate::object::layout::ListBoolStorage::from_vec(new_vec);
                    let obj_size = std::mem::size_of::<crate::object::MoltHeader>()
                        + std::mem::size_of::<*mut crate::object::layout::ListBoolStorage>()
                        + std::mem::size_of::<u64>();
                    let out_ptr = alloc_object(_py, obj_size, TYPE_ID_LIST_BOOL);
                    if out_ptr.is_null() {
                        drop((*Box::from_raw(storage_ptr)).into_vec());
                        return MoltObject::none().bits();
                    }
                    *(out_ptr as *mut *mut crate::object::layout::ListBoolStorage) = storage_ptr;
                    return MoltObject::from_ptr(out_ptr).bits();
                }
                if object_type_id(list_ptr) == TYPE_ID_LIST_INT {
                    let elems = crate::object::layout::list_int_vec_ref(list_ptr);
                    let new_vec: Vec<i64> = elems.iter().copied().collect();
                    let storage_ptr = crate::object::layout::ListIntStorage::from_vec(new_vec);
                    let obj_size = std::mem::size_of::<crate::object::MoltHeader>()
                        + std::mem::size_of::<*mut crate::object::layout::ListIntStorage>()
                        + std::mem::size_of::<u64>();
                    let out_ptr = alloc_object(_py, obj_size, TYPE_ID_LIST_INT);
                    if out_ptr.is_null() {
                        drop((*Box::from_raw(storage_ptr)).into_vec());
                        return MoltObject::none().bits();
                    }
                    *(out_ptr as *mut *mut crate::object::layout::ListIntStorage) = storage_ptr;
                    return MoltObject::from_ptr(out_ptr).bits();
                }
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
                promote_specialized_list_to_list(_py, list_ptr);
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
                promote_specialized_list_to_list(_py, list_ptr);
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
            promote_specialized_list_to_list(_py, list_ptr);
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
            let other_tid = object_type_id(other_ptr);
            if other_tid == TYPE_ID_LIST_BOOL || other_tid == TYPE_ID_LIST_INT {
                promote_specialized_list_to_list(_py, other_ptr);
            }
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
            if object_type_id(list_ptr) != TYPE_ID_LIST
                && object_type_id(list_ptr) != TYPE_ID_LIST_BOOL
                && object_type_id(list_ptr) != TYPE_ID_LIST_INT
            {
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
                promote_specialized_list_to_list(_py, ptr);
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
                promote_specialized_list_to_list(_py, ptr);
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
