use std::collections::HashSet;

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
use crate::tir::target_info::TargetInfo;
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::facts::BranchlessFacts;

/// Run the branchless boolean counting pass on `func`.
///
/// Gated by the cost model: the rewrite trades a conditional branch for a
/// branchless add, which only pays off where a mispredicted branch costs more
/// than the extra arithmetic.
pub fn run(func: &mut TirFunction, tti: &TargetInfo) -> PassStats {
    let mut stats = PassStats {
        name: "branchless_count",
        ..Default::default()
    };

    if !tti.is_profitable_branchless_rewrite() {
        return stats;
    }

    let facts = BranchlessFacts::collect(func);
    let rewrites = collect_rewrites(func, &facts);
    apply_rewrites(func, rewrites, &mut stats);
    stats
}

fn collect_rewrites(func: &TirFunction, facts: &BranchlessFacts) -> Vec<Rewrite> {
    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_unstable();
    let predecessors = crate::tir::dominators::build_pred_map(func);
    let Ok(mut protected_roots) = func.block_retirement_metadata_roots(&HashSet::new()) else {
        // Invalid label custody cannot authorize a rewrite. The TIR verifier
        // owns diagnostics for invalid input; preserve it without mutation.
        return Vec::new();
    };
    protected_roots.insert(func.entry_block);
    // Predecessor sets deliberately erase edge multiplicity and kind. An arm
    // can also be an implicit handler of its sole ordinary predecessor, or a
    // structural TryEnd target. Neither ownership is retired by this rewrite.
    let mut rewrites = Vec::new();

    for &bid in &block_ids {
        let block = &func.blocks[&bid];

        let (cond, then_blk, else_blk) = match &block.terminator {
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } if then_args.is_empty() && else_args.is_empty() => (*cond, *then_block, *else_block),
            _ => continue,
        };

        if !facts.is_bool(cond) {
            continue;
        }
        if then_blk == else_blk
            || then_blk == bid
            || else_blk == bid
            || protected_roots.contains(&then_blk)
            || protected_roots.contains(&else_blk)
            || predecessors.get(&then_blk).map(Vec::as_slice) != Some(&[bid])
            || predecessors.get(&else_blk).map(Vec::as_slice) != Some(&[bid])
        {
            continue;
        }

        let Some(then_block) = func.blocks.get(&then_blk) else {
            continue;
        };
        if !then_block.args.is_empty() || then_block.ops.len() != 1 {
            continue;
        }
        let then_op = &then_block.ops[0];
        if !matches!(then_op.opcode, OpCode::Add | OpCode::InplaceAdd) {
            continue;
        }
        if then_op.operands.len() != 2 || then_op.results.len() != 1 {
            continue;
        }

        let counter_val = if facts.const_int(then_op.operands[1]) == Some(1) {
            then_op.operands[0]
        } else if facts.const_int(then_op.operands[0]) == Some(1) {
            then_op.operands[1]
        } else {
            continue;
        };
        if !facts.can_increment_unconditionally(func, bid, counter_val) {
            continue;
        }

        let incremented_val = then_op.results[0];

        let (merge_blk, then_merge_args) = match &then_block.terminator {
            Terminator::Branch { target, args } => (*target, args.clone()),
            _ => continue,
        };
        if then_merge_args.len() != 1 || then_merge_args[0] != incremented_val {
            continue;
        }
        if [bid, then_blk, else_blk].contains(&merge_blk) {
            continue;
        }

        let Some(else_block) = func.blocks.get(&else_blk) else {
            continue;
        };
        if !else_block.args.is_empty() || !else_block.ops.is_empty() {
            continue;
        }
        let (else_target, else_merge_args) = match &else_block.terminator {
            Terminator::Branch { target, args } => (*target, args.clone()),
            _ => continue,
        };
        if else_target != merge_blk {
            continue;
        }
        if else_merge_args.len() != 1 || else_merge_args[0] != counter_val {
            continue;
        }

        let Some(merge_block) = func.blocks.get(&merge_blk) else {
            continue;
        };
        if merge_block.args.len() != 1 {
            continue;
        }

        rewrites.push(Rewrite {
            cond_block: bid,
            then_block_id: then_blk,
            else_block_id: else_blk,
            merge_block_id: merge_blk,
            cond_val: cond,
            counter_val,
        });
    }

    rewrites
}

fn apply_rewrites(func: &mut TirFunction, rewrites: Vec<Rewrite>, stats: &mut PassStats) {
    if rewrites.is_empty() {
        return;
    }
    let mut retired = HashSet::with_capacity(rewrites.len() * 2);
    for rw in rewrites {
        let new_counter = func.fresh_value();

        let add_op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![rw.counter_val, rw.cond_val],
            results: vec![new_counter],
            attrs: AttrDict::new(),
            source_span: None,
        };

        let cond_block = func.blocks.get_mut(&rw.cond_block).unwrap();
        cond_block.ops.push(add_op);
        cond_block.terminator = Terminator::Branch {
            target: rw.merge_block_id,
            args: vec![new_counter],
        };

        retired.insert(rw.then_block_id);
        retired.insert(rw.else_block_id);

        stats.values_changed += 1;
        stats.ops_removed += 1;
        stats.ops_added += 1;
    }
    let retained = func
        .blocks
        .keys()
        .copied()
        .filter(|block| !retired.contains(block))
        .collect();
    func.retain_blocks(&retained)
        .expect("branchless diamond retirement preserves all live block references");
}

struct Rewrite {
    cond_block: BlockId,
    then_block_id: BlockId,
    else_block_id: BlockId,
    merge_block_id: BlockId,
    cond_val: ValueId,
    counter_val: ValueId,
}
