use std::collections::{HashMap, HashSet};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::{AliasAnalysis, AliasAnalysisResult};
use crate::tir::passes::escape_analysis::EscapeState;
use crate::tir::passes::value_range::{ValueRange, ValueRangeResult};
use crate::tir::values::ValueId;

use super::super::PassStats;
use super::classify::{
    collect_const_immediates, removable_store_obj_offset, stack_alloc_result,
    store_value_is_refcount_neutral, terminator_references,
};
use super::report::emit_report;

pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    run_with(func, am)
}

fn run_with(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "sroa",
        ..Default::default()
    };
    if func.blocks.values().all(|b| b.ops.is_empty()) {
        return stats;
    }

    let alias: AliasAnalysisResult = am.get::<AliasAnalysis>(func).clone();
    let ranges: ValueRangeResult = am.get::<ValueRange>(func).clone();
    let const_immediates = collect_const_immediates(func, &ranges);
    let report = std::env::var("MOLT_SROA_REPORT").as_deref() == Ok("1");
    let mut diag: Vec<String> = Vec::new();

    let mut candidate_roots: HashSet<ValueId> = HashSet::new();
    let mut raw_stack_allocs = 0usize;
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(result) = stack_alloc_result(op) {
                raw_stack_allocs += 1;
                let root = alias.root(result);
                if matches!(
                    alias.escape_state(root),
                    EscapeState::NoEscape | EscapeState::ArgEscape
                ) {
                    candidate_roots.insert(root);
                } else if report {
                    diag.push(format!(
                        "  root v{} REJECTED: escape_state={:?}",
                        root.0,
                        alias.escape_state(root)
                    ));
                }
            }
        }
    }
    if candidate_roots.is_empty() {
        emit_report(report, func, raw_stack_allocs, 0, 0, 0, &diag);
        return stats;
    }

    let mut blocked: HashSet<ValueId> = HashSet::new();
    let mut stores_to_remove: HashMap<ValueId, Vec<(BlockId, usize)>> = HashMap::new();

    for (&bid, block) in &func.blocks {
        for (op_idx, op) in block.ops.iter().enumerate() {
            let mut touched: HashSet<ValueId> = HashSet::new();
            for &v in op.operands.iter().chain(op.results.iter()) {
                let r = alias.root(v);
                if candidate_roots.contains(&r) {
                    touched.insert(r);
                }
            }
            if touched.is_empty() {
                continue;
            }

            if let Some(alloc_result) = stack_alloc_result(op) {
                let alloc_root = alias.root(alloc_result);
                if touched.len() == 1
                    && touched.contains(&alloc_root)
                    && op
                        .operands
                        .iter()
                        .all(|&v| !candidate_roots.contains(&alias.root(v)))
                {
                    continue;
                }
            }

            if alias.is_transparent_alias_op(op) && touched.len() == 1 {
                continue;
            }

            if let Some((store_obj, _offset)) = removable_store_obj_offset(op) {
                let store_root = alias.root(store_obj);
                let value = op.operands[1];
                let value_root = alias.root(value);
                let value_is_neutral =
                    store_value_is_refcount_neutral(value, func, &const_immediates, &ranges);
                if touched.len() == 1
                    && touched.contains(&store_root)
                    && !candidate_roots.contains(&value_root)
                    && value_is_neutral
                {
                    stores_to_remove
                        .entry(store_root)
                        .or_default()
                        .push((bid, op_idx));
                    continue;
                }
                if report {
                    diag.push(format!(
                        "  root v{} STORE not-removable: touched={} value=v{} \
                         value_is_candidate={} value_neutral={} fits47={}",
                        store_root.0,
                        touched.len(),
                        value.0,
                        candidate_roots.contains(&value_root),
                        value_is_neutral,
                        ranges.fits_inline_int47(value),
                    ));
                }
            }

            if report {
                let mut roots: Vec<u32> = touched.iter().map(|r| r.0).collect();
                roots.sort_unstable();
                diag.push(format!(
                    "  roots {:?} BLOCKED by {:?} (kind={:?})",
                    roots,
                    op.opcode,
                    op.attrs.get("_original_kind"),
                ));
            }
            for r in touched {
                blocked.insert(r);
            }
        }

        for &root in &candidate_roots {
            if terminator_references(&block.terminator, root, &alias) {
                if report {
                    diag.push(format!("  root v{} BLOCKED by terminator (escape)", root.0));
                }
                blocked.insert(root);
            }
        }
    }

    let promotable: Vec<ValueId> = stores_to_remove
        .keys()
        .copied()
        .filter(|r| !blocked.contains(r))
        .collect();
    if promotable.is_empty() {
        emit_report(
            report,
            func,
            raw_stack_allocs,
            candidate_roots.len(),
            0,
            0,
            &diag,
        );
        return stats;
    }

    let mut removals_by_block: HashMap<BlockId, Vec<usize>> = HashMap::new();
    for root in &promotable {
        for &(bid, op_idx) in &stores_to_remove[root] {
            removals_by_block.entry(bid).or_default().push(op_idx);
        }
    }
    for (bid, mut indices) in removals_by_block {
        indices.sort_unstable_by(|a, b| b.cmp(a));
        indices.dedup();
        let Some(block) = func.blocks.get_mut(&bid) else {
            continue;
        };
        for op_idx in indices {
            if op_idx >= block.ops.len() {
                continue;
            }
            if removable_store_obj_offset(&block.ops[op_idx]).is_none() {
                continue;
            }
            block.ops.remove(op_idx);
            stats.ops_removed += 1;
        }
    }

    emit_report(
        report,
        func,
        raw_stack_allocs,
        candidate_roots.len(),
        promotable.len(),
        stats.ops_removed,
        &diag,
    );

    stats
}
