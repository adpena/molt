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
    // A split continuation can itself be reached only through an exception
    // observation. Ordinary block dominance would miss every use downstream;
    // block-only full dominance also loses the observation's program position.
    // Rename only where the continuation's arguments reach every execution path.
    let dominance = crate::tir::dominators::ProgramPointDominance::compute_executable(func);
    let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    for bid in blocks {
        let block = func.blocks.get_mut(&bid).unwrap();
        for (index, op) in block.ops.iter_mut().enumerate() {
            if dominance.definition_available(continuation, None, bid, index) {
                for operand in &mut op.operands {
                    if let Some(new_value) = remap.get(operand).copied() {
                        *operand = new_value;
                    }
                }
            }
        }
        if dominance.definition_available(continuation, None, bid, usize::MAX) {
            block.terminator = remap_terminator_values(&block.terminator, remap);
        }
    }
}
