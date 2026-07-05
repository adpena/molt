use std::collections::HashMap;

use crate::tir::analysis::LoopForestResult;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

/// Whole-function map of `ValueId -> i64` for every `ConstInt` definition.
pub(super) fn build_const_int_map(func: &TirFunction) -> HashMap<ValueId, i64> {
    let mut map = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt
                && op.results.len() == 1
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                map.insert(op.results[0], *v);
            }
        }
    }
    map
}

/// Locate the op defining `value` anywhere in the function (block, op).
pub(super) fn find_def(func: &TirFunction, value: ValueId) -> Option<(BlockId, &TirOp)> {
    for (&bid, block) in &func.blocks {
        for op in &block.ops {
            if op.results.first() == Some(&value) {
                return Some((bid, op));
            }
        }
    }
    None
}

pub(super) fn loop_forest_contains_header(loop_forest: &LoopForestResult, header: BlockId) -> bool {
    loop_forest
        .headers
        .binary_search_by_key(&header.0, |b| b.0)
        .is_ok()
}

/// True if `block` unconditionally branches back to `header` (the back-edge).
pub(super) fn block_loops_back_to(func: &TirFunction, block: BlockId, header: BlockId) -> bool {
    func.blocks.get(&block).is_some_and(
        |b| matches!(&b.terminator, Terminator::Branch { target, .. } if *target == header),
    )
}

/// The argument list `term` passes to `target` along whichever edge reaches it,
/// or `None` if `term` does not branch to `target`.
pub(super) fn branch_args_to(term: &Terminator, target: BlockId) -> Option<&[ValueId]> {
    match term {
        Terminator::Branch { target: t, args } if *t == target => Some(args.as_slice()),
        Terminator::CondBranch {
            then_block,
            then_args,
            else_block,
            else_args,
            ..
        } => {
            if *then_block == target {
                Some(then_args.as_slice())
            } else if *else_block == target {
                Some(else_args.as_slice())
            } else {
                None
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
        } => {
            if *default == target {
                Some(default_args.as_slice())
            } else {
                cases
                    .iter()
                    .find_map(|(_, b, args)| (*b == target).then_some(args.as_slice()))
            }
        }
        _ => None,
    }
}
