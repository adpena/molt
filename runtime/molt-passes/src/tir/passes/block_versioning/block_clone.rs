use std::collections::HashMap;

use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::TirOp;
use crate::tir::values::{TirValue, ValueId};

/// Clone a block, remapping all ValueIds using a fresh-value allocator.
/// Returns the cloned block and a mapping from old ValueId -> new ValueId.
pub(super) fn clone_block_with_fresh_values(
    block: &TirBlock,
    new_block_id: BlockId,
    func: &mut TirFunction,
) -> (TirBlock, HashMap<ValueId, ValueId>) {
    let mut remap: HashMap<ValueId, ValueId> = HashMap::new();

    // Allocate fresh IDs for block arguments.
    let new_args: Vec<TirValue> = block
        .args
        .iter()
        .map(|arg| {
            let new_id = func.fresh_value();
            remap.insert(arg.id, new_id);
            TirValue {
                id: new_id,
                ty: arg.ty.clone(),
            }
        })
        .collect();

    // Allocate fresh IDs for op results first (so operand remapping can find them).
    let mut new_ops: Vec<TirOp> = Vec::with_capacity(block.ops.len());
    // First pass: allocate result IDs.
    let mut result_ids: Vec<Vec<ValueId>> = Vec::with_capacity(block.ops.len());
    for op in &block.ops {
        let new_results: Vec<ValueId> = op
            .results
            .iter()
            .map(|&r| {
                let new_id = func.fresh_value();
                remap.insert(r, new_id);
                new_id
            })
            .collect();
        result_ids.push(new_results);
    }

    // Second pass: remap operands and build ops.
    for (op, new_results) in block.ops.iter().zip(result_ids) {
        let new_operands: Vec<ValueId> = op
            .operands
            .iter()
            .map(|&v| *remap.get(&v).unwrap_or(&v))
            .collect();
        new_ops.push(TirOp {
            dialect: op.dialect,
            opcode: op.opcode,
            operands: new_operands,
            results: new_results,
            attrs: op.attrs.clone(),
            source_span: op.source_span,
        });
    }

    // Remap terminator.
    let new_terminator = remap_terminator(&block.terminator, &remap);

    let new_block = TirBlock {
        id: new_block_id,
        args: new_args,
        ops: new_ops,
        terminator: new_terminator,
    };

    (new_block, remap)
}

/// Remap ValueIds in a terminator. BlockIds are NOT remapped (the clone
/// targets the same successor blocks as the original).
fn remap_terminator(term: &Terminator, remap: &HashMap<ValueId, ValueId>) -> Terminator {
    let r = |v: &ValueId| -> ValueId { *remap.get(v).unwrap_or(v) };

    match term {
        Terminator::Branch { target, args } => Terminator::Branch {
            target: *target,
            args: args.iter().map(&r).collect(),
        },
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Terminator::CondBranch {
            cond: r(cond),
            then_block: *then_block,
            then_args: then_args.iter().map(&r).collect(),
            else_block: *else_block,
            else_args: else_args.iter().map(&r).collect(),
        },
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => Terminator::Switch {
            value: r(value),
            cases: cases
                .iter()
                .map(|(v, bid, args)| (*v, *bid, args.iter().map(&r).collect()))
                .collect(),
            default: *default,
            default_args: default_args.iter().map(&r).collect(),
        },
        Terminator::StateDispatch {
            cases,
            default,
            default_args,
        } => Terminator::StateDispatch {
            cases: cases
                .iter()
                .map(|(s, bid, args)| (*s, *bid, args.iter().map(&r).collect()))
                .collect(),
            default: *default,
            default_args: default_args.iter().map(&r).collect(),
        },
        Terminator::Return { values } => Terminator::Return {
            values: values.iter().map(r).collect(),
        },
        Terminator::Unreachable => Terminator::Unreachable,
    }
}

/// Rewrite a terminator to redirect edges from `old_target` to `new_target`,
/// also remapping branch arguments through `arg_remap`.
pub(super) fn redirect_terminator(
    term: &mut Terminator,
    old_target: BlockId,
    new_target: BlockId,
    arg_remap: &HashMap<ValueId, ValueId>,
) {
    let remap_args = |args: &mut Vec<ValueId>| {
        for a in args.iter_mut() {
            if let Some(&new_v) = arg_remap.get(a) {
                *a = new_v;
            }
        }
    };

    match term {
        Terminator::Branch { target, args } => {
            if *target == old_target {
                *target = new_target;
                remap_args(args);
            }
        }
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == old_target {
                *then_block = new_target;
                remap_args(then_args);
            }
            if *else_block == old_target {
                *else_block = new_target;
                remap_args(else_args);
            }
        }
        Terminator::Switch {
            cases,
            default,
            default_args,
            ..
        }
        | Terminator::StateDispatch {
            cases,
            default,
            default_args,
            ..
        } => {
            for (_, target, args) in cases.iter_mut() {
                if *target == old_target {
                    *target = new_target;
                    remap_args(args);
                }
            }
            if *default == old_target {
                *default = new_target;
                remap_args(default_args);
            }
        }
        Terminator::Return { .. } | Terminator::Unreachable => {}
    }
}

// ---------------------------------------------------------------------------
