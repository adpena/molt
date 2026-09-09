//! Mapping traversal shared by dictionary updates and call keyword expansion.
//! Direct dictionaries supply cached hashes. Other mappings materialize keys
//! before value lookup; the destination decides whether duplicates are legal.

use super::iterable::OwnedIterator;
use crate::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeOutcome {
    Complete,
    NotMapping,
    Error,
}

unsafe fn direct_dict(py: &PyToken<'_>, ptr: *mut u8) -> bool {
    if unsafe { object_type_id(ptr) } != TYPE_ID_DICT {
        return false;
    }
    let class = unsafe { object_class_bits(ptr) };
    if class == 0 || class == builtin_classes(py).dict {
        return true;
    }
    let Some(class_ptr) = obj_from_bits(class).as_ptr() else {
        return false;
    };
    let Some(dict_class) = obj_from_bits(builtin_classes(py).dict).as_ptr() else {
        return false;
    };
    let Some(name) = attr_name_bits_from_bytes(py, b"__iter__") else {
        return false;
    };
    let same = unsafe {
        class_attr_lookup_raw_mro(py, class_ptr, name)
            == class_attr_lookup_raw_mro(py, dict_class, name)
    };
    dec_ref_bits(py, name);
    same
}

pub(crate) unsafe fn apply(
    py: &PyToken<'_>,
    source: u64,
    mut accept_key: impl FnMut(u64, Option<u64>) -> bool,
    mut insert: impl FnMut(u64, u64, Option<u64>) -> bool,
) -> MergeOutcome {
    unsafe {
        let Some(ptr) = obj_from_bits(source).as_ptr() else {
            return MergeOutcome::NotMapping;
        };
        if direct_dict(py, ptr) {
            let length = dict_order(ptr).len();
            for index in (0..length).step_by(2) {
                // No source backing borrow survives Python hash/equality or a
                // destination setter. Pin both objects before invoking it.
                let (key, value, hash) = {
                    let order = dict_order(ptr);
                    (order[index], order[index + 1], dict_hashes(ptr)[index / 2])
                };
                inc_ref_bits(py, key);
                inc_ref_bits(py, value);
                let ok = accept_key(key, Some(hash)) && insert(key, value, Some(hash));
                dec_ref_bits(py, key);
                dec_ref_bits(py, value);
                if !ok || exception_pending(py) {
                    return MergeOutcome::Error;
                }
                if dict_order(ptr).len() != length {
                    raise_exception::<()>(py, "RuntimeError", "dict mutated during update");
                    return MergeOutcome::Error;
                }
            }
            return MergeOutcome::Complete;
        }
        if exception_pending(py) {
            return MergeOutcome::Error;
        }
        let Some(name) = attr_name_bits_from_bytes(py, b"keys") else {
            return MergeOutcome::Error;
        };
        let method = attr_lookup_ptr_allow_missing(py, ptr, name);
        dec_ref_bits(py, name);
        if exception_pending(py) {
            if let Some(method) = method {
                dec_ref_bits(py, method);
            }
            return MergeOutcome::Error;
        }
        let Some(method) = method else {
            return MergeOutcome::NotMapping;
        };
        let output = call_callable0(py, method);
        dec_ref_bits(py, method);
        if exception_pending(py) {
            dec_ref_bits(py, output);
            return MergeOutcome::Error;
        }
        let exact_list = obj_from_bits(output).as_ptr().is_some_and(|ptr| {
            object_type_id(ptr) == TYPE_ID_LIST
                && (object_class_bits(ptr) == 0
                    || object_class_bits(ptr) == builtin_classes(py).list)
        });
        let keys = if exact_list {
            output
        } else {
            // PyMapping_Keys first gets an iterator, then materializes THAT
            // iterator as a list: __iter__ can observably run twice.
            let first = OwnedIterator::new(py, output);
            dec_ref_bits(py, output);
            let Some(first) = first else {
                return MergeOutcome::Error;
            };
            let Some(keys) = super::ops::list_from_iter_bits(py, first.bits()) else {
                return MergeOutcome::Error;
            };
            keys
        };
        let iter = OwnedIterator::new(py, keys);
        dec_ref_bits(py, keys);
        let Some(mut iter) = iter else {
            return MergeOutcome::Error;
        };
        loop {
            let key = match iter.next() {
                Ok(Some(key)) => key,
                Ok(None) => return MergeOutcome::Complete,
                Err(()) => return MergeOutcome::Error,
            };
            if !accept_key(key, None) {
                dec_ref_bits(py, key);
                return MergeOutcome::Error;
            }
            let value = molt_getitem_method(source, key);
            let ok = !exception_pending(py) && insert(key, value, None);
            dec_ref_bits(py, key);
            dec_ref_bits(py, value);
            if !ok || exception_pending(py) {
                return MergeOutcome::Error;
            }
        }
    }
}

