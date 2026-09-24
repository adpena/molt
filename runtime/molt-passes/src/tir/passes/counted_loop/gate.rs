use std::collections::HashSet;

use crate::tir::blocks::{BlockId, LoopBreakKind, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_counted_loop_comparison_role_table;
use crate::tir::values::ValueId;

pub(super) struct LoopGate {
    pub(super) cmp_cond: ValueId,
    pub(super) cmp_polarity: CmpPolarity,
    pub(super) body_id: BlockId,
    pub(super) exit_id: BlockId,
    pub(super) exit_args: Vec<ValueId>,
    pub(super) has_material_exit: bool,
}

#[derive(Clone, Copy)]
pub(super) enum CmpPolarity {
    AsWritten,
    Inverted,
}

pub(super) fn loop_gate(
    func: &TirFunction,
    header: BlockId,
    guard_path: &[BlockId],
    loop_body: &HashSet<BlockId>,
) -> Option<LoopGate> {
    let cond_block_id = *guard_path.last()?;
    let cond_block = func.blocks.get(&cond_block_id)?;
    match &cond_block.terminator {
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let then_loops = loop_body.contains(then_block);
            let else_loops = loop_body.contains(else_block);
            match (then_loops, else_loops) {
                (true, false) => {
                    if !then_args.is_empty() {
                        return None;
                    }
                    Some(LoopGate {
                        cmp_cond: *cond,
                        cmp_polarity: CmpPolarity::AsWritten,
                        body_id: *then_block,
                        exit_id: *else_block,
                        exit_args: else_args.clone(),
                        has_material_exit: true,
                    })
                }
                (false, true) => {
                    if !else_args.is_empty() {
                        return None;
                    }
                    Some(LoopGate {
                        cmp_cond: *cond,
                        cmp_polarity: CmpPolarity::Inverted,
                        body_id: *else_block,
                        exit_id: *then_block,
                        exit_args: then_args.clone(),
                        has_material_exit: true,
                    })
                }
                _ => None,
            }
        }
        Terminator::Branch { target, args } if args.is_empty() => {
            structured_terminal_loop_gate(func, header, guard_path, *target)
        }
        Terminator::Branch { .. }
        | Terminator::Switch { .. }
        | Terminator::StateDispatch { .. }
        | Terminator::Return { .. }
        | Terminator::Unreachable => None,
    }
}

fn structured_terminal_loop_gate(
    func: &TirFunction,
    header: BlockId,
    guard_path: &[BlockId],
    body_id: BlockId,
) -> Option<LoopGate> {
    let cond_block_id = *guard_path.last()?;
    if func.loop_cond_blocks.get(&header).copied() != Some(cond_block_id) {
        return None;
    }
    let break_kind = func.loop_break_kinds.get(&header)?;
    let cmp_cond = unique_loop_guard_cmp_cond(func, guard_path)?;
    let cmp_polarity = match break_kind {
        LoopBreakKind::BreakIfFalse => CmpPolarity::AsWritten,
        LoopBreakKind::BreakIfTrue => CmpPolarity::Inverted,
    };
    Some(LoopGate {
        cmp_cond,
        cmp_polarity,
        body_id,
        exit_id: func
            .loop_pairs
            .get(&header)
            .copied()
            .unwrap_or(cond_block_id),
        exit_args: Vec::new(),
        has_material_exit: false,
    })
}

fn unique_loop_guard_cmp_cond(func: &TirFunction, guard_path: &[BlockId]) -> Option<ValueId> {
    let mut guard: Option<ValueId> = None;
    for op in guard_path.iter().flat_map(|bid| &func.blocks[bid].ops) {
        if opcode_counted_loop_comparison_role_table(op.opcode).is_ordered()
            && op.results.len() == 1
            && guard.replace(op.results[0]).is_some()
        {
            return None;
        }
    }
    guard
}
