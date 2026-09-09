use crate::{
    MoltHeader, MoltObject, PyToken, TYPE_ID_DICT, TYPE_ID_FROZENSET, TYPE_ID_LIST, TYPE_ID_SET,
    TYPE_ID_TUPLE, alloc_object, dec_ref_bits, dict_len, dict_update_apply,
    dict_update_set_in_place, exception_pending, maybe_ptr_from_bits, obj_from_bits,
    object_type_id, raise_exception, set_table_capacity, usize_from_bits,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Ok(capacity_hint) = usize::try_from(capacity_bits) else {
            return raise_exception::<_>(_py, "MemoryError", "dict allocation failed");
        };
        let ptr =
            crate::object::builders::alloc_dict_with_capacity_and_pairs(_py, capacity_hint, &[]);
        if ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "dict allocation failed");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DictSeqError {
    NotIterable,
    BadLen(usize),
    Exception,
}

fn dict_pair_iteration_error(py: &PyToken<'_>, acquisition: bool) -> DictSeqError {
    let error = crate::builtins::exceptions::molt_exception_last_pending();
    let type_error =
        crate::builtins::exceptions::exception_matches_builtin_name(py, error, "TypeError");
    dec_ref_bits(py, error);
    if type_error {
        if !crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
            crate::molt_exception_clear();
            return DictSeqError::NotIterable;
        }
        if acquisition {
            crate::molt_exception_clear();
            raise_exception::<()>(py, "TypeError", "object is not iterable");
        }
    }
    DictSeqError::Exception
}

/// Return two owned references, irrespective of the input representation.
pub(crate) fn dict_pair_from_item(
    _py: &PyToken<'_>,
    item_bits: u64,
) -> Result<(u64, u64), DictSeqError> {
    if let Some(ptr) = obj_from_bits(item_bits).as_ptr() {
        unsafe {
            let class = crate::object_class_bits(ptr);
            let exact = class == 0 || crate::is_builtin_class_bits(_py, class);
            if exact && matches!(object_type_id(ptr), TYPE_ID_LIST | TYPE_ID_TUPLE) {
                return crate::object::seq_access::with_borrowed(ptr, |items| {
                    if items.len() != 2 {
                        return Err(DictSeqError::BadLen(items.len()));
                    }
                    crate::inc_ref_bits(_py, items[0]);
                    crate::inc_ref_bits(_py, items[1]);
                    Ok((items[0], items[1]))
                });
            }
        }
    }
    let Some(first) = crate::object::iterable::OwnedIterator::new(_py, item_bits) else {
        return Err(dict_pair_iteration_error(_py, true));
    };
    // PySequence_Fast's list materialization acquires the returned iterator a
    // second time. Python 3.14 preserves later callback errors; earlier versions
    // translate all TypeErrors at the dictionary-update boundary.
    let mut values = crate::object::iterable::collect(
        _py,
        first.bits(),
        crate::object::iterable::LengthHint::Consult,
    )
    .ok_or_else(|| dict_pair_iteration_error(_py, false))?;
    if values.len() != 2 {
        let len = values.len();
        for value in values {
            dec_ref_bits(_py, value);
        }
        return Err(DictSeqError::BadLen(len));
    }
    let value = values.pop().unwrap();
    let key = values.pop().unwrap();
    Ok((key, value))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_from_obj(obj_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let mut capacity = 0usize;
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_DICT {
                    capacity = dict_len(ptr);
                }
            }
        }
        let dict_bits = molt_dict_new(capacity as u64);
        if obj_from_bits(dict_bits).is_none() {
            return MoltObject::none().bits();
        }
        let Some(_dict_ptr) = maybe_ptr_from_bits(dict_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, obj_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        dict_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<*mut Vec<usize>>()
            + std::mem::size_of::<*mut Vec<u64>>();
        let ptr = alloc_object(_py, total, TYPE_ID_SET);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let Some(capacity_hint) = usize_from_bits(capacity_bits) else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            };
            let Some(order_ptr) =
                crate::object::backing::tracked_vec_box_with_capacity::<u64>(capacity_hint)
            else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            };
            let table_cap = if capacity_hint > 0 {
                set_table_capacity(capacity_hint)
            } else {
                0
            };
            let Some(table_ptr) =
                crate::object::backing::tracked_vec_box_zeroed::<usize>(table_cap)
            else {
                drop(crate::object::backing::tracked_vec_box_from_raw(order_ptr));
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            };
            let Some(hashes_ptr) =
                crate::object::backing::tracked_vec_box_with_capacity::<u64>(capacity_hint)
            else {
                drop(crate::object::backing::tracked_vec_box_from_raw(table_ptr));
                drop(crate::object::backing::tracked_vec_box_from_raw(order_ptr));
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            };
            *(ptr as *mut *mut Vec<u64>) = order_ptr;
            *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
            *(ptr.add(std::mem::size_of::<*mut Vec<u64>>() + std::mem::size_of::<*mut Vec<usize>>())
                as *mut *mut Vec<u64>) = hashes_ptr;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let total = std::mem::size_of::<MoltHeader>()
            + std::mem::size_of::<*mut Vec<u64>>()
            + std::mem::size_of::<*mut Vec<usize>>()
            + std::mem::size_of::<*mut Vec<u64>>();
        let ptr = alloc_object(_py, total, TYPE_ID_FROZENSET);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        unsafe {
            let Some(capacity_hint) = usize_from_bits(capacity_bits) else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            };
            let Some(order_ptr) =
                crate::object::backing::tracked_vec_box_with_capacity::<u64>(capacity_hint)
            else {
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            };
            let table_cap = if capacity_hint > 0 {
                set_table_capacity(capacity_hint)
            } else {
                0
            };
            let Some(table_ptr) =
                crate::object::backing::tracked_vec_box_zeroed::<usize>(table_cap)
            else {
                drop(crate::object::backing::tracked_vec_box_from_raw(order_ptr));
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            };
            let Some(hashes_ptr) =
                crate::object::backing::tracked_vec_box_with_capacity::<u64>(capacity_hint)
            else {
                drop(crate::object::backing::tracked_vec_box_from_raw(table_ptr));
                drop(crate::object::backing::tracked_vec_box_from_raw(order_ptr));
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
                return MoltObject::none().bits();
            };
            *(ptr as *mut *mut Vec<u64>) = order_ptr;
            *(ptr.add(std::mem::size_of::<*mut Vec<u64>>()) as *mut *mut Vec<usize>) = table_ptr;
            *(ptr.add(std::mem::size_of::<*mut Vec<u64>>() + std::mem::size_of::<*mut Vec<usize>>())
                as *mut *mut Vec<u64>) = hashes_ptr;
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dict_capacity_overflow_fails_closed_as_memory_error() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let bits = molt_dict_new(u64::MAX);
            assert!(obj_from_bits(bits).is_none());
            assert!(exception_pending(_py));
            let _ = crate::molt_exception_clear();
        });
    }
}
