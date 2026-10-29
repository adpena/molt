use std::collections::HashSet;

use crate::tir::blocks::BlockId;
use crate::tir::dominators;
use crate::tir::function::TirFunction;

/// Reverse postorder of the `reachable` blocks, entry first. A block's
/// terminator successors come before its implicit exception successors (the
/// full-CFG edge set of the S1 dominator analyses, in memory-SSA's own visit
/// order, which numbers its phis). The shared iterative traversal keeps native
/// stack depth independent of function size.
pub(super) fn reverse_postorder(func: &TirFunction, reachable: &HashSet<BlockId>) -> Vec<BlockId> {
    let label_to_block = dominators::exception_label_to_block(func);
    let mut visited: HashSet<BlockId> = HashSet::with_capacity(reachable.len());
    dominators::reverse_postorder_by([func.entry_block], reachable.len(), |bid, successors| {
        if !reachable.contains(&bid) || !visited.insert(bid) {
            return false;
        }
        if let Some(block) = func.blocks.get(&bid) {
            successors.extend(dominators::terminator_successors(&block.terminator));
            successors.extend(dominators::exception_successors(block, &label_to_block));
            successors.retain(|next| !visited.contains(next));
        }
        true
    })
}
