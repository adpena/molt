use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, TirBlock};
use crate::tir::dominators;
use crate::tir::function::TirFunction;

use super::classify::{
    RaiseFacts, check_has_exception_edge, op_clears_pending_exception, pending_after_check,
};

fn exception_target_blocks(func: &TirFunction, labels: &HashMap<i64, BlockId>) -> HashSet<BlockId> {
    func.blocks
        .values()
        .flat_map(|block| &block.ops)
        .filter_map(|op| dominators::exception_transfer_target(op, labels))
        .collect()
}

fn transfer_block_pending(
    raise: &RaiseFacts,
    block: &TirBlock,
    labels: &HashMap<i64, BlockId>,
    mut pending: bool,
) -> bool {
    for op in &block.ops {
        if op.opcode == crate::tir::ops::OpCode::CheckException {
            pending = pending_after_check(check_has_exception_edge(op, labels), op, pending);
            continue;
        }
        if op_clears_pending_exception(op) {
            pending = false;
        } else if raise.op_may_raise(op) {
            pending = true;
        }
    }
    pending
}

pub(super) fn compute_block_entry_pending(
    func: &TirFunction,
    raise: &RaiseFacts,
    labels: &HashMap<i64, BlockId>,
) -> HashMap<BlockId, bool> {
    let exception_targets = exception_target_blocks(func, labels);
    let mut entry_pending: HashMap<BlockId, bool> = func
        .blocks
        .keys()
        .copied()
        .map(|bid| (bid, false))
        .collect();
    entry_pending.insert(func.entry_block, true);
    for target in &exception_targets {
        entry_pending.insert(*target, true);
    }

    loop {
        let mut next: HashMap<BlockId, bool> = func
            .blocks
            .keys()
            .copied()
            .map(|bid| (bid, false))
            .collect();
        next.insert(func.entry_block, true);
        for target in &exception_targets {
            next.insert(*target, true);
        }

        for (bid, block) in &func.blocks {
            let starts_pending = entry_pending.get(bid).copied().unwrap_or(false);
            let exits_pending = transfer_block_pending(raise, block, labels, starts_pending);
            if !exits_pending {
                continue;
            }
            for succ in dominators::terminator_successors(&block.terminator) {
                if func.blocks.contains_key(&succ) {
                    next.insert(succ, true);
                }
            }
        }

        if next == entry_pending {
            return entry_pending;
        }
        entry_pending = next;
    }
}
