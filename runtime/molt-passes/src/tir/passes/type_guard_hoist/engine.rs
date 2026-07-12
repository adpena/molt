use std::collections::HashMap;

use crate::tir::analysis::{AnalysisManager, ImmediateDoms, LoopForest, PredMap};
use crate::tir::blocks::BlockId;
use crate::tir::dominators::dominates;
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};

use super::super::PassStats;
use super::defs::build_def_map;
use super::loops::{derive_loop_headers, map_blocks_to_innermost_loop};

/// Hoist TypeGuard ops out of loops when the guarded value is loop-invariant.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "type_guard_hoist",
        ..Default::default()
    };

    if func.blocks.is_empty() || func.has_exception_handling {
        return stats;
    }

    let def_map = build_def_map(func);
    let pred_map = am.get::<PredMap>(func).clone();
    let idoms = am.get::<ImmediateDoms>(func).clone();
    let loop_forest = am.get::<LoopForest>(func).clone();

    let loop_headers = derive_loop_headers(&loop_forest, &pred_map);
    if loop_headers.is_empty() {
        return stats;
    }

    let block_to_header = map_blocks_to_innermost_loop(&loop_headers);
    let mut hoist_list = Vec::new();
    let block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();

    for bid in &block_ids {
        let Some(&header) = block_to_header.get(bid) else {
            continue;
        };
        let Some(loop_info) = loop_headers.get(&header) else {
            continue;
        };
        let Some(preheader) = loop_info.preheader else {
            continue;
        };
        let Some(block) = func.blocks.get(bid) else {
            continue;
        };

        for (idx, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::TypeGuard {
                continue;
            }
            let Some(guarded) = op.operands.first().copied() else {
                continue;
            };
            let Some(&def_block) = def_map.get(&guarded) else {
                continue;
            };
            if loop_info.body.contains(&def_block) {
                continue;
            }
            if !dominates(def_block, header, &idoms) {
                continue;
            }
            hoist_list.push(HoistWork {
                preheader,
                op: op.clone(),
                source_block: *bid,
                source_idx: idx,
            });
        }
    }

    if hoist_list.is_empty() {
        return stats;
    }

    apply_hoists(func, hoist_list, &mut stats);
    stats
}

struct HoistWork {
    preheader: BlockId,
    op: TirOp,
    source_block: BlockId,
    source_idx: usize,
}

fn apply_hoists(func: &mut TirFunction, hoist_list: Vec<HoistWork>, stats: &mut PassStats) {
    let mut removals: HashMap<BlockId, Vec<usize>> = HashMap::new();
    let mut inserts: HashMap<BlockId, Vec<TirOp>> = HashMap::new();

    for work in hoist_list {
        removals
            .entry(work.source_block)
            .or_default()
            .push(work.source_idx);
        inserts.entry(work.preheader).or_default().push(work.op);
    }

    for (bid, mut indices) in removals {
        indices.sort_unstable_by(|a, b| b.cmp(a));
        indices.dedup();
        if let Some(block) = func.blocks.get_mut(&bid) {
            for idx in &indices {
                block.ops.remove(*idx);
                stats.ops_removed += 1;
            }
        }
    }

    for (bid, ops) in inserts {
        if let Some(block) = func.blocks.get_mut(&bid) {
            for op in ops {
                block.ops.push(op);
                stats.ops_added += 1;
            }
        }
    }
}
