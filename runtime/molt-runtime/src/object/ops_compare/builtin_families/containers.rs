use super::*;
use crate::object::ops::{dict_find_entry_with_hash, set_find_entry_in_place_with_hash};

pub(super) fn dict(
    py: &PyToken<'_>,
    left: MoltObject,
    right: MoltObject,
    op: RichCompareOp,
) -> CompareValueOutcome {
    if !op.is_equality() || physical_type(right) != Some(TYPE_ID_DICT) {
        return CompareValueOutcome::NotComparable;
    }
    let Some(_guard) = crate::state::recursion::RecursionGuard::enter_with_message(
        py,
        "maximum recursion depth exceeded in comparison",
    ) else {
        return CompareValueOutcome::Error;
    };
    let left = left.as_ptr().unwrap();
    let right = right.as_ptr().unwrap();
    let compare = || -> Result<bool, molt_runtime_core::ErrorIndicatorSet> {
        unsafe {
            if dict_len(left) != dict_len(right) {
                return Ok(false);
            }
            let mut index = 0;
            // Compact ordered storage may move or shrink at every callback.
            // Snapshot only scalar handles/hash, then pin before looking up.
            while index < dict_len(left) {
                let key = Pin::borrow(py, dict_order(left)[index * 2]);
                let value = Pin::borrow(py, dict_order(left)[index * 2 + 1]);
                let hash = dict_hashes(left)[index];
                let found = dict_find_entry_with_hash(py, right, key.bits, hash);
                if exception_pending(py) {
                    return Err(molt_runtime_core::ErrorIndicatorSet);
                }
                let Some(found) = found else {
                    return Ok(false);
                };
                let other = Pin::borrow(py, dict_order(right)[found * 2 + 1]);
                let equal = element_equal(py, value.bits, other.bits)?;
                drop(other);
                drop(value);
                drop(key);
                if exception_pending(py) {
                    return Err(molt_runtime_core::ErrorIndicatorSet);
                }
                if !equal {
                    return Ok(false);
                }
                index += 1;
            }
            Ok(true)
        }
    };
    equality(compare(), op)
}

// Declaring set slots use stored hashes and physical storage. Dict views use
// the observable size, iteration and membership protocols below instead.
fn set_contained(
    py: &PyToken<'_>,
    left: *mut u8,
    right: *mut u8,
) -> Result<bool, molt_runtime_core::ErrorIndicatorSet> {
    unsafe {
        let mut index = 0;
        while let Some(entry) = crate::object::ops::set_pin_entry(py, left, index) {
            let found = set_find_entry_in_place_with_hash(py, right, entry.bits(), entry.hash());
            drop(entry);
            if exception_pending(py) {
                return Err(molt_runtime_core::ErrorIndicatorSet);
            }
            if found.is_none() {
                return Ok(false);
            }
            index += 1;
        }
        Ok(true)
    }
}

fn view_contained(
    py: &PyToken<'_>,
    left: u64,
    right: u64,
) -> Result<bool, molt_runtime_core::ErrorIndicatorSet> {
    let mut iter = crate::object::iterable::OwnedIterator::new(py, left)
        .ok_or(molt_runtime_core::ErrorIndicatorSet)?;
    while let Some(item) = iter.next()? {
        let item = Pin::adopt(py, item);
        let result = Pin::adopt(py, molt_contains(right, item.bits));
        if exception_pending(py) {
            return Err(molt_runtime_core::ErrorIndicatorSet);
        }
        let found = is_truthy(py, obj_from_bits(result.bits));
        drop(result);
        drop(item);
        if exception_pending(py) {
            return Err(molt_runtime_core::ErrorIndicatorSet);
        }
        if !found {
            return Ok(false);
        }
    }
    Ok(true)
}

fn protocol_size(
    py: &PyToken<'_>,
    value: u64,
) -> Result<i64, molt_runtime_core::ErrorIndicatorSet> {
    let result = Pin::adopt(py, molt_len(value));
    if exception_pending(py) {
        return Err(molt_runtime_core::ErrorIndicatorSet);
    }
    crate::builtins::numbers::index_i64_integral_bits(result.bits)
        .ok_or(molt_runtime_core::ErrorIndicatorSet)
}

