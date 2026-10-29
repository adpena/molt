use crate::BuiltinExc;
#[cfg(test)]
use crate::exception_pending;
use crate::{
    MoltObject, PyToken, TYPE_ID_FROZENSET, TYPE_ID_LIST, TYPE_ID_SET, TYPE_ID_TUPLE, dec_ref_bits,
    dict_update_apply, dict_update_set_in_place, obj_from_bits, object_type_id, raise_exception,
    usize_from_bits,
};

#[unsafe(no_mangle)]
pub extern "C" fn molt_dict_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Ok(capacity_hint) = usize::try_from(capacity_bits) else {
            crate::record_memory_error_without_allocation(_py);
            return MoltObject::none().bits();
        };
        let ptr =
            crate::object::builders::alloc_dict_with_capacity_and_pairs(_py, capacity_hint, &[]);
        if ptr.is_null() {
            return MoltObject::none().bits();
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
    let type_error = crate::builtins::exceptions::exception_matches_kind(
        py,
        error,
        crate::BuiltinExc::TypeError,
    );
    dec_ref_bits(py, error);
    if type_error {
        if !crate::object::ops_sys::runtime_target_at_least(py, 3, 14) {
            crate::molt_exception_clear();
            return DictSeqError::NotIterable;
        }
        if acquisition {
            crate::molt_exception_clear();
            raise_exception::<()>(py, BuiltinExc::TypeError, "object is not iterable");
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
        // Direct mapping merge owns its clone/presize policy; generic mappings
        // begin empty and grow while consuming their real protocol.
        let dict_bits = molt_dict_new(0);
        if obj_from_bits(dict_bits).is_none() {
            return MoltObject::none().bits();
        }
        if !unsafe { dict_update_apply(_py, dict_bits, dict_update_set_in_place, obj_bits) } {
            dec_ref_bits(_py, dict_bits);
            return MoltObject::none().bits();
        }
        dict_bits
    })
}

fn set_like_new(py: &PyToken<'_>, capacity_bits: u64, type_id: u32) -> u64 {
    let Some(capacity) = usize_from_bits(capacity_bits) else {
        crate::record_memory_error_without_allocation(py);
        return MoltObject::none().bits();
    };
    let ptr = crate::object::builders::alloc_set_like_with_capacity_and_entries(
        py,
        capacity,
        &[],
        type_id,
    );
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_set_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_like_new(py, capacity_bits, TYPE_ID_SET) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_frozenset_new(capacity_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { set_like_new(py, capacity_bits, TYPE_ID_FROZENSET) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::{LimitedTracker, ResourceLimits, UnlimitedTracker, set_tracker};
    use crate::{TYPE_ID_DICT, dict_len, dict_storage, dict_table, set_len};

    struct DenyAllocations;

    impl DenyAllocations {
        fn enter() -> Self {
            set_tracker(Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(0),
                max_allocations: Some(0),
                ..Default::default()
            })));
            Self
        }
    }

    impl Drop for DenyAllocations {
        fn drop(&mut self) {
            set_tracker(Box::new(UnlimitedTracker));
        }
    }

    #[test]
    fn hash_aggregate_constructors_accept_raw_capacity_counts() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            for (constructor, expected_type) in [
                (molt_dict_new as extern "C" fn(u64) -> u64, TYPE_ID_DICT),
                (molt_set_new, TYPE_ID_SET),
                (molt_frozenset_new, TYPE_ID_FROZENSET),
            ] {
                for capacity in [0, 1, 2] {
                    let bits = constructor(capacity);
                    assert!(!exception_pending(_py), "capacity={capacity}");
                    let ptr = obj_from_bits(bits)
                        .as_ptr()
                        .expect("raw capacity must construct an aggregate");
                    unsafe {
                        assert_eq!(object_type_id(ptr), expected_type);
                        let len = if expected_type == TYPE_ID_DICT {
                            dict_len(ptr)
                        } else {
                            set_len(ptr)
                        };
                        assert_eq!(len, 0, "capacity={capacity}");
                    }
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    #[test]
    fn dictionary_constructor_bounds_raw_presize_hints() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // CPython 3.12–3.15 dict_new_presized deliberately caps hints at
            // 2^17 slots; an empty dict need not admit every hinted row before
            // resizing. These are literal table/usable boundaries, independent
            // of the implementation's capacity calculator.
            for (hint, table_slots, usable) in [
                (0, 0, 0),
                (5, 0, 0),
                (6, 16, 10),
                (10, 16, 10),
                (11, 32, 21),
                (87_381, 131_072, 87_381),
                (87_382, 131_072, 87_381),
                (usize::MAX, 131_072, 87_381),
            ] {
                let bits = molt_dict_new(hint as u64);
                let ptr = obj_from_bits(bits)
                    .as_ptr()
                    .expect("representable dictionary hints are bounded, not rejected");
                assert!(!exception_pending(py), "hint={hint}");
                unsafe {
                    assert_eq!(object_type_id(ptr), TYPE_ID_DICT);
                    assert_eq!(dict_len(ptr), 0, "a capacity hint creates no rows");
                    assert_eq!(dict_table(ptr).len(), table_slots, "hint={hint}");
                    assert_eq!(dict_storage(ptr).usable, usable, "hint={hint}");
                }
                dec_ref_bits(py, bits);
            }
        });
    }

    #[test]
    fn set_aggregate_capacity_overflow_fails_closed_as_memory_error() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (name, constructor) in [
                ("set", molt_set_new as extern "C" fn(u64) -> u64),
                ("frozenset", molt_frozenset_new),
            ] {
                for capacity in [usize::MAX, usize::MAX / 2] {
                    let bits = constructor(capacity as u64);
                    assert!(obj_from_bits(bits).is_none(), "{name}: capacity={capacity}");
                    crate::test_support::assert_and_clear_emergency_memory_error(py);
                }
            }
        });
    }

    #[test]
    fn hash_aggregate_allocation_denial_selects_memory_error_at_exported_boundary() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for (name, constructor, capacity) in [
                (
                    "dict",
                    molt_dict_new as extern "C" fn(u64) -> u64,
                    usize::MAX as u64,
                ),
                ("set", molt_set_new, 6),
                ("frozenset", molt_frozenset_new, 6),
            ] {
                // All hints fit the target. Dict's largest hint is bounded;
                // its actual allocation remains subject to the same budget.
                for prior_error in [false, true] {
                    if prior_error {
                        raise_exception::<()>(
                            py,
                            BuiltinExc::ValueError,
                            "prior constructor error",
                        );
                    }
                    let budget = DenyAllocations::enter();
                    let bits = constructor(capacity);
                    assert!(
                        obj_from_bits(bits).is_none(),
                        "{name}: prior_error={prior_error}"
                    );
                    crate::test_support::assert_and_clear_emergency_memory_error(py);
                    drop(budget);
                }
            }
        });
    }

    #[cfg(target_pointer_width = "32")]
    #[test]
    fn hash_aggregate_raw_capacity_rejects_values_outside_target_usize() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            for constructor in [
                molt_dict_new as extern "C" fn(u64) -> u64,
                molt_set_new,
                molt_frozenset_new,
            ] {
                // This is an unrepresentable raw extent, unlike usize::MAX's
                // representable dictionary hint on either 32- or 64-bit hosts.
                for capacity in [u64::from(u32::MAX) + 1, u64::MAX] {
                    let bits = constructor(capacity);
                    assert!(obj_from_bits(bits).is_none());
                    crate::test_support::assert_and_clear_emergency_memory_error(py);
                }
            }
        });
    }
}
