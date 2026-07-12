use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::AttrValue;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::classify::{const_int_values, op_clears_pending_exception, op_may_raise};

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
            let mut successors = Vec::with_capacity(cases.len() + 1);
            successors.push(*default);
            successors.extend(cases.iter().map(|(_, target, _)| *target));
            successors
        }
        Terminator::Return { .. } | Terminator::Unreachable => Vec::new(),
    }
}

fn exception_target_blocks(func: &TirFunction) -> HashSet<BlockId> {
    let label_to_block: HashMap<i64, BlockId> = func
        .label_id_map
        .iter()
        .map(|(&bid, &label)| (label, BlockId(bid)))
        .collect();
    let mut targets = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if dominators::is_exception_transfer_edge(op.opcode)
                && let Some(AttrValue::Int(label)) = op.attrs.get("value")
                && let Some(&target) = label_to_block.get(label)
            {
                targets.insert(target);
            }
        }
    }
    targets
}

fn transfer_block_pending(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    block: &TirBlock,
    mut pending: bool,
) -> bool {
    for op in &block.ops {
        if op.opcode == crate::tir::ops::OpCode::CheckException {
            if pending {
                pending = false;
            }
            continue;
        }
        if op_clears_pending_exception(op) {
            pending = false;
        } else if op_may_raise(value_types, const_ints, op) {
            pending = true;
        }
    }
    pending
}

pub(super) fn compute_block_entry_pending(func: &TirFunction) -> HashMap<BlockId, bool> {
    let exception_targets = exception_target_blocks(func);
    let const_ints = const_int_values(func);
    let value_types = func.value_types.clone();
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
            let exits_pending =
                transfer_block_pending(&value_types, &const_ints, block, starts_pending);
            if !exits_pending {
                continue;
            }
            for succ in terminator_successors(&block.terminator) {
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
