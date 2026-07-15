//! Pure CFG-terminator utilities shared across the module-slot-promotion
//! transform: successor enumeration, value-use scans, block-arg appends, edge
//! argument extraction, and edge retargeting. Representation-only helpers over
//! [`Terminator`] with no promotion policy of their own.

use std::collections::HashSet;

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::values::ValueId;

pub(super) fn terminator_uses(term: &Terminator, set: &HashSet<ValueId>) -> bool {
    let mut found = false;
    term.for_each_value(|value| found |= set.contains(&value));
    found
}

/// Retarget every appearance of `header` in `term` to `args`-augmented form:
/// append `extra` to the arg list of each edge into the header.
pub(super) fn append_args_on_edges_to(term: &mut Terminator, header: BlockId, extra: &[ValueId]) {
    term.for_each_edge_mut(|target, args| {
        if *target == header {
            args.extend_from_slice(extra);
        }
    });
}

pub(super) fn rewrite_terminator_values(term: &mut Terminator, f: &dyn Fn(ValueId) -> ValueId) {
    term.for_each_value_mut(|value| *value = f(*value));
}

/// The args `term` passes on its edge to `to` (first matching edge).
pub(super) fn edge_args(term: &Terminator, to: BlockId) -> Vec<ValueId> {
    term.first_edge_args_to(to).unwrap_or_default().to_vec()
}

/// Retarget every edge in `term` from `old` to `new`, clearing the edge args
/// (the new edge block forwards the originals itself).
pub(super) fn retarget_edge(term: &mut Terminator, old: BlockId, new: BlockId) {
    term.for_each_edge_mut(|target, args| {
        if *target == old {
            *target = new;
            args.clear();
        }
    });
}
