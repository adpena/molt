use crate::tir::analysis::{AnalysisManager, ImmediateDoms, LoopForest};
use crate::tir::blocks::BlockId;
use crate::tir::dominators::dominates;
use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;

use super::super::PassStats;
use super::balance::refcount_balance_role;
use super::facts::build_def_map;

pub(super) fn eliminate_loop_invariant_pairs(
    func: &mut TirFunction,
    am: &mut AnalysisManager,
    alias: &AliasAnalysisResult,
    stats: &mut PassStats,
) {
    let loop_forest = am.get::<LoopForest>(func).clone();
    if loop_forest.headers.is_empty() {
        return;
    }

    let idoms = am.get::<ImmediateDoms>(func).clone();
    let def_map = build_def_map(func);
    let mut removals = Vec::new();

    for &header_bid in &loop_forest.headers {
        let Some(block) = func.blocks.get(&header_bid) else {
            continue;
        };
        let Some(body) = loop_forest.bodies.get(&header_bid) else {
            continue;
        };
        let n = block.ops.len();
        if n < 2 {
            continue;
        }

        for i in 0..n {
            let op_i = &block.ops[i];
            let role_i = refcount_balance_role(op_i.opcode);
            let Some(target_opcode) = role_i.complementary_opcode() else {
                continue;
            };
            let Some(val) = op_i.operands.first().copied() else {
                continue;
            };

            let Some(&def_block) = def_map.get(&val) else {
                continue;
            };
            if body.contains(&def_block) || !dominates(def_block, header_bid, &idoms) {
                continue;
            }

            let mut partner = None;
            for j in (i + 1)..n {
                let op_j = &block.ops[j];
                if alias.is_rc_barrier(op_j) {
                    break;
                }
                if op_j.opcode == target_opcode && op_j.operands.first().copied() == Some(val) {
                    partner = Some(j);
                    break;
                }
                if refcount_balance_role(op_j.opcode) == role_i
                    && op_j.operands.first().copied() == Some(val)
                {
                    break;
                }
            }

            if let Some(j) = partner {
                removals.push((header_bid, i, j));
                break;
            }
        }
    }

    for (bid, idx_a, idx_b) in removals {
        apply_pair_removal(func, stats, bid, idx_a, idx_b);
    }
}

fn apply_pair_removal(
    func: &mut TirFunction,
    stats: &mut PassStats,
    bid: BlockId,
    idx_a: usize,
    idx_b: usize,
) {
    if let Some(block) = func.blocks.get_mut(&bid) {
        let (lo, hi) = if idx_a < idx_b {
            (idx_a, idx_b)
        } else {
            (idx_b, idx_a)
        };
        if hi < block.ops.len() && lo < block.ops.len() {
            block.ops.remove(hi);
            block.ops.remove(lo);
            stats.ops_removed += 2;
        }
    }
}
