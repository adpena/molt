//! Loop role and loop-condition detection for SimpleIR to TIR lowering.
//!
//! These helpers recover loop metadata from the original structural
//! SimpleIR stream after CFG construction, keeping the parent lowering file
//! focused on assembly orchestration.

use std::collections::HashMap;

use crate::ir::FunctionIR;

use super::super::blocks::{BlockId, LoopBreakKind, LoopRole};
use super::super::cfg::CFG;

/// Scan the original SimpleIR ops and CFG to detect which TIR blocks correspond
/// to `loop_start` and `loop_end` structural markers, which loop-end pairs with
/// each header, and what the original loop-break polarity was.
pub(super) fn detect_loop_structure(
    ir: &FunctionIR,
    cfg: &CFG,
) -> (
    HashMap<BlockId, LoopRole>,
    HashMap<BlockId, BlockId>,
    HashMap<BlockId, LoopBreakKind>,
) {
    let mut roles = HashMap::new();
    let mut loop_pairs = HashMap::new();
    let mut loop_break_kinds = HashMap::new();
    let block_containing = |op_idx: usize| -> Option<BlockId> {
        cfg.blocks
            .iter()
            .position(|bb| bb.start_op <= op_idx && op_idx < bb.end_op)
            .map(|bid| BlockId(bid as u32))
    };
    for (bid, bb) in cfg.blocks.iter().enumerate() {
        if bb.start_op >= ir.ops.len() {
            continue;
        }
        let first_kind = ir.ops[bb.start_op].kind.as_str();
        match first_kind {
            "loop_start" => {
                roles.insert(BlockId(bid as u32), LoopRole::LoopHeader);
            }
            "loop_end" => {
                roles.insert(BlockId(bid as u32), LoopRole::LoopEnd);
            }
            _ => {}
        }
    }
    let mut loop_stack: Vec<(usize, BlockId)> = Vec::new();
    for (op_idx, op) in ir.ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                if let Some(header_bid) = block_containing(op_idx) {
                    loop_stack.push((op_idx, header_bid));
                }
            }
            "loop_end" => {
                let Some((header_op_idx, header_bid)) = loop_stack.pop() else {
                    continue;
                };
                let Some(end_bid) = block_containing(op_idx) else {
                    continue;
                };
                loop_pairs.insert(header_bid, end_bid);

                let mut nested_depth = 0usize;
                for inner_idx in (header_op_idx + 1)..op_idx {
                    match ir.ops[inner_idx].kind.as_str() {
                        "loop_start" => nested_depth += 1,
                        "loop_end" => nested_depth = nested_depth.saturating_sub(1),
                        "loop_break_if_true" if nested_depth == 0 => {
                            loop_break_kinds.insert(header_bid, LoopBreakKind::BreakIfTrue);
                            break;
                        }
                        "loop_break_if_false" if nested_depth == 0 => {
                            loop_break_kinds.insert(header_bid, LoopBreakKind::BreakIfFalse);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    (roles, loop_pairs, loop_break_kinds)
}

pub(super) fn detect_loop_cond_blocks(ir: &FunctionIR, cfg: &CFG) -> HashMap<BlockId, BlockId> {
    let mut loop_cond_blocks = HashMap::new();
    let block_containing = |op_idx: usize| -> Option<BlockId> {
        cfg.blocks
            .iter()
            .position(|bb| bb.start_op <= op_idx && op_idx < bb.end_op)
            .map(|bid| BlockId(bid as u32))
    };
    let mut loop_stack: Vec<(usize, BlockId)> = Vec::new();
    for (op_idx, op) in ir.ops.iter().enumerate() {
        match op.kind.as_str() {
            "loop_start" => {
                if let Some(header_bid) = block_containing(op_idx) {
                    loop_stack.push((op_idx, header_bid));
                }
            }
            "loop_end" => {
                loop_stack.pop();
            }
            "loop_break_if_true" | "loop_break_if_false" => {
                let Some((_, header_bid)) = loop_stack.last().copied() else {
                    continue;
                };
                let Some(cond_bid) = block_containing(op_idx) else {
                    continue;
                };
                loop_cond_blocks.entry(header_bid).or_insert(cond_bid);
            }
            _ => {}
        }
    }
    loop_cond_blocks
}
