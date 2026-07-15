use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::values::ValueId;

/// Collect (target_block, arg_values) edges from a terminator.
pub(super) fn collect_branch_edges(block: &TirBlock) -> Vec<(BlockId, Vec<ValueId>)> {
    let mut edges = Vec::new();
    block
        .terminator
        .for_each_edge(|target, args| edges.push((target, args.to_vec())));
    edges
}