pub(super) fn set_like(
    py: &PyToken<'_>,
    family: BuiltinComparison,
    left: MoltObject,
    right: MoltObject,
    op: RichCompareOp,
) -> CompareValueOutcome {
    let view = matches!(
        family,
        BuiltinComparison::DictKeys | BuiltinComparison::DictItems
    );
    let right_type = physical_type(right);
    if !(matches!(right_type, Some(TYPE_ID_SET | TYPE_ID_FROZENSET))
        || (view
            && matches!(
                right_type,
                Some(TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_ITEMS_VIEW)
            )))
    {
        return CompareValueOutcome::NotComparable;
    }
    let Some(_guard) = crate::state::recursion::RecursionGuard::enter_with_message(
        py,
        "maximum recursion depth exceeded in comparison",
    ) else {
        return CompareValueOutcome::Error;
    };
    let result = (|| -> Result<bool, molt_runtime_core::ErrorIndicatorSet> {
        let (lhs, rhs) = if view {
            (
                protocol_size(py, left.bits())?,
                protocol_size(py, right.bits())?,
            )
        } else {
            unsafe {
                (
                    crate::builtins::containers::set_len(left.as_ptr().unwrap()) as i64,
                    crate::builtins::containers::set_len(right.as_ptr().unwrap()) as i64,
                )
            }
        };
        let admitted = match op {
            RichCompareOp::Eq | RichCompareOp::Ne => lhs == rhs,
            RichCompareOp::Lt => lhs < rhs,
            RichCompareOp::Le => lhs <= rhs,
            RichCompareOp::Gt => lhs > rhs,
            RichCompareOp::Ge => lhs >= rhs,
        };
        if !admitted {
            return Ok(false);
        }
        let (source, probe) = if matches!(op, RichCompareOp::Gt | RichCompareOp::Ge) {
            (right, left)
        } else {
            (left, right)
        };
        if view {
            view_contained(py, source.bits(), probe.bits())
        } else {
            set_contained(py, source.as_ptr().unwrap(), probe.as_ptr().unwrap())
        }
    })();
    equality(result, op)
}

/// The sq_contains authority used by both view descriptors and ordinary `in`.
/// Key lookup hashes the requested key; item values need not be hashable.
pub(super) fn view_contains(
    py: &PyToken<'_>,
    family: BuiltinComparison,
    view: u64,
    item: u64,
) -> Result<bool, molt_runtime_core::ErrorIndicatorSet> {
    let _view = Pin::borrow(py, view);
    let _item = Pin::borrow(py, item);
    unsafe {
        let dict = obj_from_bits(dict_view_dict_bits(obj_from_bits(view).as_ptr().unwrap()))
            .as_ptr()
            .ok_or(molt_runtime_core::ErrorIndicatorSet)?;
        let _dict = Pin::borrow(py, MoltObject::from_ptr(dict).bits());
        if family == BuiltinComparison::DictKeys {
            let found = crate::object::ops::dict_find_entry(py, dict, item);
            return if exception_pending(py) {
                Err(molt_runtime_core::ErrorIndicatorSet)
            } else {
                Ok(found.is_some())
            };
        }
        let Some(tuple) = obj_from_bits(item).as_ptr() else {
            return Ok(false);
        };
        if object_type_id(tuple) != TYPE_ID_TUPLE {
            return Ok(false);
        }
        let pair = crate::object::seq_access::with_immutable_tuple_slice(tuple, |values| {
            (values.len() == 2).then(|| (values[0], values[1]))
        })
        .flatten();
        let Some((key, value)) = pair else {
            return Ok(false);
        };
        let key = Pin::borrow(py, key);
        let value = Pin::borrow(py, value);
        let found = crate::object::ops::dict_find_entry(py, dict, key.bits);
        if exception_pending(py) {
            return Err(molt_runtime_core::ErrorIndicatorSet);
        }
        let Some(found) = found else {
            return Ok(false);
        };
        let stored = Pin::borrow(py, dict_order(dict)[found * 2 + 1]);
        element_equal(py, stored.bits, value.bits)
    }
}
