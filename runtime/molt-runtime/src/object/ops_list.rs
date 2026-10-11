//\! List, tuple, and bisect operations — extracted from ops.rs.

use super::ops::{eq_bool_from_bits, is_truthy};
use super::ops_arith::repeat_sequence;
use super::ops_compare::{CompareBoolOutcome, CompareOp, rich_compare_op_bool};
use crate::*;
use molt_obj_model::MoltObject;

use super::list_storage::ListStorage;
use molt_obj_model::timsort;

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

pub(crate) fn list_receiver<'a, 'py>(
    py: &'a PyToken<'py>,
    bits: u64,
    method: &str,
) -> Option<ListStorage<'a, 'py>> {
    ListStorage::admit(py, bits, method)
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
            crate::record_memory_error_without_allocation(_py);
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
            crate::record_memory_error_without_allocation(_py);
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
        let Some(storage) = list_receiver(_py, list_bits, "append") else {
            return false;
        };
        if storage.native().is_some() {
            return storage.append(val_bits);
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
                    tid = object_type_id(ptr);
                    if tid != TYPE_ID_LIST {
                        return false;
                    }
                }
                if tid == TYPE_ID_LIST_INT {
                    if let Some(int_val) = crate::object::layout::InlineListInt::from_bits(val_bits)
                    {
                        // Fast path: append directly to ListIntStorage.
                        // No NaN-boxing, no promotion, no IncRef (i64 is
                        // not a heap reference).
                        let storage = &mut *crate::object::layout::list_int_storage_ptr(ptr);
                        if !storage.push(int_val) {
                            return {
                                crate::record_memory_error_without_allocation(_py);
                                crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
                            };
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
                            return {
                                crate::record_memory_error_without_allocation(_py);
                                crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
                            };
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
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = list_receiver(py, list_bits, "pop") else {
            return MoltObject::none().bits();
        };
        let mut index = index_ssize_from_obj(py, index_bits);
        if exception_pending(py) || !storage.promote() {
            return MoltObject::none().bits();
        }
        let Some(len) = storage.len() else {
            return MoltObject::none().bits();
        };
        if len == 0 {
            return raise_exception(py, "IndexError", "pop from empty list");
        }
        if index < 0 {
            index += len as i64;
        }
        if index < 0 || index as usize >= len {
            return raise_exception(py, "IndexError", "pop index out of range");
        }
        storage.pop(index as isize)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_extend(list_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(storage) = list_receiver(_py, list_bits, "extend") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() {
            let _ = storage.extend(other_bits);
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
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = list_receiver(py, list_bits, "insert") else {
            return false;
        };
        let index = index_ssize_from_obj(py, index_bits);
        if exception_pending(py) || !storage.promote() {
            return false;
        }
        if let Some(list_ptr) = storage.managed() {
            unsafe {
                insert_at_native_index_with_projection(py, list_ptr, index, val_bits, item_ptr)
            }
        } else {
            storage.insert(index as isize, val_bits)
        }
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
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = list_receiver(py, list_bits, "remove") else {
            return MoltObject::none().bits();
        };
        if !storage.promote() {
            return MoltObject::none().bits();
        }
        let mut index = 0;
        while let Some(item) = storage.item(index) {
            let equal = unsafe { eq_bool_from_bits(py, item.bits(), val_bits) };
            drop(item);
            let Some(equal) = equal else {
                return MoltObject::none().bits();
            };
            if equal {
                let _ = storage.remove_at(index);
                return MoltObject::none().bits();
            }
            index += 1;
        }
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        raise_exception(py, "ValueError", "list.remove(x): x not in list")
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_clear(list_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(storage) = list_receiver(_py, list_bits, "clear") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() {
            let _ = storage.clear();
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
        let Some(storage) = list_receiver(_py, list_bits, "copy") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() {
            return storage.copy();
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
    crate::with_gil_entry_nopanic!(py, {
        let _ = reverse_in_place(py, list_bits);
        MoltObject::none().bits()
    })
}

pub(crate) fn reverse_in_place(py: &PyToken<'_>, list_bits: u64) -> bool {
    let Some(storage) = list_receiver(py, list_bits, "reverse") else {
        return false;
    };
    if !storage.promote() {
        return false;
    }
    if let Some(ptr) = storage.managed() {
        unsafe { crate::object::list_mutation::reverse(py, ptr) }
    } else {
        storage.reverse()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_sort(list_bits: u64, key_bits: u64, reverse_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = list_receiver(py, list_bits, "sort") else {
            return MoltObject::none().bits();
        };
        let reverse = is_truthy(py, obj_from_bits(reverse_bits));
        if !exception_pending(py) {
            let _ = sort_storage(py, storage, key_bits, reverse);
        }
        MoltObject::none().bits()
    })
}

/// Explicit completion for direct C operations; an admitted prior exception
/// is independent of this sort's result. Python wrappers convert arguments
/// before entering this same mutation authority.
pub(crate) fn sort_in_place(
    _py: &PyToken<'_>,
    list_bits: u64,
    key_bits: u64,
    reverse: bool,
) -> bool {
    let Some(storage) = list_receiver(_py, list_bits, "sort") else {
        return false;
    };
    sort_storage(_py, storage, key_bits, reverse)
}

fn sort_storage(
    _py: &PyToken<'_>,
    storage: ListStorage<'_, '_>,
    key_bits: u64,
    reverse: bool,
) -> bool {
    if storage.native().is_some() {
        return storage.sort(key_bits, reverse);
    }
    if let Some(list_ptr) = storage.managed() {
        unsafe {
            promote_specialized_list_to_list(_py, list_ptr);
            if object_type_id(list_ptr) != TYPE_ID_LIST {
                return false;
            }
            let use_key = !obj_from_bits(key_bits).is_none();
            let len = list_len(list_ptr);
            let mut items: Vec<SortItem> = Vec::new();
            let mut ordered_values: Vec<u64> = Vec::new();
            if items.try_reserve_exact(len).is_err()
                || ordered_values.try_reserve_exact(len).is_err()
            {
                return {
                    crate::record_memory_error_without_allocation(_py);
                    crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
                };
            }
            let Some(sort_txn) = crate::object::list_mutation::ListSortTxn::begin(_py, list_ptr)
            else {
                return false;
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
                        sort_txn.finish(&ordered_values, 0..len);
                        return false;
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
            let sorted =
                timsort::timsort(&mut items, runs, |left, right| {
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
            if matches!(sorted, Err(timsort::TimsortError::Memory)) {
                // CPython selects merge-allocation failure before retiring
                // keys or callback-added values.
                crate::record_memory_error_without_allocation(_py);
            }
            if use_key {
                // In their sorted order, before the result is reversed back.
                for item in &items {
                    dec_ref_bits(_py, item.key_bits);
                }
            }
            let mutated = sorted.is_ok() && sort_txn.was_mutated();
            if mutated {
                // Key destruction is part of the mutation window. Select
                // this error while the original list is still detached;
                // restoration then retires added values under that error.
                let _ = raise_exception::<u64>(_py, "ValueError", "list modified during sort");
            }
            if reverse {
                items.reverse();
            }
            ordered_values.extend(items.iter().map(|item| item.value_bits));
            sort_txn.finish(
                &ordered_values,
                items.iter().map(|item| item.original_index),
            );
            return sorted.is_ok() && !mutated;
        }
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_add_method(list_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(storage) = list_receiver(_py, list_bits, "__add__") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() || super::list_storage::native_list(other_bits).is_some() {
            return storage.concat(other_bits);
        }
        let list_ptr = storage.managed().unwrap();
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
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
    let Some(storage) = list_receiver(py, list_bits, method) else {
        return MoltObject::none().bits();
    };
    // Explicit sequence descriptors use the repeat slot directly. Reflected
    // numeric dispatch belongs to the source operator, never this method call.
    let Some(count) = super::ops_arith::sequence_repeat_count(py, count_bits) else {
        return MoltObject::none().bits();
    };
    if storage.native().is_some() {
        return storage.repeat(count as isize, matches!(mode, ListRepeatMode::InPlace));
    }
    let list_ptr = storage.managed().unwrap();
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
        let Some(storage) = list_receiver(py, list_bits, "__getitem__") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() {
            return storage.getitem(key_bits);
        }
        crate::object::ops::molt_getitem_builtin(list_bits, key_bits)
    })
}

pub(crate) extern "C" fn list_setitem_slot(list_bits: u64, key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = list_receiver(py, list_bits, "__setitem__") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() {
            let _ = storage.assign(key_bits, Some(value_bits));
            return MoltObject::none().bits();
        }
        let _ = crate::object::ops::molt_setitem_builtin(list_bits, key_bits, value_bits);
        MoltObject::none().bits()
    })
}

pub(crate) extern "C" fn list_delitem_slot(list_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = list_receiver(py, list_bits, "__delitem__") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() {
            let _ = storage.assign(key_bits, None);
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
        let Some(storage) = list_receiver(py, list_bits, "__len__") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() {
            return storage
                .len()
                .map(|len| int_bits_from_i64(py, len as i64))
                .unwrap_or_else(|| MoltObject::none().bits());
        }
        crate::object::ops_sys::molt_len_builtin(list_bits)
    })
}

pub(crate) extern "C" fn list_contains_slot(list_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = list_receiver(py, list_bits, "__contains__") else {
            return MoltObject::none().bits();
        };
        if storage.native().is_some() {
            return storage.contains(item_bits);
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_list_count(list_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(storage) = list_receiver(py, list_bits, "count") else {
            return MoltObject::none().bits();
        };
        if !storage.promote() {
            return MoltObject::none().bits();
        }
        let (mut index, mut count) = (0, 0);
        while let Some(item) = storage.item(index) {
            let equal = unsafe { eq_bool_from_bits(py, item.bits(), val_bits) };
            drop(item);
            let Some(equal) = equal else {
                return MoltObject::none().bits();
            };
            count += i64::from(equal);
            index += 1;
        }
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        int_bits_from_i64(py, count)
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
        let Some(storage) = list_receiver(_py, list_bits, "index") else {
            return MoltObject::none().bits();
        };
        let missing = missing_bits(_py);
        let err = "slice indices must be integers or have an __index__ method";
        let mut start = if start_bits == missing {
            0
        } else {
            let Some(value) =
                crate::builtins::numbers::index_ssize_clamped_from_obj(_py, start_bits, err)
            else {
                return MoltObject::none().bits();
            };
            value
        };
        let mut stop = if stop_bits == missing {
            isize::MAX
        } else {
            let Some(value) =
                crate::builtins::numbers::index_ssize_clamped_from_obj(_py, stop_bits, err)
            else {
                return MoltObject::none().bits();
            };
            value
        };
        if !storage.promote() {
            return MoltObject::none().bits();
        }
        let Some(len) = storage.len() else {
            return MoltObject::none().bits();
        };
        if start < 0 {
            start = (start + len as isize).max(0);
        }
        if stop < 0 {
            stop = (stop + len as isize).max(0);
        }
        while start < stop {
            let Some(item) = storage.item(start as usize) else {
                break;
            };
            let equal = unsafe { eq_bool_from_bits(_py, item.bits(), val_bits) };
            drop(item);
            let Some(equal) = equal else {
                return MoltObject::none().bits();
            };
            if equal {
                return int_bits_from_i64(_py, start as i64);
            }
            start += 1;
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        // CPython 3.12/3.13 raise ValueError("<repr(x)> is not in
        // list") and propagate any exception x.__repr__ raises; 3.14
        // reverted to the static "list.index(x): x not in list" and
        // does not call __repr__.
        if crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
            return raise_exception::<_>(_py, "ValueError", "list.index(x): x not in list");
        }
        let repr_bits = crate::molt_repr_from_obj(val_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let rendered = crate::object::ops_format::string_obj_to_owned(obj_from_bits(repr_bits))
            .unwrap_or_default();
        dec_ref_bits(_py, repr_bits);
        let msg = format!("{rendered} is not in list");
        return raise_exception::<_>(_py, "ValueError", &msg);
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
