use crate::tir::analysis::{AnalysisManager, ImmediateDoms, PredMap};
use crate::tir::blocks::BlockId;
use crate::tir::dominators::dominates;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::RefcountBalanceRole;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::balance::{is_refcount_balance_op, refcount_balance_role};

pub(super) fn eliminate_cross_block_pairs(
    func: &mut TirFunction,
    am: &mut AnalysisManager,
    alias: &AliasAnalysisResult,
    stats: &mut PassStats,
) {
    if func.blocks.len() <= 1 {
        return;
    }

    let pred_map = am.get::<PredMap>(func).clone();
    let idoms = am.get::<ImmediateDoms>(func).clone();
    let mut removals = Vec::new();

    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    for &succ_bid in &block_ids {
        let Some(preds) = pred_map.get(&succ_bid) else {
            continue;
        };
        if preds.len() != 1 {
            continue;
        }
        let pred_bid = preds[0];

        if !dominates(pred_bid, succ_bid, &idoms) {
            continue;
        }

        let trailing = {
            let pred_block = &func.blocks[&pred_bid];
            let mut result = None;
            for (idx, op) in pred_block.ops.iter().enumerate().rev() {
                if alias.is_rc_barrier(op) {
                    break;
                }
                let role = refcount_balance_role(op.opcode);
                if role.is_refcount_balance() {
                    if let Some(&val) = op.operands.first() {
                        result = Some(TrailingInfo { role, val, idx });
                    }
                    break;
                }
            }
            result
        };

        let Some(trail) = trailing else {
            continue;
        };

        let pred_block = &func.blocks[&pred_bid];
        if pred_block.ops[(trail.idx + 1)..]
            .iter()
            .any(|op| alias.is_rc_barrier(op))
        {
            continue;
        }

        let Some(target_opcode) = trail.role.complementary_opcode() else {
            continue;
        };

        let leading = {
            let succ_block = &func.blocks[&succ_bid];
            let mut result = None;
            for (idx, op) in succ_block.ops.iter().enumerate() {
                if alias.is_rc_barrier(op) {
                    break;
                }
                if op.opcode == target_opcode && op.operands.first().copied() == Some(trail.val) {
                    result = Some(idx);
                    break;
                }
                if is_refcount_balance_op(op.opcode)
                    && op.operands.first().copied() == Some(trail.val)
                {
                    break;
                }
            }
            result
        };

        if let Some(lead_idx) = leading {
            removals.push((pred_bid, trail.idx, succ_bid, lead_idx));
        }
    }

    for (pred_bid, pred_idx, succ_bid, succ_idx) in removals {
        if let Some(pred_block) = func.blocks.get_mut(&pred_bid)
            && pred_idx < pred_block.ops.len()
        {
            let op = &pred_block.ops[pred_idx];
            if is_refcount_balance_op(op.opcode) && op.operands.first().copied().is_some() {
                pred_block.ops.remove(pred_idx);
                stats.ops_removed += 1;
            }
        }
        if let Some(succ_block) = func.blocks.get_mut(&succ_bid)
            && succ_idx < succ_block.ops.len()
        {
            let op = &succ_block.ops[succ_idx];
            if is_refcount_balance_op(op.opcode) && op.operands.first().copied().is_some() {
                succ_block.ops.remove(succ_idx);
                stats.ops_removed += 1;
            }
        }
    }
}

struct TrailingInfo {
    role: RefcountBalanceRole,
    val: ValueId,
    idx: usize,
}
