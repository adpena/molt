//\! List, tuple, and bisect operations — extracted from ops.rs.

use super::ops::{eq_bool_from_bits, is_truthy};
use super::ops_arith::repeat_sequence;
use super::ops_compare::{CompareBoolOutcome, CompareOp, rich_compare_op_bool};
use crate::*;
use molt_obj_model::MoltObject;

mod timsort;

/// Physical list storage is the receiver contract for every list descriptor.
/// A Python MRO alone cannot make a TYPE_ID_OBJECT payload safe to reinterpret.
pub(crate) fn list_storage_ptr(bits: u64) -> Option<*mut u8> {
    obj_from_bits(bits).as_ptr().filter(|&ptr| unsafe {
        matches!(
            object_type_id(ptr),
            TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL
        )
    })
}

pub(crate) fn list_receiver(py: &PyToken<'_>, bits: u64, method: &str) -> Option<*mut u8> {
    if let Some(ptr) = list_storage_ptr(bits) {
        return Some(ptr);
    }
    let received = type_name(py, obj_from_bits(bits));
    raise_exception::<()>(
        py,
        "TypeError",
        &format!("descriptor '{method}' requires a 'list' object but received a '{received}'"),
    );
    None
}

#[derive(Clone, Copy)]
struct SortItem {
    key_bits: u64,
    value_bits: u64,
    original_index: usize,
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

/// Commit the completed generic backing and its collector lifetime together.
/// Both scalar representations reach this boundary only after conversion has
/// succeeded, before any reference-bearing list mutation or C view publication.
unsafe fn commit_specialized_list_promotion(
    py: &PyToken<'_>,
    ptr: *mut u8,
    backing: *mut Vec<u64>,
) {
    unsafe {
        *(ptr as *mut *mut Vec<u64>) = backing;
        (*header_from_obj_ptr(ptr)).type_id = TYPE_ID_LIST;
        super::gc::gc_track_if_cyclic(py, ptr, TYPE_ID_LIST);
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
        let int_storage_ref = &*int_storage_ptr;
        let int_slice = std::slice::from_raw_parts(int_storage_ref.data, int_storage_ref.len);
        let Some(vec_ptr) =
            crate::object::backing::tracked_vec_box_with_capacity::<u64>(int_slice.len())
        else {
            let _ = raise_exception::<u64>(_py, "MemoryError", "list allocation failed");
            return;
        };
        let boxed_vec = &mut *vec_ptr;
        for &raw in int_slice {
            boxed_vec.push(MoltObject::from_int(raw).bits());
        }
        let int_storage = *Box::from_raw(int_storage_ptr);
        drop(int_storage.into_vec());
        commit_specialized_list_promotion(_py, ptr, vec_ptr);
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
        let bool_storage_ref = &*bool_storage_ptr;
        let bool_slice = std::slice::from_raw_parts(bool_storage_ref.data, bool_storage_ref.len);
        let Some(vec_ptr) =
            crate::object::backing::tracked_vec_box_with_capacity::<u64>(bool_slice.len())
        else {
            let _ = raise_exception::<u64>(_py, "MemoryError", "list allocation failed");
            return;
        };
        let boxed_vec = &mut *vec_ptr;
        for &b in bool_slice {
            boxed_vec.push(MoltObject::from_bool(b != 0).bits());
        }
        let bool_storage = *Box::from_raw(bool_storage_ptr);
        drop(bool_storage.into_vec());
        commit_specialized_list_promotion(_py, ptr, vec_ptr);
        // No HEADER_FLAG_CONTAINS_REFS needed — bools are not heap refs.
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_append(list_bits: u64, val_bits: u64) -> u64 {
    let _ = molt_list_append_with_projection(list_bits, val_bits, std::ptr::null_mut());
    MoltObject::none().bits()
}

pub(crate) fn molt_list_append_with_projection(
    list_bits: u64,
    val_bits: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> bool {
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "append").is_none() {
            return false;
        }
        let obj = obj_from_bits(list_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                // Julia-inspired container monomorphization: if the list is
                // currently a specialized type and the appended value matches,
                // keep the compact representation instead of promoting to
                // the generic TYPE_ID_LIST. This preserves the specialized
                // layout for comprehension-built homogeneous lists that
                // accumulate elements one at a time.
                let mut tid = object_type_id(ptr);
                let has_abi_view = (*header_from_obj_ptr(ptr)).load_synchronized_flags()
                    & crate::object::HEADER_FLAG_HAS_ABI_VIEW
                    != 0;
                if matches!(tid, TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL)
                    && (has_abi_view || !item_ptr.is_null())
                {
                    promote_specialized_list_to_list(_py, ptr);
                    if exception_pending(_py) {
                        return false;
                    }
                    tid = object_type_id(ptr);
                }
                if tid == TYPE_ID_LIST_INT {
                    if let Some(int_val) = crate::object::layout::InlineListInt::from_bits(val_bits)
                    {
                        // Fast path: append directly to ListIntStorage.
                        // No NaN-boxing, no promotion, no IncRef (i64 is
                        // not a heap reference).
                        let storage = &mut *crate::object::layout::list_int_storage_ptr(ptr);
                        if !storage.push(int_val) {
                            return raise_exception::<_>(
                                _py,
                                "MemoryError",
                                "list allocation failed",
                            );
                        }
                        return true;
                    }
                    // Value is not an int — fall through to promote + append.
                } else if tid == TYPE_ID_LIST_BOOL {
                    let val_obj = obj_from_bits(val_bits);
                    if let Some(bool_val) = val_obj.as_bool() {
                        // Fast path: append directly to ListBoolStorage.
                        // No NaN-boxing, no promotion, no IncRef (bools are
                        // inline NaN-boxed values, not heap references).
                        let storage = &mut *crate::object::layout::list_bool_storage_ptr(ptr);
                        if !storage.push(bool_val as u8) {
                            return raise_exception::<_>(
                                _py,
                                "MemoryError",
                                "list allocation failed",
                            );
                        }
                        return true;
                    }
                    // Value is not a bool — fall through to promote + append.
                }
                promote_specialized_list_to_list(_py, ptr);
                if object_type_id(ptr) == TYPE_ID_LIST {
                    if !crate::object::list_mutation::append_with_projection(
                        _py, ptr, val_bits, item_ptr,
                    ) {
                        return false;
                    }
                    return true;
                }
            }
        }
        false
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_pop(list_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "pop").is_none() {
            return MoltObject::none().bits();
        }
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
                        // CPython raises "'<type>' object cannot be interpreted as
                        // an integer" (version-stable across 3.12/3.13/3.14) when
                        // the pop index lacks a usable __index__.
                        let pop_idx_msg = format!(
                            "'{}' object cannot be interpreted as an integer",
                            type_name(_py, index_obj)
                        );
                        index_i64_from_obj(_py, index_bits, &pop_idx_msg)
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
                    return crate::object::list_mutation::pop(_py, ptr, idx as usize)
                        .unwrap_or_else(|| MoltObject::none().bits());
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_extend(list_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "extend").is_none() {
            return MoltObject::none().bits();
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                promote_specialized_list_to_list(_py, list_ptr);
                if object_type_id(list_ptr) != TYPE_ID_LIST {
                    return MoltObject::none().bits();
                }
                let other_obj = obj_from_bits(other_bits);
                if let Some(other_ptr) = other_obj.as_ptr()
                    && crate::object::iterable::builtin_receiver(_py, other_ptr)
                {
                    let other_type = object_type_id(other_ptr);
                    if other_type == TYPE_ID_LIST || other_type == TYPE_ID_TUPLE {
                        let Some(snapshot) = crate::object::seq_access::snapshot(
                            _py,
                            other_ptr,
                            "list extension snapshot allocation failed",
                        ) else {
                            return MoltObject::none().bits();
                        };
                        let extended = crate::object::list_mutation::extend_from_slice(
                            _py, list_ptr, &snapshot,
                        );
                        if !extended {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::none().bits();
                    }
                    if other_type == TYPE_ID_DICT
                        || other_type == TYPE_ID_DICT_KEYS_VIEW
                        || other_type == TYPE_ID_DICT_VALUES_VIEW
                        || other_type == TYPE_ID_DICT_ITEMS_VIEW
                    {
                        use super::ops_dict::{DictSnapshotKind, dict_snapshot};
                        let dict = if other_type == TYPE_ID_DICT {
                            other_ptr
                        } else {
                            obj_from_bits(dict_view_dict_bits(other_ptr))
                                .as_ptr()
                                .unwrap()
                        };
                        let kind = match other_type {
                            TYPE_ID_DICT_VALUES_VIEW => DictSnapshotKind::Values,
                            TYPE_ID_DICT_ITEMS_VIEW => DictSnapshotKind::Items,
                            _ => DictSnapshotKind::Keys,
                        };
                        let Some(snapshot) = dict_snapshot(_py, dict, kind) else {
                            return MoltObject::none().bits();
                        };
                        let extended = crate::object::list_mutation::extend_from_slice(
                            _py, list_ptr, &snapshot,
                        );
                        if !extended {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::none().bits();
                    }
                }
                let Some(mut iter) = crate::object::iterable::OwnedIterator::new(_py, other_bits)
                else {
                    return MoltObject::none().bits();
                };
                let Some(hint) = crate::object::iterable::length_hint(_py, other_bits) else {
                    return MoltObject::none().bits();
                };
                if !crate::object::list_mutation::reserve_additional(_py, list_ptr, hint) {
                    return MoltObject::none().bits();
                }
                loop {
                    match iter.next() {
                        Ok(Some(item)) => {
                            let extended = crate::object::list_mutation::extend_from_slice(
                                _py,
                                list_ptr,
                                std::slice::from_ref(&item),
                            );
                            dec_ref_bits(_py, item);
                            if !extended {
                                return MoltObject::none().bits();
                            }
                        }
                        Ok(None) => break,
                        Err(molt_runtime_core::ErrorIndicatorSet) => {
                            return MoltObject::none().bits();
                        }
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
    let _ = molt_list_insert_with_projection(list_bits, index_bits, val_bits, std::ptr::null_mut());
    MoltObject::none().bits()
}

pub(crate) fn molt_list_insert_with_projection(
    list_bits: u64,
    index_bits: u64,
    val_bits: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> bool {
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "insert").is_none() {
            return false;
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                promote_specialized_list_to_list(_py, list_ptr);
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    // CPython raises "'<type>' object cannot be interpreted as an
                    // integer" (version-stable across 3.12/3.13/3.14) when the
                    // insert index lacks a usable __index__.
                    let insert_idx_msg = format!(
                        "'{}' object cannot be interpreted as an integer",
                        type_name(_py, obj_from_bits(index_bits))
                    );
                    let idx = index_i64_from_obj(_py, index_bits, &insert_idx_msg);
                    if exception_pending(_py) {
                        return false;
                    }
                    return insert_at_native_index_with_projection(
                        _py, list_ptr, idx, val_bits, item_ptr,
                    );
                }
            }
        }
        false
    })
}

/// Insert an already-native signed index without boxing it into Molt's compact
/// integer representation. CPython accepts the complete Py_ssize_t domain and
/// clamps extreme values; the magnitude formulation avoids signed overflow for
/// `isize::MIN`/`i64::MIN`.
pub(crate) unsafe fn insert_at_native_index_with_projection(
    py: &PyToken<'_>,
    list_ptr: *mut u8,
    index: i64,
    val_bits: u64,
    item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> bool {
    if list_ptr.is_null() || unsafe { object_type_id(list_ptr) } != TYPE_ID_LIST {
        return false;
    }
    let len = unsafe { list_len(list_ptr) };
    let index = if index < 0 {
        let magnitude = index.unsigned_abs();
        if magnitude >= len as u64 {
            0
        } else {
            len - magnitude as usize
        }
    } else {
        (index as u64).min(len as u64) as usize
    };
    unsafe {
        crate::object::list_mutation::insert_with_projection(
            py, list_ptr, index, val_bits, item_ptr,
        )
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_remove(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "remove").is_none() {
            return MoltObject::none().bits();
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                promote_specialized_list_to_list(_py, list_ptr);
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    let mut idx = 0usize;
                    loop {
                        let len = list_len(list_ptr);
                        if idx >= len {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "list.remove(x): x not in list",
                            );
                        }
                        let Some(elem) = crate::object::seq_access::pin_item(_py, list_ptr, idx)
                        else {
                            return MoltObject::none().bits();
                        };
                        let elem_bits = elem.bits();
                        let eq = match eq_bool_from_bits(_py, elem_bits, val_bits) {
                            Some(val) => val,
                            None => return MoltObject::none().bits(),
                        };
                        drop(elem);
                        if eq {
                            let live_len = list_len(list_ptr);
                            let low = idx.min(live_len);
                            let high = idx.saturating_add(1).min(live_len);
                            if !crate::object::list_mutation::replace_range(
                                _py,
                                list_ptr,
                                low,
                                high,
                                &[],
                            ) {
                                return MoltObject::none().bits();
                            }
                            return MoltObject::none().bits();
                        }
                        idx = idx.saturating_add(1);
                    }
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_clear(list_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "clear").is_none() {
            return MoltObject::none().bits();
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                promote_specialized_list_to_list(_py, list_ptr);
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    if !crate::object::list_mutation::clear(_py, list_ptr) {
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
pub extern "C" fn molt_list_init_method(list_bits: u64, iterable_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "__init__").is_none() {
            return MoltObject::none().bits();
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
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "copy").is_none() {
            return MoltObject::none().bits();
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                if object_type_id(list_ptr) == TYPE_ID_LIST_BOOL {
                    // Copy as a new ListBoolStorage (preserves compact representation).
                    let elems = crate::object::layout::list_bool_vec_ref(list_ptr);
                    return match crate::object::builders::alloc_list_bool_from_raw_slice(
                        _py,
                        elems.as_slice(),
                    ) {
                        Ok(out_ptr) => MoltObject::from_ptr(out_ptr).bits(),
                        Err(bits) => bits,
                    };
                }
                if object_type_id(list_ptr) == TYPE_ID_LIST_INT {
                    let elems = crate::object::layout::list_int_vec_ref(list_ptr);
                    return match crate::object::builders::alloc_list_int_from_raw_slice(
                        _py,
                        elems.as_slice(),
                    ) {
                        Ok(out_ptr) => MoltObject::from_ptr(out_ptr).bits(),
                        Err(bits) => bits,
                    };
                }
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    let Some(elems) = crate::object::seq_access::snapshot(
                        _py,
                        list_ptr,
                        "list copy snapshot allocation failed",
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let out_ptr = alloc_list(_py, &elems);
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
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "reverse").is_none() {
            return MoltObject::none().bits();
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                promote_specialized_list_to_list(_py, list_ptr);
                if object_type_id(list_ptr) == TYPE_ID_LIST {
                    if !crate::object::list_mutation::reverse(_py, list_ptr) {
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
pub extern "C" fn molt_list_sort(list_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "sort").is_none() {
            return MoltObject::none().bits();
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(list_ptr) = list_obj.as_ptr() {
            unsafe {
                promote_specialized_list_to_list(_py, list_ptr);
                if object_type_id(list_ptr) != TYPE_ID_LIST {
                    return MoltObject::none().bits();
                }
                let use_key = !obj_from_bits(key_bits).is_none();
                let reverse = is_truthy(_py, obj_from_bits(reverse_bits));
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                let len = list_len(list_ptr);
                let mut items: Vec<SortItem> = Vec::new();
                let mut ordered_values: Vec<u64> = Vec::new();
                if items.try_reserve_exact(len).is_err()
                    || ordered_values.try_reserve_exact(len).is_err()
                {
                    return raise_exception::<_>(_py, "MemoryError", "list sort allocation failed");
                }
                let Some(sort_txn) =
                    crate::object::list_mutation::ListSortTxn::begin(_py, list_ptr)
                else {
                    return MoltObject::none().bits();
                };
                // Keys are computed in list order while the list is published
                // empty, as CPython's `list.sort` computes them.
                for (original_index, &val_bits) in sort_txn.values().iter().enumerate() {
                    let key_val_bits = if use_key {
                        let res_bits = call_callable1(_py, key_bits, val_bits);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, res_bits);
                            // CPython releases the computed keys last to first.
                            for item in items.drain(..).rev() {
                                dec_ref_bits(_py, item.key_bits);
                            }
                            ordered_values.extend_from_slice(sort_txn.values());
                            let _ = sort_txn.finish(&ordered_values, 0..len);
                            return MoltObject::none().bits();
                        }
                        res_bits
                    } else {
                        val_bits
                    };
                    items.push(SortItem {
                        key_bits: key_val_bits,
                        value_bits: val_bits,
                        original_index,
                    });
                }
                // A stable descending sort, as CPython makes it: reverse, sort
                // ascending, reverse back.
                if reverse {
                    items.reverse();
                }
                // Run detection changed in CPython 3.13, and with it the
                // comparisons a sort makes.
                let runs = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
                    timsort::RunDetection::WeaklyDescending
                } else {
                    timsort::RunDetection::StrictlyDescending
                };
                let sorted = timsort::timsort(&mut items, runs, |left, right| {
                    match rich_compare_op_bool(
                        _py,
                        obj_from_bits(left.key_bits),
                        obj_from_bits(right.key_bits),
                        CompareOp::Lt,
                    ) {
                        CompareBoolOutcome::True => Ok(true),
                        CompareBoolOutcome::False => Ok(false),
                        CompareBoolOutcome::NotComparable | CompareBoolOutcome::Error => Err(()),
                    }
                });
                if use_key {
                    // In their sorted order, before the result is reversed back.
                    for item in &items {
                        dec_ref_bits(_py, item.key_bits);
                    }
                }
                if reverse {
                    items.reverse();
                }
                ordered_values.extend(items.iter().map(|item| item.value_bits));
                let mutated = sort_txn
                    .finish(
                        &ordered_values,
                        items.iter().map(|item| item.original_index),
                    )
                    .unwrap_or(false);
                match sorted {
                    // A failed comparison's exception is pending, and the list
                    // keeps the permutation the sort had reached.
                    Err(timsort::TimsortError::Compare) => {}
                    Err(timsort::TimsortError::Memory) => {
                        return raise_exception::<_>(
                            _py,
                            "MemoryError",
                            "list sort allocation failed",
                        );
                    }
                    Ok(()) if mutated => {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "list modified during sort",
                        );
                    }
                    Ok(()) => {}
                }
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_add_method(list_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(list_ptr) = list_receiver(_py, list_bits, "__add__") else {
            return MoltObject::none().bits();
        };
        unsafe {
            promote_specialized_list_to_list(_py, list_ptr);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let other_obj = obj_from_bits(other_bits);
            let Some(other_ptr) = other_obj.as_ptr() else {
                return super::ops_arith::native_slots::SequenceConcatKind::List
                    .raise(_py, list_bits, other_bits);
            };
            let other_tid = object_type_id(other_ptr);
            if other_tid == TYPE_ID_LIST_BOOL || other_tid == TYPE_ID_LIST_INT {
                promote_specialized_list_to_list(_py, other_ptr);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
            if object_type_id(other_ptr) != TYPE_ID_LIST {
                return super::ops_arith::native_slots::SequenceConcatKind::List
                    .raise(_py, list_bits, other_bits);
            }
            let Some(combined) = crate::object::seq_access::snapshot_concat(
                _py,
                list_ptr,
                other_ptr,
                "list concatenation allocation failed",
            ) else {
                return MoltObject::none().bits();
            };
            let ptr = alloc_list(_py, &combined);
            if ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[derive(Clone, Copy)]
enum ListRepeatMode {
    Copy,
    InPlace,
}

fn list_repeat_method(
    py: &PyToken<'_>,
    list_bits: u64,
    count_bits: u64,
    mode: ListRepeatMode,
) -> u64 {
    let method = match mode {
        ListRepeatMode::Copy => "__mul__",
        ListRepeatMode::InPlace => "__imul__",
    };
    let Some(list_ptr) = list_receiver(py, list_bits, method) else {
        return MoltObject::none().bits();
    };
    // Explicit sequence descriptors use the repeat slot directly. Reflected
    // numeric dispatch belongs to the source operator, never this method call.
    let Some(count) = super::ops_arith::sequence_repeat_count(py, count_bits) else {
        return MoltObject::none().bits();
    };
    let result = match mode {
        ListRepeatMode::Copy => repeat_sequence(py, list_ptr, count),
        ListRepeatMode::InPlace => super::ops_arith::repeat_sequence_in_place(py, list_ptr, count),
    };
    result.unwrap_or_else(|| MoltObject::none().bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_mul_method(list_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        list_repeat_method(_py, list_bits, count_bits, ListRepeatMode::Copy)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_imul_method(list_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        list_repeat_method(_py, list_bits, count_bits, ListRepeatMode::InPlace)
    })
}

pub(crate) extern "C" fn list_iadd_slot(list_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if list_receiver(py, list_bits, "__iadd__").is_none() {
            return MoltObject::none().bits();
        }
        let _ = molt_list_extend(list_bits, other_bits);
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        inc_ref_bits(py, list_bits);
        list_bits
    })
}

pub(crate) extern "C" fn list_getitem_slot(list_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if list_receiver(py, list_bits, "__getitem__").is_none() {
            return MoltObject::none().bits();
        }
        crate::object::ops::molt_getitem_builtin(list_bits, key_bits)
    })
}

pub(crate) extern "C" fn list_setitem_slot(list_bits: u64, key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if list_receiver(py, list_bits, "__setitem__").is_none() {
            return MoltObject::none().bits();
        }
        let _ = crate::object::ops::molt_setitem_builtin(list_bits, key_bits, value_bits);
        MoltObject::none().bits()
    })
}

pub(crate) extern "C" fn list_delitem_slot(list_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if list_receiver(py, list_bits, "__delitem__").is_none() {
            return MoltObject::none().bits();
        }
        let _ = crate::object::ops::molt_delitem_builtin(list_bits, key_bits);
        MoltObject::none().bits()
    })
}

pub(crate) extern "C" fn list_iter_slot(list_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if list_receiver(py, list_bits, "__iter__").is_none() {
            return MoltObject::none().bits();
        }
        crate::object::ops_iter::builtin_iter_slot(list_bits)
    })
}

pub(crate) extern "C" fn list_len_slot(list_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if list_receiver(py, list_bits, "__len__").is_none() {
            return MoltObject::none().bits();
        }
        crate::object::ops_sys::molt_len_builtin(list_bits)
    })
}

pub(crate) extern "C" fn list_contains_slot(list_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if list_receiver(py, list_bits, "__contains__").is_none() {
            return MoltObject::none().bits();
        }
        crate::object::ops::molt_contains_builtin(list_bits, item_bits)
    })
}

pub(crate) extern "C" fn list_reversed_slot(list_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if list_receiver(py, list_bits, "__reversed__").is_none() {
            return MoltObject::none().bits();
        }
        crate::object::ops_iter::builtin_reversed_slot(list_bits)
    })
}

