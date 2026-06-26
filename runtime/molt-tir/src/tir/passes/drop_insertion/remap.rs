use std::collections::HashMap;

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::TirOp;
use crate::tir::values::ValueId;

fn remap_value(value: ValueId, remap: &HashMap<ValueId, ValueId>) -> ValueId {
    remap.get(&value).copied().unwrap_or(value)
}

pub(super) fn remap_op_operands(op: &TirOp, remap: &HashMap<ValueId, ValueId>) -> TirOp {
    let mut out = op.clone();
    out.operands = out
        .operands
        .iter()
        .map(|&value| remap_value(value, remap))
        .collect();
    out
}

pub(super) fn remap_terminator_values(
    term: &Terminator,
    remap: &HashMap<ValueId, ValueId>,
) -> Terminator {
    let remap_values = |values: &[ValueId]| -> Vec<ValueId> {
        values
            .iter()
            .map(|&value| remap_value(value, remap))
            .collect()
    };
    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: *target,
            args: remap_values(args),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: remap_value(*cond, remap),
            then_block: *then_block,
            then_args: remap_values(then_args),
            else_block: *else_block,
            else_args: remap_values(else_args),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: remap_value(*value, remap),
            cases: cases
                .iter()
                .map(|(case, target, args)| (*case, *target, remap_values(args)))
                .collect(),
            default: *default,
            default_args: remap_values(default_args),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => Terminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(case, target, args)| (*case, *target, remap_values(args)))
                .collect(),
            default: *default,
            default_args: remap_values(default_args),
        },
        Terminator::Return { values } => Terminator::Return {
            values: remap_values(values),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

pub(super) fn remap_uses_dominated_by_split_continuation(
    func: &mut TirFunction,
    continuation: BlockId,
    remap: &HashMap<ValueId, ValueId>,
) {
    if remap.is_empty() {
        return;
    }
    let pred_map = crate::tir::dominators::build_pred_map_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
    );
    let idoms = crate::tir::dominators::compute_idoms_with(
        func,
        &pred_map,
        crate::tir::dominators::CfgEdgePolicy::TerminatorOnly,
    );
    let mut dominated_blocks: Vec<BlockId> = func
        .blocks
        .keys()
        .copied()
        .filter(|block| crate::tir::dominators::dominates(continuation, *block, &idoms))
        .collect();
    dominated_blocks.sort_unstable_by_key(|block| block.0);

    for bid in dominated_blocks {
        let Some(block) = func.blocks.get_mut(&bid) else {
            continue;
        };
        for op in &mut block.ops {
            for operand in &mut op.operands {
                if let Some(new_value) = remap.get(operand).copied() {
                    *operand = new_value;
                }
            }
        }
        block.terminator = remap_terminator_values(&block.terminator, remap);
    }
}
