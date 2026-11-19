//! The poll as fusion sees it: its resume edges made plain control flow.
//!
//! A generator poll starts at a `StateDispatch` terminator that enters the
//! body on a fresh start (its default) or at the resume block of the state
//! the last suspension saved (a case). Each `StateYield` saves its next state
//! (`value`) and ends its block in `unreachable`: the suspension leaves the
//! poll, and the next resume continues at that state's case.
//!
//! A fused body never suspends: the consumer body runs at the yield and the
//! generator continues where it would resume. So, for fusion, the yield's
//! block continues to its resume block and the dispatch enters only at its
//! default. This is the CFG the slot plan, the clone and the splice read: a
//! read after a resume sees the store before the yield that reaches it.

use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_generator_fusion_poll_role_table;
use crate::tir::ops::TirOp;

use super::attr_value_int;

/// A suspension, as the op-kind registry's poll roles name it.
fn is_yield(op: &TirOp) -> bool {
    opcode_generator_fusion_poll_role_table(op.opcode).is_required_yield()
}

/// `poll` with each yield's block continuing at its resume block and the
/// state dispatch entering at its default, or `None` when a dispatch case or a
/// yield does not take that shape: a yield that is not its block's last
/// operation before `unreachable`, a yield whose state no case resumes, or a
/// case that no yield saves.
pub(super) fn linearize_poll(poll: &TirFunction) -> Option<TirFunction> {
    let mut linear = poll.clone();
    let mut dispatches = linear
        .blocks
        .values()
        .filter(|block| matches!(block.terminator, Terminator::StateDispatch { .. }));
    let Some(dispatch) = dispatches.next().map(|block| block.id) else {
        // No resume edges: the poll is already plain control flow.
        return Some(linear);
    };
    if dispatches.next().is_some() {
        return None;
    }
    let Terminator::StateDispatch {
        cases,
        default,
        default_args,
    } = linear.blocks[&dispatch].terminator.clone()
    else {
        unreachable!("selected a dispatch");
    };
    let mut resumed = 0usize;
    let mut yield_blocks: Vec<_> = linear
        .blocks
        .values()
        .filter(|block| block.ops.iter().any(is_yield))
        .map(|block| block.id)
        .collect();
    yield_blocks.sort_unstable();
    for block in yield_blocks {
        let body = &linear.blocks[&block];
        let last = body.ops.last()?;
        if !is_yield(last)
            || body.ops.iter().filter(|op| is_yield(op)).count() != 1
            || !matches!(body.terminator, Terminator::Unreachable)
        {
            return None;
        }
        let state = attr_value_int(last)?;
        let (_, target, args) = cases.iter().find(|(case, _, _)| *case == state)?;
        if !args.is_empty() {
            return None;
        }
        resumed += 1;
        linear
            .blocks
            .get_mut(&block)
            .expect("yield block")
            .terminator = Terminator::Branch {
            target: *target,
            args: Vec::new(),
        };
    }
    if resumed != cases.len() {
        return None;
    }
    linear
        .blocks
        .get_mut(&dispatch)
        .expect("dispatch block")
        .terminator = Terminator::Branch {
        target: default,
        args: default_args,
    };
    Some(linear)
}
