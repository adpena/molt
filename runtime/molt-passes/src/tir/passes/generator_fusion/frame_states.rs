//! The poll prepared for its frame states (docs/design/foundation/
//! spliced_frames.md 5.2).
//!
//! On a target that tests pending work, each asynchronous-work poll of the
//! body services work only when some is pending (`AsyncWorkPending`), in a
//! cold service block. The clone brackets the poll there, so the generator's
//! record is linked only while a signal handler may run, and the hot path
//! pays the test the unspliced poll pays. The rewrite runs on the linearized
//! poll before the slot plan, so the plan sees the service block as ordinary
//! control flow.

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::types::TirType;

use super::clone::is_bookkeeping_op;

/// The index of each poll of `block` that may move to a service block: one
/// the clone keeps (the check observing a dropped bookkeeping call goes with
/// that call) and whose only job is the work. A poll with an Origin edge also
/// observes the exception of the operation before it (a call's return), and
/// its lowering tests that exception on the path with no work pending, so it
/// stays where it is.
fn kept_polls(block: &TirBlock) -> Vec<usize> {
    let mut polls = Vec::new();
    let mut after_dropped_call = false;
    for (index, op) in block.ops.iter().enumerate() {
        if matches!(op.opcode, OpCode::StateSwitch | OpCode::StateSet) || is_bookkeeping_op(op) {
            after_dropped_call = op.runtime.is_some();
            continue;
        }
        if std::mem::take(&mut after_dropped_call) && op.opcode == OpCode::CheckException {
            continue;
        }
        if op.opcode == OpCode::CheckException
            && op.is_async_work_poll()
            && op.exception_edge_kind() == Some(ExceptionEdgeKind::Unwind)
        {
            polls.push(index);
        }
    }
    polls
}

/// `poll`, linearized, with each poll of its body moved to a service block
/// entered only when work is pending (`split_polls`: the target lowers
/// `AsyncWorkPending`); the rest of its block continues in a fresh block both
/// paths reach.
pub(super) fn prepare_poll(poll: &TirFunction, split_polls: bool) -> TirFunction {
    let mut poll = poll.clone();
    if !split_polls {
        return poll;
    }
    let mut blocks: Vec<BlockId> = poll.blocks.keys().copied().collect();
    blocks.sort_unstable();
    for bid in blocks {
        let polls = kept_polls(&poll.blocks[&bid]);
        if polls.is_empty() {
            continue;
        }
        let TirBlock {
            id,
            args,
            ops,
            terminator,
        } = poll.blocks.remove(&bid).expect("listed block");
        let mut current = id;
        let mut current_args = args;
        let mut current_ops = Vec::with_capacity(ops.len() + 1);
        for (index, op) in ops.into_iter().enumerate() {
            if !polls.contains(&index) {
                current_ops.push(op);
                continue;
            }
            let pending = poll.fresh_value();
            poll.value_types.insert(pending, TirType::Bool);
            let mut test = TirOp::new(OpCode::AsyncWorkPending, vec![], vec![pending]);
            test.source_span = op.source_span;
            current_ops.push(test);
            let service = poll.fresh_block();
            let rest = poll.fresh_block();
            poll.blocks.insert(
                current,
                TirBlock {
                    id: current,
                    args: std::mem::take(&mut current_args),
                    ops: std::mem::take(&mut current_ops),
                    terminator: Terminator::CondBranch {
                        cond: pending,
                        then_block: service,
                        then_args: vec![],
                        else_block: rest,
                        else_args: vec![],
                    },
                },
            );
            poll.blocks.insert(
                service,
                TirBlock {
                    id: service,
                    args: vec![],
                    ops: vec![op],
                    terminator: Terminator::Branch {
                        target: rest,
                        args: vec![],
                    },
                },
            );
            current = rest;
        }
        poll.blocks.insert(
            current,
            TirBlock {
                id: current,
                args: current_args,
                ops: current_ops,
                terminator,
            },
        );
    }
    poll
}
