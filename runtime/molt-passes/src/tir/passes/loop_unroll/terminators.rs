//! Pure CFG-terminator utilities shared by the loop-unroll transform.
//!
//! These are representation-only helpers over [`Terminator`]: they read or
//! rewrite the value references and successor edges of a terminator without any
//! knowledge of the unroll cost model or loop recognition. Keeping them in a
//! leaf module isolates the terminator match arms from the transform logic in
//! [`super`].

use std::collections::HashMap;

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::values::ValueId;

/// Every `ValueId` referenced by a terminator (condition + all branch args).
pub(super) fn terminator_value_refs(term: &Terminator) -> Vec<ValueId> {
    let mut refs = Vec::new();
    term.for_each_value(|value| refs.push(value));
    refs
}

/// Replace every value reference in `term` (condition + all branch args) that
/// appears as a key in `subst` with its mapped value.
pub(super) fn substitute_terminator_values(
    term: &mut Terminator,
    subst: &HashMap<ValueId, ValueId>,
) {
    fn remap(v: &mut ValueId, subst: &HashMap<ValueId, ValueId>) {
        if let Some(&nv) = subst.get(v) {
            *v = nv;
        }
    }
    term.for_each_value_mut(|value| remap(value, subst));
}

/// Extract the args passed to `header` from a predecessor terminator.
pub(super) fn header_args_from(term: &Terminator, header: BlockId) -> Option<&[ValueId]> {
    term.first_edge_args_to(header)
}

/// Returns `true` if the terminator references `target` as any successor.
pub(super) fn branches_to(term: &Terminator, target: BlockId) -> bool {
    term.has_successor(target)
}

/// Replace every successor reference to `from` with `to` in `term`. The landing
/// block has zero block arguments by construction, so we MUST also drop any
/// argument list that was being forwarded to `from` to keep TIR verification
/// (block-arg arity match) sound.
pub(super) fn redirect_terminator(term: &mut Terminator, from: BlockId, to: BlockId) {
    term.for_each_edge_mut(|target, args| {
        if *target == from {
            *target = to;
            args.clear();
        }
    });
}
