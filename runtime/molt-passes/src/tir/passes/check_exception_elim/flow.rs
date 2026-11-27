use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;

use super::classify::{
    PendingTransfer, RaiseFacts, check_has_exception_edge, check_pending_transfer,
    operation_pending_transfer,
};

fn block_pending_transfer(
    raise: &RaiseFacts,
    block: &TirBlock,
    labels: &HashMap<i64, BlockId>,
) -> PendingTransfer {
    // The last operation that overwrites the pending bit determines the
    // block's transfer. Earlier operations only matter at their own checks.
    block
        .ops
        .iter()
        .rev()
        .map(|op| {
            if op.opcode == OpCode::CheckException {
                check_pending_transfer(check_has_exception_edge(op, labels), op)
            } else {
                operation_pending_transfer(raise, op)
            }
        })
        .find(|&transfer| transfer != PendingTransfer::Preserve)
        .unwrap_or(PendingTransfer::Preserve)
}

pub(super) fn compute_block_entry_pending(
    func: &TirFunction,
    raise: &RaiseFacts,
    labels: &HashMap<i64, BlockId>,
) -> HashMap<BlockId, bool> {
    let mut entry_pending: HashMap<BlockId, bool> = func
        .blocks
        .keys()
        .copied()
        .map(|bid| (bid, false))
        .collect();
    entry_pending.insert(func.entry_block, true);
    for target in func
        .blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter_map(|op| dominators::exception_transfer_target(op, labels))
    {
        entry_pending.insert(target, true);
    }

    let mut preserves_pending = HashSet::default();
    let mut pending_exits = Vec::new();
    for (&id, block) in &func.blocks {
        let transfer = block_pending_transfer(raise, block, labels);
        if transfer == PendingTransfer::Preserve {
            preserves_pending.insert(id);
        }
        if transfer.apply(entry_pending[&id]) {
            pending_exits.push(id);
        }
    }
    while let Some(id) = pending_exits.pop() {
        func.blocks[&id].terminator.for_each_edge(|target, _| {
            if let Some(pending) = entry_pending.get_mut(&target)
                && !*pending
            {
                *pending = true;
                if preserves_pending.contains(&target) {
                    pending_exits.push(target);
                }
            }
        });
    }
    entry_pending
}
