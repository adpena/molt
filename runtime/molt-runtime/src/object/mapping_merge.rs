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

/// Resolve direct merge storage only when the source retains dict.__iter__.
/// The original object remains the protocol receiver on the slow path.
pub(crate) unsafe fn direct_dict(py: &PyToken<'_>, source: u64) -> Result<Option<u64>, ()> {
    let class = type_of_bits(py, source);
    if class == builtin_classes(py).dict {
        return super::ops::dict_backing_bits(py, source);
    }
    if !unsafe { crate::object::class_layout::is_real_subtype(py, class, builtin_classes(py).dict) }
    {
        return Ok(None);
    }
    let Some(class_ptr) = obj_from_bits(class).as_ptr() else {
        return Ok(None);
    };
    let Some(dict_class) = obj_from_bits(builtin_classes(py).dict).as_ptr() else {
        return Ok(None);
    };
    let Some(name) = attr_name_bits_from_bytes(py, b"__iter__") else {
        return Err(());
    };
    let same = unsafe {
        class_attr_lookup_raw_mro(py, class_ptr, name)
            == class_attr_lookup_raw_mro(py, dict_class, name)
    };
    dec_ref_bits(py, name);
    if exception_pending(py) {
        return Err(());
    }
    if same {
        super::ops::dict_backing_bits(py, source)
    } else {
        Ok(None)
    }
}

/// Consume a mapping method's owned output using CPython method_output_as_list.
/// Exact semantic lists retain identity, including specialized int/bool storage.
/// Length hints belong to the first iterator, never the method output.
pub(crate) fn output_as_list(
    py: &PyToken<'_>,
    source: u64,
    method: &[u8],
    output: u64,
) -> Option<u64> {
    use molt_cpython_abi::api::errors::with_preserved_error;
    if exception_pending(py) {
        with_preserved_error(|| dec_ref_bits(py, output));
        return None;
    }
    if type_of_bits(py, output) == builtin_classes(py).list {
        return Some(output);
    }
    let first = OwnedIterator::new(py, output);
    if first.is_none() {
        let exception = molt_exception_last();
        if crate::builtins::exceptions::exception_matches_builtin_name(py, exception, "TypeError") {
            // Replace the pending TypeError, retaining only the independently
            // handled exception as implicit context, just like PyErr_Format.
            clear_exception(py);
            let owner = type_name(py, obj_from_bits(source));
            let result = type_name(py, obj_from_bits(output));
            let owner = owner.as_bytes();
            let result = result.as_bytes();
            let message = [
                &owner[..owner.len().min(200)],
                b".",
                method,
                b"() returned a non-iterable (type ",
                &result[..result.len().min(200)],
                b")",
            ]
            .concat();
            crate::builtins::exceptions::raise_exception_bytes::<()>(py, "TypeError", &message);
        }
        with_preserved_error(|| {
            dec_ref_bits(py, exception);
            dec_ref_bits(py, output);
        });
        return None;
    }
    with_preserved_error(|| dec_ref_bits(py, output));
    let first = first.unwrap();
    let list = unsafe { super::ops::list_from_iter_bits(py, first.bits()) };
    with_preserved_error(|| drop(first));
    list
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
        if let Some(backing) = match direct_dict(py, source) {
            Ok(backing) => backing,
            Err(()) => return MergeOutcome::Error,
        } {
            let ptr = obj_from_bits(backing).as_ptr().unwrap();
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
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, method));
        let Some(keys) = output_as_list(py, source, b"keys", output) else {
            return MergeOutcome::Error;
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
                Err(molt_runtime_core::ErrorIndicatorSet) => return MergeOutcome::Error,
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
            let name = crate::object::ops_format::format_obj_str_bytes(py, obj_from_bits(key));
            if !exception_pending(py) {
                let message = [
                    b"got multiple values for keyword argument '".as_slice(),
                    &name,
                    b"'",
                ]
                .concat();
                crate::builtins::exceptions::raise_exception_bytes::<()>(py, "TypeError", &message);
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
    validate_keyword_names(
        py,
        unsafe { dict_order(dict) }
            .chunks_exact(2)
            .map(|pair| pair[0]),
    )
}

/// Keyword names at a call boundary are strings (`str` storage, subclasses
/// included), whether they still sit in the call's mapping or were already
/// unpacked from it.
pub(crate) fn validate_keyword_names(
    py: &PyToken<'_>,
    names: impl IntoIterator<Item = u64>,
) -> bool {
    for name in names {
        if !obj_from_bits(name)
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
