use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;

pub(super) fn collect_guard_raise_path_blocks(func: &TirFunction, start: BlockId) -> Vec<BlockId> {
    let mut raise_blocks = Vec::new();
    let mut cur = start;
    let mut visited: HashSet<BlockId> = HashSet::new();
    for _ in 0..3 {
        if !visited.insert(cur) {
            break;
        }
        raise_blocks.push(cur);
        let Some(blk) = func.blocks.get(&cur) else {
            break;
        };
        if let Terminator::Branch { target, .. } = &blk.terminator {
            cur = *target;
        } else {
            break;
        }
    }
    raise_blocks
}

// ---------------------------------------------------------------------------
// RPO traversal
// ---------------------------------------------------------------------------

pub(super) fn reverse_postorder(
    func: &TirFunction,
    state_yield_resume_after: &HashMap<BlockId, BlockId>,
) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut postorder: Vec<BlockId> = Vec::new();
    let mut stack: Vec<(BlockId, bool)> = vec![(func.entry_block, false)];

    while let Some((bid, processed)) = stack.pop() {
        if processed {
            postorder.push(bid);
            continue;
        }
        if visited.contains(&bid) {
            continue;
        }
        visited.insert(bid);
        stack.push((bid, true));

        if let Some(block) = func.blocks.get(&bid) {
            // Push successors in reverse order for correct DFS.
            let succs = match &block.terminator {
                Terminator::StateDispatch { default, .. } => vec![*default],
                _ => successors_of(block),
            };
            for succ in succs.into_iter().rev() {
                if !visited.contains(&succ) {
                    stack.push((succ, false));
                }
            }
        }
    }

    postorder.reverse();
    let mut ordered: Vec<BlockId> = Vec::with_capacity(postorder.len());
    let mut emitted: HashSet<BlockId> = HashSet::new();
    for bid in postorder {
        if emitted.insert(bid) {
            ordered.push(bid);
        }
        if let Some(&resume_bid) = state_yield_resume_after.get(&bid)
            && emitted.insert(resume_bid)
        {
            ordered.push(resume_bid);
        }
    }
    let mut postorder = ordered;

    // Append any blocks not reachable via normal control flow (e.g. exception
    // handler blocks only reachable via check_exception implicit edges, or
    // state-machine resume blocks only reachable via state_switch dispatch).
    // These must still appear in the output so the native backend can create
    // state_blocks for their labels.
    if (func.has_exception_handling || !state_yield_resume_after.is_empty())
        && postorder.len() < func.blocks.len()
    {
        let mut unreachable: Vec<BlockId> = func
            .blocks
            .keys()
            .filter(|bid| !emitted.contains(bid))
            .copied()
            .collect();
        // Sort for deterministic output.
        unreachable.sort_by_key(|bid| bid.0);
        postorder.extend(unreachable);
    }

    postorder
}

/// Reducibility probe for structured-loop body/exit polarity.
///
/// Returns `true` when the loop `header` is reachable from `start` through the
/// forward CFG WITHOUT re-entering the loop-controlling `cond_block`. For a
/// reducible natural loop the BODY successor of `cond_block` reaches the header
/// (it contains the back-edge), while the EXIT successor leaves the loop and
/// does not — so this distinguishes which cond successor is the loop body
/// independent of any (possibly stale, post-roundtrip) `break_kind` hint.
///
/// `cond_block` is excluded from traversal so the body→cond→{body,exit} cycle
/// cannot make the exit side appear to reach the header through the cond block.
/// `start == header` reaches trivially (a zero-body `while`-true backbone).
pub(super) fn successor_reaches_header(
    func: &TirFunction,
    start: BlockId,
    header: BlockId,
    cond_block: BlockId,
) -> bool {
    if start == header {
        return true;
    }
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut stack = vec![start];
    while let Some(b) = stack.pop() {
        if b == header {
            return true;
        }
        // Do not traverse back through the loop-controlling cond block: the
        // back-edge body reaches the header BEFORE returning to the cond, so a
        // genuine body successor is found without this edge; allowing it would
        // let the exit side reach the header via cond→body→header.
        if b == cond_block || !visited.insert(b) {
            continue;
        }
        if let Some(blk) = func.blocks.get(&b) {
            for succ in successors_of(blk) {
                stack.push(succ);
            }
        }
    }
    false
}

pub(super) fn successors_of(block: &TirBlock) -> Vec<BlockId> {
    match &block.terminator {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            let mut succs = vec![*default];
            for (_, target, _) in cases {
                succs.push(*target);
            }
            succs
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}
