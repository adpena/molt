use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::executable_reverse_postorder;
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
// Executable block order and suspension layout
// ---------------------------------------------------------------------------

pub(super) fn lowering_block_order(
    func: &TirFunction,
    state_yield_resume_after: &HashMap<BlockId, BlockId>,
) -> Vec<BlockId> {
    // The shared order includes exception transfers. Appending their cleanup
    // blocks by allocation ID would place suffix-first release chains backward
    // in the output and manufacture redispatch edges in an otherwise acyclic CFG.
    let postorder = executable_reverse_postorder(func);
    let mut ordered: Vec<BlockId> = Vec::with_capacity(postorder.len());
    let mut emitted: HashSet<BlockId> = HashSet::new();
    for bid in postorder {
        // Keep a suspension chain adjacent even when full RPO visits saved-state
        // cases in reverse order. This follows only the existing layout relation,
        // not CFG successors. First placement wins for shared/cyclic resumes;
        // explicit state_targets, not adjacency, remain the execution authority.
        let mut next = Some(bid);
        while let Some(block) = next {
            if !emitted.insert(block) {
                break;
            }
            ordered.push(block);
            next = state_yield_resume_after.get(&block).copied();
        }
    }

    // Preserve non-executable region/state metadata deterministically. Actual
    // exception and resume destinations already participated in the shared order.
    if (func.has_exception_handling || func.has_state_machine())
        && ordered.len() < func.blocks.len()
    {
        let mut unreachable: Vec<BlockId> = func
            .blocks
            .keys()
            .filter(|bid| !emitted.contains(bid))
            .copied()
            .collect();
        // Sort for deterministic output.
        unreachable.sort_by_key(|bid| bid.0);
        ordered.extend(unreachable);
    }

    ordered
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
            for succ in blk.terminator.successors() {
                stack.push(succ);
            }
        }
    }
    false
}
