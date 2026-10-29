//! LIR edge policy for the shared Semi-NCA dominator core.

use std::collections::HashMap;

use super::super::blocks::BlockId;
use super::super::dominators::{DominanceGraph, DominatorTree, exception_labels_to_blocks};
use super::super::lir::{LirBlock, LirFunction};
use super::super::ops::AttrValue;

struct LirGraph<'a> {
    func: &'a LirFunction,
    labels: HashMap<i64, BlockId>,
}

impl DominanceGraph for LirGraph<'_> {
    fn entry_block(&self) -> BlockId {
        self.func.entry_block
    }
    fn block_count(&self) -> usize {
        self.func.blocks.len()
    }
    fn append_successors(&self, block: BlockId, output: &mut Vec<BlockId>) -> bool {
        let Some(block) = self.func.blocks.get(&block) else {
            return false;
        };
        output.extend(block.terminator.successors());
        output.extend(exception_successors(block, &self.labels));
        true
    }
}

pub(super) fn compute_dominator_tree(func: &LirFunction) -> DominatorTree {
    DominatorTree::compute(&LirGraph {
        func,
        labels: exception_labels_to_blocks(&func.label_id_map),
    })
}

/// Return the implicit successors of `block` that are reached only via
/// exception flow — encoded by `CheckException`/`TryStart` ops with a `value`
/// attr giving the target label_id. `TryEnd` carries pairing metadata, not a
/// handler-transfer edge. The LIR verifier needs to follow real transfer edges
/// so that exception handler blocks are considered reachable from the function
/// entry; otherwise their value uses appear to violate dominance even though at
/// runtime control flow correctly reaches them via the runtime exception path.
fn exception_successors(block: &LirBlock, label_to_block: &HashMap<i64, BlockId>) -> Vec<BlockId> {
    let mut successors = Vec::new();
    for op in &block.ops {
        if crate::tir::dominators::is_exception_transfer_edge(op.tir_op.opcode)
            && let Some(AttrValue::Int(target_label)) = op.tir_op.attrs.get("value")
            && let Some(&target) = label_to_block.get(target_label)
        {
            successors.push(target);
        }
    }
    successors
}
