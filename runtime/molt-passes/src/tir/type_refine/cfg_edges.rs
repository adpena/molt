use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;
use std::collections::HashSet;

/// Exception targets and explicit handler entries require conservative inputs.
/// Ordinary labels and polls are not handler entries: the canonical edge
/// resolver identifies the target, rather than the block containing the poll.
pub(super) fn handler_entry_blocks(func: &TirFunction) -> HashSet<BlockId> {
    if !func.has_exception_handling {
        return HashSet::new();
    }
    let labels = crate::tir::dominators::exception_label_to_block(func);
    let mut handlers = HashSet::new();
    for block in func.blocks.values() {
        handlers.extend(crate::tir::dominators::exception_successors(block, &labels));
        if block
            .ops
            .first()
            .is_some_and(|op| op.opcode == OpCode::StateBlockStart)
        {
            handlers.insert(block.id);
        }
    }
    handlers
}

/// Collect (target_block, arg_values) edges from a terminator.
pub(super) fn collect_branch_edges(block: &TirBlock) -> Vec<(BlockId, Vec<ValueId>)> {
    let mut edges = Vec::new();
    block
        .terminator
        .for_each_edge(|target, args| edges.push((target, args.to_vec())));
    edges
}
