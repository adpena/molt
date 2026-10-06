//! Managed storage adapter for the shared native/C-ABI lexicographic contract.
use super::*;
use crate::object::seq_access::{
    PinnedSequenceItem, locked_len, pin_item, with_immutable_tuple_slice,
};
use molt_obj_model::sequence_compare::{
    RichCompareOp, SequenceCompareContext, SequenceKind, compare_sequences,
};

enum Item<'a, 'py> {
    // Published tuple storage is immutable and both containers remain live
    // through the operation. Avoid RC traffic for their borrowed elements.
    Tuple(u64),
    List(PinnedSequenceItem<'a, 'py>),
}

impl Item<'_, '_> {
    fn bits(&self) -> u64 {
        match self {
            Self::Tuple(bits) => *bits,
            Self::List(item) => item.bits(),
        }
    }
}

struct ManagedSequence<'a, 'py> {
    py: &'a PyToken<'py>,
    left: *mut u8,
    right: *mut u8,
    kind: SequenceKind,
}

impl<'a, 'py> SequenceCompareContext for ManagedSequence<'a, 'py> {
    type Item = Item<'a, 'py>;
    type Value = u64;
    type Error = molt_runtime_core::ErrorIndicatorSet;

    fn lengths(&self) -> Result<(usize, usize), molt_runtime_core::ErrorIndicatorSet> {
        Ok(unsafe { (locked_len(self.left), locked_len(self.right)) })
    }

    fn pin_pair(
        &self,
        index: usize,
    ) -> Result<Option<(Self::Item, Self::Item)>, molt_runtime_core::ErrorIndicatorSet> {
        unsafe {
            if matches!(self.kind, SequenceKind::Tuple) {
                let pair = with_immutable_tuple_slice(self.left, |left| {
                    with_immutable_tuple_slice(self.right, |right| {
                        Some((
                            Item::Tuple(*left.get(index)?),
                            Item::Tuple(*right.get(index)?),
                        ))
                    })
                })
                .flatten()
                .flatten();
                return match pair {
                    Some(pair) => Ok(Some(pair)),
                    None => {
                        raise_exception::<()>(
                            self.py,
                            "SystemError",
                            "invalid tuple comparison storage",
                        );
                        Err(molt_runtime_core::ErrorIndicatorSet)
                    }
                };
            }
            let Some(left) = pin_item(self.py, self.left, index) else {
                return Ok(None);
            };
            let Some(right) = pin_item(self.py, self.right, index) else {
                return Ok(None);
            };
            Ok(Some((Item::List(left), Item::List(right))))
        }
    }

    fn equal(
        &self,
        left: &Self::Item,
        right: &Self::Item,
    ) -> Result<bool, molt_runtime_core::ErrorIndicatorSet> {
        match compare_object_eq_bool(
            self.py,
            obj_from_bits(left.bits()),
            obj_from_bits(right.bits()),
        ) {
            CompareBoolOutcome::True => Ok(true),
            CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => Ok(false),
            CompareBoolOutcome::Error => Err(molt_runtime_core::ErrorIndicatorSet),
        }
    }

    fn order(
        &self,
        left: &Self::Item,
        right: &Self::Item,
        op: RichCompareOp,
    ) -> Result<u64, molt_runtime_core::ErrorIndicatorSet> {
        let op = ordering_op(op);
        match compare_object_value_for_op(
            self.py,
            obj_from_bits(left.bits()),
            obj_from_bits(right.bits()),
            op,
        ) {
            CompareValueOutcome::Value(bits) => Ok(bits),
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                Err(molt_runtime_core::ErrorIndicatorSet)
            }
        }
    }

    fn boolean(&self, value: bool) -> u64 {
        MoltObject::from_bool(value).bits()
    }

    fn equal_prefix(&self) -> Result<usize, molt_runtime_core::ErrorIndicatorSet> {
        unsafe {
            if matches!(self.kind, SequenceKind::Tuple) {
                return with_immutable_tuple_slice(self.left, |left| {
                    with_immutable_tuple_slice(self.right, |right| {
                        simd_find_first_mismatch(left, right)
                    })
                })
                .flatten()
                .ok_or(molt_runtime_core::ErrorIndicatorSet);
            }
        }
        Ok(0)
    }
}

pub(super) unsafe fn compare(
    py: &PyToken<'_>,
    left: *mut u8,
    right: *mut u8,
    op: RichCompareOp,
) -> CompareValueOutcome {
    let Some(_guard) = crate::state::recursion::RecursionGuard::enter_with_message(
        py,
        "maximum recursion depth exceeded in comparison",
    ) else {
        return CompareValueOutcome::Error;
    };
    let kind = if unsafe { object_type_id(left) } == TYPE_ID_TUPLE {
        SequenceKind::Tuple
    } else {
        SequenceKind::List
    };
    match compare_sequences(
        &ManagedSequence {
            py,
            left,
            right,
            kind,
        },
        kind,
        op,
    ) {
        Ok(value) => CompareValueOutcome::Value(value),
        Err(molt_runtime_core::ErrorIndicatorSet) => CompareValueOutcome::Error,
    }
}
