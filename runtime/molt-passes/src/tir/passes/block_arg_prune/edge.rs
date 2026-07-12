use std::collections::{BTreeMap, HashMap};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, AttrValue, OpCode};
use crate::tir::values::ValueId;

pub(super) fn retain_positions_not_in<T>(values: &mut Vec<T>, dead_positions: &[usize]) -> usize {
    if dead_positions.is_empty() || values.is_empty() {
        return 0;
    }
    let mut idx = 0usize;
    let before = values.len();
    values.retain(|_| {
        let keep = dead_positions.binary_search(&idx).is_err();
        idx += 1;
        keep
    });
    before - values.len()
}

fn prune_edge_args(
    target: BlockId,
    args: &mut Vec<ValueId>,
    prune: &BTreeMap<BlockId, Vec<usize>>,
) -> usize {
    prune
        .get(&target)
        .map(|dead| retain_positions_not_in(args, dead))
        .unwrap_or(0)
}

pub(super) fn prune_terminator_payloads(
    term: &mut Terminator,
    prune: &BTreeMap<BlockId, Vec<usize>>,
) -> usize {
    match term {
        Terminator::Branch { target, args } => prune_edge_args(*target, args, prune),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            prune_edge_args(*then_block, then_args, prune)
                + prune_edge_args(*else_block, else_args, prune)
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut removed = prune_edge_args(*default, default_args, prune);
            for (_, target, args) in cases {
                removed += prune_edge_args(*target, args, prune);
            }
            removed
        }
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
            ..
        } => {
            let mut removed = prune_edge_args(*default, default_args, prune);
            for (_, target, args) in cases {
                removed += prune_edge_args(*target, args, prune);
            }
            removed
        }
        Terminator::Return { .. } | Terminator::Unreachable => 0,
    }
}

pub(super) fn exception_transfer_target(
    op_opcode: OpCode,
    attrs: &AttrDict,
    label_to_block: &HashMap<i64, BlockId>,
) -> Option<BlockId> {
    if !dominators::is_exception_transfer_edge(op_opcode) {
        return None;
    }
    let Some(AttrValue::Int(label)) = attrs.get("value") else {
        return None;
    };
    label_to_block.get(label).copied()
}

pub(super) fn prune_exception_payloads(
    func: &mut TirFunction,
    prune: &BTreeMap<BlockId, Vec<usize>>,
) -> usize {
    let label_to_block = dominators::exception_label_to_block(func);
    let mut removed = 0usize;
    for block in func.blocks.values_mut() {
        for op in &mut block.ops {
            let Some(target) = exception_transfer_target(op.opcode, &op.attrs, &label_to_block)
            else {
                continue;
            };
            removed += prune_edge_args(target, &mut op.operands, prune);
        }
    }
    removed
}
