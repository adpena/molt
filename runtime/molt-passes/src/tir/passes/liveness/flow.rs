use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::values::ValueId;

/// The successors of `block` under the terminator-only CFG (the edges that carry
/// SSA values via block args). Exception edges are handled by the drop pass's
/// CheckException logic, not by liveness propagation — at this analysis layer a
/// value live across a potentially-throwing op is captured by ordinary
/// straight-line liveness (the op is just another op in the block).
fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. }
        | Terminator::StateDispatch { cases, default, .. } => {
            let mut out: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
            out.push(*default);
            out
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

/// The values `term` *uses* directly (the condition of a CondBranch, the switch
/// value, and Return values) — NOT the branch args, which are handled by the
/// successor block-arg propagation in [`live_out_of`].
pub(super) fn terminator_direct_uses(term: &Terminator) -> Vec<ValueId> {
    match term {
        Terminator::Branch { .. } => vec![],
        Terminator::CondBranch { cond, .. } => vec![*cond],
        Terminator::Switch { value, .. } => vec![*value],
        // `StateDispatch` reads the saved state from the frame header at codegen
        // time, not an SSA value — it has no direct value use.
        Terminator::StateDispatch { .. } => vec![],
        Terminator::Return { values } => values.clone(),
        Terminator::Unreachable => vec![],
    }
}

/// For an edge `B → S`, the values `B` passes to bind `S`'s block args (indexed
/// by arg position). Returns the args delivered specifically to successor `succ`
/// on the matching edge.
fn edge_args_to(term: &Terminator, succ: BlockId) -> Vec<ValueId> {
    match term {
        Terminator::Branch { target, args } if *target == succ => args.clone(),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            let mut out = Vec::new();
            if *then_block == succ {
                out.extend(then_args.iter().copied());
            }
            if *else_block == succ {
                out.extend(else_args.iter().copied());
            }
            out
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        }
        | Terminator::StateDispatch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut out = Vec::new();
            for (_, b, args) in cases {
                if *b == succ {
                    out.extend(args.iter().copied());
                }
            }
            if *default == succ {
                out.extend(default_args.iter().copied());
            }
            out
        }
        _ => vec![],
    }
}

/// Compute LiveOut[B] from the current LiveIn of B's successors.
///
/// A value `v` is live-out of `B` if some successor `S` needs it live on entry.
/// For block-argument SSA that means:
/// * `v` is live-in to `S` AND `v` is not one of `S`'s block args (block args are
///   defined at `S`'s entry — a `v` that aliases a block arg id is killed there,
///   not propagated back), OR
/// * `v` is passed by `B`'s edge to `S` as a block arg (an explicit use in `B`).
pub(super) fn live_out_of(
    block: &TirBlock,
    live_in: &HashMap<BlockId, HashSet<ValueId>>,
    block_args: &HashMap<BlockId, HashSet<ValueId>>,
    heap_carrying: &dyn Fn(ValueId) -> bool,
    canon: &dyn Fn(ValueId) -> ValueId,
    keepalive_roots: &dyn Fn(ValueId) -> Vec<ValueId>,
) -> HashSet<ValueId> {
    let mut out = HashSet::new();
    for succ in terminator_successors(&block.terminator) {
        if let Some(succ_in) = live_in.get(&succ) {
            let succ_args = block_args.get(&succ);
            for &v in succ_in {
                // `v` is already an alias root (live sets are in root space).
                let is_succ_arg = succ_args.is_some_and(|a| a.contains(&v));
                if !is_succ_arg {
                    out.insert(v);
                }
            }
        }
        // Edge args this block supplies to the successor's block args are direct
        // uses in this block — they are live-out of this block (their value must
        // survive to the branch). Canonicalize to the alias root so a copied edge
        // arg keeps its underlying object live.
        for v in edge_args_to(&block.terminator, succ) {
            if heap_carrying(v) {
                out.insert(canon(v));
            }
            // A borrow result forwarded on an edge keeps its source object live-out
            // of this block (the source must reach the successor where the borrow
            // is consumed). Design 20 interior-borrow keepalive.
            for src_root in keepalive_roots(v) {
                out.insert(src_root);
            }
        }
    }
    out
}
