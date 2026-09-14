use std::collections::{BTreeMap, BTreeSet};

use crate::tir::analysis::{AnalysisManager, PredMap};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators::{exception_label_to_block, exception_successors};
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::RefcountBalanceRole;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;

use super::super::PassStats;
use super::balance::refcount_balance_role;

/// A retain and release execute equally often only on this one-to-one edge.
/// Conditional successors and implicit exception transfers cannot establish it.
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
    let exception_targets = exception_label_to_block(func);
    let mut removals: BTreeMap<BlockId, BTreeSet<usize>> = BTreeMap::new();
    for (&pred_id, pred) in &func.blocks {
        let Terminator::Branch {
            target: succ_id, ..
        } = &pred.terminator
        else {
            continue;
        };
        if *succ_id == pred_id
            || *succ_id == func.entry_block
            || !pred_map
                .get(succ_id)
                .is_some_and(|preds| preds.as_slice() == [pred_id])
        {
            continue;
        }
        // PredMap is a set of predecessor blocks, not incoming execution
        // points. An exceptional arrival from this same predecessor can skip
        // the trailing retain. The function entry also has an initial arrival
        // not represented by PredMap and is excluded above.
        if exception_successors(pred, &exception_targets).contains(succ_id) {
            continue;
        }
        let Some(succ) = func.blocks.get(succ_id) else {
            continue;
        };
        let mut trailing = None;
        for (index, op) in pred.ops.iter().enumerate().rev() {
            let role = refcount_balance_role(op.opcode);
            if role.is_refcount_balance() {
                if role == RefcountBalanceRole::Increment
                    && op.has_valid_shape()
                    && op.operands.len() == 1
                {
                    trailing = Some((index, alias.root(op.operands[0])));
                }
                break;
            }
            if alias.is_rc_barrier(op) {
                break;
            }
        }
        let Some((retain_index, root)) = trailing else {
            continue;
        };
        for (release_index, op) in succ.ops.iter().enumerate() {
            let role = refcount_balance_role(op.opcode);
            if role == RefcountBalanceRole::Decrement
                && op.has_valid_shape()
                && op.operands.len() == 1
                && alias.root(op.operands[0]) == root
            {
                removals.entry(pred_id).or_default().insert(retain_index);
                removals.entry(*succ_id).or_default().insert(release_index);
                break;
            }
            if role.is_refcount_balance() || alias.is_rc_barrier(op) {
                break;
            }
        }
    }
    // Indices were collected against immutable blocks. Remove descending and
    // deduplicate so a chain cannot shift or consume another pair's endpoint.
    for (block_id, indices) in removals {
        let block = func.blocks.get_mut(&block_id).unwrap();
        for index in indices.into_iter().rev() {
            block.ops.remove(index);
            stats.ops_removed += 1;
        }
    }
}