// heapq operations moved to ops_heapq.rs

fn bisect_len_from_obj(py: &PyToken<'_>, obj: MoltObject) -> Option<i64> {
    let result = crate::object::ops_sys::molt_len(obj.bits());
    if exception_pending(py) {
        dec_ref_bits(py, result);
        return None;
    }
    // molt_len owns method lookup, result coercion and the target ssize bound.
    let length = to_i64(obj_from_bits(result));
    dec_ref_bits(py, result);
    length
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
                let mut bits = 0;
                if crate::object::seq_access::read_item_owned(ptr, idx as usize, &mut bits) == 0 {
                    raise_exception::<()>(_py, "IndexError", "list index out of range");
                    return None;
                }
                return Some((bits, true));
            }
            if type_id == TYPE_ID_TUPLE {
                if idx < 0 {
                    raise_exception::<()>(_py, "IndexError", "tuple index out of range");
                    return None;
                }
                let Some(bits) = crate::object::seq_access::item(ptr, idx as usize) else {
                    raise_exception::<()>(_py, "IndexError", "tuple index out of range");
                    return None;
                };
                return Some((bits, false));
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
    match rich_compare_op_bool(
        _py,
        obj_from_bits(lhs_bits),
        obj_from_bits(rhs_bits),
        CompareOp::Lt,
    ) {
        CompareBoolOutcome::True => Some(true),
        CompareBoolOutcome::False => Some(false),
        CompareBoolOutcome::NotComparable | CompareBoolOutcome::Error => None,
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
            if object_type_id(ptr) == TYPE_ID_LIST
                && crate::object::iterable::builtin_receiver(_py, ptr)
            {
                let len = list_len(ptr) as i64;
                let mut pos = idx;
                if pos < 0 {
                    pos = 0;
                }
                if pos > len {
                    pos = len;
                }
                return crate::object::list_mutation::insert(_py, ptr, pos as usize, value_bits)
                    .then_some(());
            }
            if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"insert") {
                if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                    dec_ref_bits(_py, name_bits);
                    let idx_bits = int_bits_from_i64(_py, idx);
                    crate::call::discard_owned_call_result(
                        _py,
                        call_callable2(_py, call_bits, idx_bits, value_bits),
                    );
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "count").is_none() {
            return MoltObject::none().bits();
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(ptr) = list_obj.as_ptr() {
            unsafe {
                promote_specialized_list_to_list(_py, ptr);
                if object_type_id(ptr) == TYPE_ID_LIST {
                    let mut count = 0i64;
                    let mut idx = 0usize;
                    while let Some(elem) = crate::object::seq_access::pin_item(_py, ptr, idx) {
                        let elem_bits = elem.bits();
                        let eq = match eq_bool_from_bits(_py, elem_bits, val_bits) {
                            Some(val) => val,
                            None => return MoltObject::none().bits(),
                        };
                        drop(elem);
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
    crate::with_gil_entry_nopanic!(_py, {
        if list_receiver(_py, list_bits, "index").is_none() {
            return MoltObject::none().bits();
        }
        let list_obj = obj_from_bits(list_bits);
        if let Some(ptr) = list_obj.as_ptr() {
            unsafe {
                promote_specialized_list_to_list(_py, ptr);
                if object_type_id(ptr) == TYPE_ID_LIST {
                    let missing = missing_bits(_py);
                    let err = "slice indices must be integers or have an __index__ method";
                    let mut start = if start_bits == missing {
                        0
                    } else {
                        let Some(value) = crate::builtins::numbers::index_ssize_clamped_from_obj(
                            _py, start_bits, err,
                        ) else {
                            return MoltObject::none().bits();
                        };
                        value as i64
                    };
                    let mut stop = if stop_bits == missing {
                        isize::MAX as i64
                    } else {
                        let Some(value) = crate::builtins::numbers::index_ssize_clamped_from_obj(
                            _py, stop_bits, err,
                        ) else {
                            return MoltObject::none().bits();
                        };
                        value as i64
                    };
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    // Index conversion may itself mutate the list. Normalize
                    // negative bounds against its current size, then retain
                    // the requested stop: equality may append a later match.
                    let len = list_len(ptr) as i64;
                    if start < 0 {
                        start = (start + len).max(0);
                    }
                    if stop < 0 {
                        stop = (stop + len).max(0);
                    }
                    if start < stop {
                        let mut idx = start;
                        while idx < stop {
                            let elem =
                                match crate::object::seq_access::pin_item(_py, ptr, idx as usize) {
                                    Some(elem) => elem,
                                    None => break,
                                };
                            let elem_bits = elem.bits();
                            let eq = match eq_bool_from_bits(_py, elem_bits, val_bits) {
                                Some(val) => val,
                                None => return MoltObject::none().bits(),
                            };
                            drop(elem);
                            if eq {
                                return MoltObject::from_int(idx).bits();
                            }
                            idx += 1;
                        }
                    }
                    // CPython 3.12/3.13 raise ValueError("<repr(x)> is not in
                    // list") and propagate any exception x.__repr__ raises; 3.14
                    // reverted to the static "list.index(x): x not in list" and
                    // does not call __repr__.
                    if crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "list.index(x): x not in list",
                        );
                    }
                    let repr_bits = crate::molt_repr_from_obj(val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    let rendered =
                        crate::object::ops_format::string_obj_to_owned(obj_from_bits(repr_bits))
                            .unwrap_or_default();
                    dec_ref_bits(_py, repr_bits);
                    let msg = format!("{rendered} is not in list");
                    return raise_exception::<_>(_py, "ValueError", &msg);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_index(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let missing = missing_bits(_py);
        molt_list_index_range(list_bits, val_bits, missing, missing)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_count(tuple_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(tuple) =
            crate::object::tuple_storage::TupleStorage::admit(py, tuple_bits, "count")
        else {
            return MoltObject::none().bits();
        };
        let Some(len) = tuple.len() else {
            return MoltObject::none().bits();
        };
        let mut count = 0i64;
        for index in 0..len {
            let Some(item) = tuple.comparison_item(index) else {
                return MoltObject::none().bits();
            };
            let equal = unsafe { eq_bool_from_bits(py, item.bits(), val_bits) };
            drop(item);
            let Some(equal) = equal else {
                return MoltObject::none().bits();
            };
            count += i64::from(equal);
        }
        int_bits_from_i64(py, count)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_tuple_index(tuple_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(py, {
        let Some(tuple) =
            crate::object::tuple_storage::TupleStorage::admit(py, tuple_bits, "index")
        else {
            return MoltObject::none().bits();
        };
        let Some(len) = tuple.len() else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(py);
        let bound = |bits, default| {
            if bits == missing {
                Some(default)
            } else {
                crate::builtins::numbers::index_ssize_clamped_from_obj(
                    py,
                    bits,
                    "slice indices must be integers or have an __index__ method",
                )
                .map(|value| {
                    let value = if value < 0 {
                        value.saturating_add(len as isize)
                    } else {
                        value
                    };
                    value.clamp(0, len as isize) as usize
                })
            }
        };
        let Some(start) = bound(start_bits, 0) else {
            return MoltObject::none().bits();
        };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let Some(stop) = bound(stop_bits, len) else {
            return MoltObject::none().bits();
        };
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        for index in start..stop {
            let Some(item) = tuple.comparison_item(index) else {
                return MoltObject::none().bits();
            };
            let equal = unsafe { eq_bool_from_bits(py, item.bits(), val_bits) };
            drop(item);
            let Some(equal) = equal else {
                return MoltObject::none().bits();
            };
            if equal {
                return int_bits_from_i64(py, index as i64);
            }
        }
        raise_exception(py, "ValueError", "tuple.index(x): x not in tuple")
    })
}
