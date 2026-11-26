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

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::ExceptionEdgeKind;
use crate::tir::ops::{OpCode, TirOp};

use super::clone::is_bookkeeping_op;

/// The splice removes its state machinery along with the exception check
/// immediately following a removed bookkeeping call.
pub(super) fn skip_frame_op(op: &TirOp, after_dropped_call: &mut bool) -> bool {
    if matches!(op.opcode, OpCode::StateSwitch | OpCode::StateSet) || is_bookkeeping_op(op) {
        *after_dropped_call = op.runtime.is_some();
        return true;
    }
    std::mem::take(after_dropped_call) && op.opcode == OpCode::CheckException
}

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
        if skip_frame_op(op, &mut after_dropped_call) {
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
        for index in polls.into_iter().rev() {
            let check = poll.blocks[&bid].ops[index].clone();
            super::super::async_work_poll::split_pending_work(
                &mut poll,
                bid,
                index,
                vec![check],
                vec![],
            );
        }
    }
    poll
}
