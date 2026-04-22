//! Shared reachability helpers for TIR passes that remove blocks.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};

fn terminator_successors(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut successors = vec![*default];
            successors.extend(cases.iter().map(|(_, target, _)| *target));
            successors
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

fn exception_successors(
    block: &TirBlock,
    label_to_block: &HashMap<i64, BlockId>,
) -> Vec<BlockId> {
    let mut successors = Vec::new();
    for op in &block.ops {
        if matches!(
            op.opcode,
            OpCode::CheckException | OpCode::TryStart | OpCode::TryEnd
        ) && let Some(AttrValue::Int(target_label)) = op.attrs.get("value")
            && let Some(&target) = label_to_block.get(target_label)
        {
            successors.push(target);
        }
    }
    successors
}

/// Collect the blocks that must survive a block-removing pass.
///
/// This follows explicit terminator edges plus implicit exception edges encoded
/// by label-valued exception ops.  It also seeds structural loop-role blocks:
/// lower_to_simple depends on those metadata-carrying blocks even when a local
/// branch fold makes part of the textual loop shape temporarily unreachable.
pub(super) fn metadata_preserving_reachable_blocks(func: &TirFunction) -> HashSet<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut stack: Vec<BlockId> = vec![func.entry_block];
    for bid in func.loop_roles.keys().copied() {
        if bid != func.entry_block {
            stack.push(bid);
        }
    }

    let label_to_block: HashMap<i64, BlockId> = func
        .label_id_map
        .iter()
        .map(|(&bid, &label_id)| (label_id, BlockId(bid)))
        .collect();

    while let Some(id) = stack.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(block) = func.blocks.get(&id) else {
            continue;
        };
        stack.extend(terminator_successors(&block.terminator));
        stack.extend(exception_successors(block, &label_to_block));
    }

    visited
}
