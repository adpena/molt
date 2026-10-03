//! One authority for an operation's pending-exception observation.
//!
//! Async-work placement and ownership cleanup must agree on the exact point
//! that separates successful continuation from exceptional transfer.

use super::check_exception_elim::classify::{op_clears_pending_exception, op_may_raise};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;
use std::collections::{BTreeSet, HashMap};

pub(super) fn check_label(op: &TirOp) -> Option<i64> {
    if op.opcode != OpCode::CheckException {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

pub(super) const FINALLY_PENDING_OBSERVER: &str = "exception_finally_pending_observer";

pub(super) fn is_deferred_finally_observer(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy
        && matches!(
            op.attrs.get("_original_kind"),
            Some(AttrValue::Str(kind)) if kind == FINALLY_PENDING_OBSERVER
        )
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum PostOperationObservation {
    Check(BlockId, usize),
    DeferredFinally(BlockId, usize),
}

/// Locate the frontend-authored exception observation for an operation boundary.
///
/// Optimization and CFG construction may separate a call from its original
/// payload-bearing `CheckException` or split the observation into a unique
/// unconditional successor block. Traverse only operations the canonical
/// check-elimination oracle proves cannot raise or clear pending state, plus
/// unconditional fallthrough. Never cross another call, lexical transfer,
/// conditional edge, or cycle and incorrectly let one later check service two
/// semantic boundaries.
pub(super) fn post_operation_observation(
    func: &TirFunction,
    block_id: BlockId,
    operation_index: usize,
    target: Option<i64>,
    predecessors: &HashMap<BlockId, Vec<BlockId>>,
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
) -> Option<PostOperationObservation> {
    let mut current = block_id;
    let mut start = operation_index + 1;
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current) {
            return None;
        }
        let block = func.blocks.get(&current)?;
        for (index, op) in block.ops.iter().enumerate().skip(start) {
            if op.opcode == OpCode::CheckException {
                if op.is_async_work_poll() && check_label(op).is_none() {
                    return Some(PostOperationObservation::Check(current, index));
                }
                return check_label(op)
                    .filter(|label| target.is_none() || target == Some(*label))
                    .map(|_| PostOperationObservation::Check(current, index));
            }
            if is_deferred_finally_observer(op) {
                return Some(PostOperationObservation::DeferredFinally(current, index));
            }
            if crate::tir::dominators::is_exception_transfer_edge(op.opcode)
                || op_clears_pending_exception(op)
                || op_may_raise(value_types, const_ints, op)
            {
                return None;
            }
        }
        let Terminator::Branch { target, .. } = &block.terminator else {
            return None;
        };
        if predecessors.get(target).map(Vec::as_slice) != Some(&[current]) {
            return None;
        }
        current = *target;
        start = 0;
    }
}
