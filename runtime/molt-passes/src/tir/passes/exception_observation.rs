//! One authority for an operation's pending-exception observation.
//!
//! Async-work placement and ownership cleanup must agree on the exact point
//! that separates successful continuation from exceptional transfer.

use super::check_exception_elim::classify::{RaiseFacts, op_clears_pending_exception};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use molt_ir::deterministic_hash::HashMap;
use std::collections::BTreeSet;

pub(super) fn check_label(op: &TirOp) -> Option<i64> {
    if op.opcode != OpCode::CheckException {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(label)) => Some(*label),
        _ => None,
    }
}

/// A lexical region boundary: a check past it observes another region.
pub(super) fn closes_lexical_region(op: &TirOp) -> bool {
    crate::tir::op_kinds_generated::opcode_exception_region_nesting_role_table(op.opcode)
        != crate::tir::op_kinds_generated::ExceptionRegionNestingRole::None
}

pub(super) const FINALLY_PENDING_OBSERVER: molt_ir::OpKind =
    molt_ir::OpKind::ExceptionFinallyPendingObserver;

pub(super) fn is_deferred_finally_observer(op: &TirOp) -> bool {
    op.opcode == OpCode::Copy
        && matches!(
            op.origin,
            Some(kind) if kind == FINALLY_PENDING_OBSERVER
        )
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum PostOperationObservation {
    Check(BlockId, usize),
    /// Both paths observe the producer. Only the pending path services work.
    PolledOrigin {
        service: (BlockId, usize),
        fast: (BlockId, usize),
        continuation: BlockId,
    },
    DeferredFinally(BlockId, usize),
}

/// Recognize the paired Origins of a virtual frame. Both preserve the
/// producer's payload and converge on the same normal continuation. The
/// exception landings each forward their own incoming arguments to the same
/// original handler; one consumes linked records, the other publishes them.
fn split_origin(
    func: &TirFunction,
    gate: BlockId,
    predecessors: &HashMap<BlockId, Vec<BlockId>>,
) -> Option<PostOperationObservation> {
    use crate::tir::dominators::{exception_label_to_block, exception_transfer_target};
    use crate::tir::op_kinds_generated::ExceptionEdgeKind;
    let gate_block = &func.blocks[&gate];
    let Terminator::CondBranch {
        cond,
        then_block: service,
        else_block: fast,
        then_args,
        else_args,
    } = &gate_block.terminator
    else {
        return None;
    };
    let test = gate_block.ops.last()?;
    if test.opcode != OpCode::AsyncWorkPending
        || test.results != [*cond]
        || !then_args.is_empty()
        || !else_args.is_empty()
        || predecessors.get(service)?.as_slice() != [gate]
        || predecessors.get(fast)?.as_slice() != [gate]
    {
        return None;
    }
    let service_block = &func.blocks[service];
    let fast_block = &func.blocks[fast];
    let (marked_index, marked) =
        super::spliced_frame::poll_service(&service_block.ops, ExceptionEdgeKind::Origin)?;
    let [plain] = fast_block.ops.as_slice() else {
        return None;
    };
    if plain.opcode != OpCode::CheckException
        || plain.is_async_work_poll()
        || plain.exception_edge_kind() != Some(ExceptionEdgeKind::Origin)
        || plain.operands != marked.operands
        || plain.source_span != marked.source_span
    {
        return None;
    }
    let (
        Terminator::Branch {
            target: continuation,
            args: service_args,
        },
        Terminator::Branch {
            target: fast_target,
            args: fast_args,
        },
    ) = (&service_block.terminator, &fast_block.terminator)
    else {
        return None;
    };
    let incoming = predecessors.get(continuation)?;
    if continuation != fast_target
        || service_args != fast_args
        || incoming.len() != 2
        || !incoming.contains(service)
        || !incoming.contains(fast)
    {
        return None;
    }
    let labels = exception_label_to_block(func);
    let destination = |check: &TirOp| {
        let cold = &func.blocks[&exception_transfer_target(check, &labels)?];
        let Terminator::Branch { target, args } = &cold.terminator else {
            return None;
        };
        (cold.args.len() == check.exception_payload().len()
            && args.iter().copied().eq(cold.args.iter().map(|arg| arg.id)))
        .then_some(*target)
    };
    if destination(plain)? != destination(marked)? {
        return None;
    }
    Some(PostOperationObservation::PolledOrigin {
        service: (*service, marked_index),
        fast: (*fast, 0),
        continuation: *continuation,
    })
}

/// Locate the frontend-authored exception observation for an operation boundary.
///
/// Optimization and CFG construction may separate a call from its original
/// payload-bearing `CheckException` or split the observation into a unique
/// unconditional successor block. Traverse only operations the canonical
/// check-elimination oracle proves cannot raise or clear pending state, plus
/// unconditional fallthrough. Never cross another call, lexical transfer,
/// conditional edge, or cycle and incorrectly let one later check service two
/// semantic boundaries. A lexical region boundary (`TryStart`, `TryEnd`) also
/// stops the search, so the check found belongs to the operation's region and
/// its label is the authority. Region facts model `try` regions only; a
/// frame-restore cleanup scope (a class body's frame exit) is a label with no
/// region, and a poll aimed at the region handler instead skipped that cleanup
/// and left the class frame on the stack (HF-879).
pub(super) fn post_operation_observation(
    func: &TirFunction,
    block_id: BlockId,
    operation_index: usize,
    predecessors: &HashMap<BlockId, Vec<BlockId>>,
    raise: &RaiseFacts,
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
            if op.opcode == OpCode::AsyncWorkPending
                && index + 1 == block.ops.len()
                && let Some(split) = split_origin(func, current, predecessors)
            {
                return Some(split);
            }
            if op.opcode == OpCode::CheckException {
                if op.is_async_work_poll() && check_label(op).is_none() {
                    return Some(PostOperationObservation::Check(current, index));
                }
                return check_label(op).map(|_| PostOperationObservation::Check(current, index));
            }
            if is_deferred_finally_observer(op) {
                return Some(PostOperationObservation::DeferredFinally(current, index));
            }
            if crate::tir::dominators::is_exception_transfer_edge(op.opcode)
                || closes_lexical_region(op)
                || op_clears_pending_exception(op)
                || raise.op_may_raise(op)
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
