use crate::tir::blocks::{BlockId, LoopBreakKind, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_counted_loop_comparison_role_table;
use crate::tir::values::ValueId;

use super::facts::block_loops_back_to;

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
    cond_block_id: BlockId,
    cond_block: &TirBlock,
) -> Option<LoopGate> {
    match &cond_block.terminator {
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let then_loops = block_loops_back_to(func, *then_block, header);
            let else_loops = block_loops_back_to(func, *else_block, header);
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
            structured_terminal_loop_gate(func, header, cond_block_id, cond_block, *target)
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
    cond_block_id: BlockId,
    cond_block: &TirBlock,
    body_id: BlockId,
) -> Option<LoopGate> {
    if func.loop_cond_blocks.get(&header).copied() != Some(cond_block_id) {
        return None;
    }
    if !block_loops_back_to(func, body_id, header) {
        return None;
    }
    let break_kind = func.loop_break_kinds.get(&header)?;
    let cmp_cond = unique_loop_guard_cmp_cond(cond_block)?;
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

fn unique_loop_guard_cmp_cond(cond_block: &TirBlock) -> Option<ValueId> {
    let mut guard: Option<ValueId> = None;
    for op in &cond_block.ops {
        if opcode_counted_loop_comparison_role_table(op.opcode).is_ordered()
            && op.results.len() == 1
            && guard.replace(op.results[0]).is_some()
        {
            return None;
        }
    }
    guard
}
