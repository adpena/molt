use super::*;
use crate::object::ops::{range_components_bigint, range_len_bigint};
use molt_obj_model::sequence_compare::{compare_sequences, SequenceCompareContext, SequenceKind};

pub(super) fn range(left: MoltObject, right: MoltObject, op: RichCompareOp) -> CompareValueOutcome {
    if !op.is_equality() || physical_type(right) != Some(TYPE_ID_RANGE) {
        return CompareValueOutcome::NotComparable;
    }
    let Some((ls, le, ld)) = range_components_bigint(left.as_ptr().unwrap()) else {
        return CompareValueOutcome::Error;
    };
    let Some((rs, re, rd)) = range_components_bigint(right.as_ptr().unwrap()) else {
        return CompareValueOutcome::Error;
    };
    let llen = range_len_bigint(&ls, &le, &ld);
    let rlen = range_len_bigint(&rs, &re, &rd);
    equality(Ok(llen == rlen && (llen == BigInt::from(0)
        || (ls == rs && (llen == BigInt::from(1) || ld == rd)))), op)
}

struct SliceFields<'a, 'py> {
    py: &'a PyToken<'py>,
    left: [u64; 3],
    right: [u64; 3],
}

impl SequenceCompareContext for SliceFields<'_, '_> {
    type Item = u64;
    type Value = u64;
    type Error = ();
    fn lengths(&self) -> Result<(usize, usize), ()> { Ok((3, 3)) }
    fn pin_pair(&self, index: usize) -> Result<Option<(u64, u64)>, ()> {
        // Both immutable slice owners are pinned by the declaring contract.
        Ok(self.left.get(index).zip(self.right.get(index)).map(|(&l, &r)| (l, r)))
    }
    fn equal(&self, left: &u64, right: &u64) -> Result<bool, ()> {
        element_equal(self.py, *left, *right)
    }
    fn order(&self, left: &u64, right: &u64, op: RichCompareOp) -> Result<u64, ()> {
        let op = match op {
            RichCompareOp::Lt => CompareOp::Lt,
            RichCompareOp::Le => CompareOp::Le,
            RichCompareOp::Gt => CompareOp::Gt,
            RichCompareOp::Ge => CompareOp::Ge,
            _ => unreachable!("slice ordering"),
        };
        match compare_object_value_for_op(self.py, obj_from_bits(*left), obj_from_bits(*right), op) {
            CompareValueOutcome::Value(value) => Ok(value),
            _ => Err(()),
        }
    }
    fn boolean(&self, value: bool) -> u64 { MoltObject::from_bool(value).bits() }
}

pub(super) fn slice(
    py: &PyToken<'_>, left: MoltObject, right: MoltObject, op: RichCompareOp,
) -> CompareValueOutcome {
    if physical_type(right) != Some(TYPE_ID_SLICE) { return CompareValueOutcome::NotComparable; }
    let Some(_guard) = crate::state::recursion::RecursionGuard::enter_with_message(
        py, "maximum recursion depth exceeded in comparison",
    ) else { return CompareValueOutcome::Error; };
    let fields = |ptr| unsafe { [slice_start_bits(ptr), slice_stop_bits(ptr), slice_step_bits(ptr)] };
    let ctx = SliceFields { py, left: fields(left.as_ptr().unwrap()), right: fields(right.as_ptr().unwrap()) };
    match compare_sequences(&ctx, SequenceKind::Tuple, op) {
        Ok(value) => CompareValueOutcome::Value(value),
        Err(()) => CompareValueOutcome::Error,
    }
}

pub(super) fn generic_alias(
    py: &PyToken<'_>, left: MoltObject, right: MoltObject, op: RichCompareOp,
) -> CompareValueOutcome {
    if !op.is_equality() || physical_type(right) != Some(TYPE_ID_GENERIC_ALIAS) {
        return CompareValueOutcome::NotComparable;
    }
    let Some(_guard) = crate::state::recursion::RecursionGuard::enter_with_message(
        py, "maximum recursion depth exceeded in comparison",
    ) else { return CompareValueOutcome::Error; };
    unsafe {
        let left = left.as_ptr().unwrap();
        let right = right.as_ptr().unwrap();
        match element_equal(py, generic_alias_origin_bits(left), generic_alias_origin_bits(right)) {
            Ok(false) => return equality(Ok(false), op),
            Err(()) => return CompareValueOutcome::Error,
            Ok(true) => {},
        }
        // GenericAlias retains tuple subclasses supplied as args, so their
        // __eq__ result is a value, not a truth consumer. CPython's __ne__
        // tests that result against True by identity without invoking __bool__.
        let equal = compare_object_eq_value(
            py, obj_from_bits(generic_alias_args_bits(left)), obj_from_bits(generic_alias_args_bits(right)),
        );
        if op == RichCompareOp::Eq { return equal; }
        match equal {
            CompareValueOutcome::Value(bits) => {
                let is_true = bits == MoltObject::from_bool(true).bits();
                dec_ref_bits(py, bits);
                if exception_pending(py) { CompareValueOutcome::Error } else { boolean(!is_true) }
            }
            outcome => outcome,
        }
    }
}

pub(super) fn union(
    py: &PyToken<'_>, left: MoltObject, right: MoltObject, op: RichCompareOp,
) -> CompareValueOutcome {
    if !op.is_equality() || physical_type(right) != Some(TYPE_ID_UNION) {
        return CompareValueOutcome::NotComparable;
    }
    let Some(_guard) = crate::state::recursion::RecursionGuard::enter_with_message(
        py, "maximum recursion depth exceeded in comparison",
    ) else { return CompareValueOutcome::Error; };
    let as_set = |value: MoltObject| -> Option<Pin<'_, '_>> {
        unsafe {
            let args = obj_from_bits(union_type_args_bits(value.as_ptr()?)).as_ptr()?;
            let args = crate::object::seq_access::pin_tuple(py, args)?;
            let set = crate::object::builders::alloc_set_like_with_entries(py, &args, TYPE_ID_FROZENSET);
            if set.is_null() { return None; }
            Some(Pin::adopt(py, MoltObject::from_ptr(set).bits()))
        }
    };
    let Some(left) = as_set(left) else { return CompareValueOutcome::Error; };
    let Some(right) = as_set(right) else { return CompareValueOutcome::Error; };
    // Union members are unordered. Reuse the hashed set contract, including
    // member hash failures, instead of treating the argument tuple as ordered.
    BuiltinComparison::Frozenset.compare(py, obj_from_bits(left.bits), obj_from_bits(right.bits), op)
}