/// Reject duplicates using Python dictionary hash/equality/truth semantics.
pub(crate) unsafe fn keyword_available(
    py: &PyToken<'_>,
    target: *mut u8,
    key: u64,
    hash: Option<u64>,
) -> bool {
    unsafe {
        // Generic mappings intentionally hash once here and once on insert,
        // including the first key in an initially empty destination.
        let found = if let Some(hash) = hash {
            super::ops::dict_find_entry_with_hash(py, target, key, hash).is_some()
        } else {
            let result = molt_dict_contains(MoltObject::from_ptr(target).bits(), key);
            obj_from_bits(result).as_bool() == Some(true)
        };
        if exception_pending(py) {
            return false;
        }
        if found {
            let name = format_obj_str(py, obj_from_bits(key));
            if !exception_pending(py) {
                raise_exception::<()>(
                    py,
                    "TypeError",
                    &format!("got multiple values for keyword argument '{name}'"),
                );
            }
            return false;
        }
        true
    }
}

pub(crate) unsafe fn insert_dict(
    py: &PyToken<'_>,
    target: *mut u8,
    key: u64,
    value: u64,
    hash: Option<u64>,
) -> bool {
    unsafe {
        if let Some(hash) = hash {
            super::ops::dict_set_with_hash_in_place(py, target, key, value, hash);
        } else {
            super::ops::dict_set_in_place(py, target, key, value);
        }
    }
    !exception_pending(py)
}

pub(crate) unsafe fn merge_keywords(py: &PyToken<'_>, target: *mut u8, mapping: u64) -> bool {
    let result = unsafe {
        apply(
            py,
            mapping,
            |key, hash| keyword_available(py, target, key, hash),
            |key, value, hash| insert_dict(py, target, key, value, hash),
        )
    };
    match result {
        MergeOutcome::Complete => true,
        MergeOutcome::Error => false,
        MergeOutcome::NotMapping => {
            raise_exception::<()>(py, "TypeError", "argument after ** must be a mapping");
            false
        }
    }
}

/// This is a call boundary operation, never an operand-expansion operation.
pub(crate) unsafe fn validate_keywords(py: &PyToken<'_>, dict: *mut u8) -> bool {
    for pair in unsafe { dict_order(dict) }.chunks_exact(2) {
        if !obj_from_bits(pair[0])
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
        {
            raise_exception::<()>(py, "TypeError", "keywords must be strings");
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_probe_uses_dictionary_equality_not_carrier_identity() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let truth = MoltObject::from_bool(true).bits();
                let one = MoltObject::from_int(1).bits();
                let first = MoltObject::from_int(3).bits();
                let second = MoltObject::from_int(4).bits();
                let dict = alloc_dict_with_pairs(_py, &[truth, first]);
                assert!(!dict.is_null());
                assert_eq!(
                    crate::object::ops::dict_get_in_place(_py, dict, one),
                    Some(first)
                );
                crate::object::ops::dict_set_inline_int_in_place(_py, dict, one, 1, second);
                assert!(!exception_pending(_py));
                assert_eq!(dict_order(dict), &[truth, second]);
                assert!(!keyword_available(_py, dict, one, None));
                assert!(exception_pending(_py));
                crate::molt_exception_clear();
                dec_ref_bits(_py, MoltObject::from_ptr(dict).bits());
            }
        });
    }

    #[test]
    fn rejected_keyword_merge_preserves_previous_value() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let key = MoltObject::from_int(1).bits();
                let first = MoltObject::from_int(3).bits();
                let second = MoltObject::from_int(4).bits();
                let target = alloc_dict_with_pairs(_py, &[key, first]);
                let source = alloc_dict_with_pairs(_py, &[key, second]);
                assert!(!merge_keywords(
                    _py,
                    target,
                    MoltObject::from_ptr(source).bits()
                ));
                assert!(exception_pending(_py));
                assert_eq!(dict_order(target), &[key, first]);
                crate::molt_exception_clear();
                dec_ref_bits(_py, MoltObject::from_ptr(source).bits());
                dec_ref_bits(_py, MoltObject::from_ptr(target).bits());
            }
        });
    }
}
