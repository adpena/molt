use std::collections::{HashMap, HashSet};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::passes::alias_analysis::{AliasAnalysis, AliasAnalysisResult};
use crate::tir::passes::escape_analysis::{EscapeState, finalizer_alloc_roots};
use crate::tir::passes::typed_slot_access::boxed_allocation_layout;
use crate::tir::passes::value_range::{ValueRange, ValueRangeResult};
use crate::tir::values::ValueId;

use super::super::PassStats;
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
    let non_heap_values = crate::representation_facts::non_heap_boxed_values_for(func, &ranges);
    let exact_types = crate::tir::type_refine::extract_exact_scalar_map(func);
    let finalizer_roots: HashSet<ValueId> = finalizer_alloc_roots(func)
        .into_iter()
        .map(|value| alias.root(value))
        .collect();
    let report = std::env::var("MOLT_SROA_REPORT").as_deref() == Ok("1");
    let mut diag: Vec<String> = Vec::new();

    let mut candidate_roots = HashMap::new();
    let mut fixed_layout_allocations = 0usize;
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(allocation) = boxed_allocation_layout(op) {
                fixed_layout_allocations += 1;
                let root = alias.root(allocation.result);
                let effects = super::super::effects::op_effects_with_types(op, &exact_types);
                if effects.nothrow
                    && !effects.may_access_arbitrary_heap
                    && !finalizer_roots.contains(&root)
                    && matches!(
                        alias.escape_state(root),
                        EscapeState::NoEscape | EscapeState::ArgEscape
                    )
                {
                    candidate_roots.insert(root, allocation);
                } else if report {
                    diag.push(format!(
                        "  root v{} REJECTED: nothrow={} arbitrary_heap={} finalizer={} escape_state={:?}",
                        root.0,
                        effects.nothrow,
                        effects.may_access_arbitrary_heap,
                        finalizer_roots.contains(&root),
                        alias.escape_state(root)
                    ));
                }
            }
        }
    }
    if candidate_roots.is_empty() {
        emit_report(report, func, fixed_layout_allocations, 0, 0, 0, &diag);
        return stats;
    }

    let mut blocked: HashSet<ValueId> = HashSet::new();
    let mut removable_ops: HashMap<ValueId, Vec<(BlockId, usize)>> = HashMap::new();

    for (&bid, block) in &func.blocks {
        for (op_idx, op) in block.ops.iter().enumerate() {
            let mut touched: HashSet<ValueId> = HashSet::new();
            for &v in op.operands.iter().chain(op.results.iter()) {
                let r = alias.root(v);
                if candidate_roots.contains_key(&r) {
                    touched.insert(r);
                }
            }
            if touched.is_empty() {
                continue;
            }

            if let Some(allocation) = boxed_allocation_layout(op) {
                let alloc_root = alias.root(allocation.result);
                if touched.len() == 1
                    && touched.contains(&alloc_root)
                    && op
                        .operands
                        .iter()
                        .all(|&v| !candidate_roots.contains_key(&alias.root(v)))
                {
                    removable_ops
                        .entry(alloc_root)
                        .or_default()
                        .push((bid, op_idx));
                    continue;
                }
            }

            if alias.is_transparent_alias_op(op) && touched.len() == 1 {
                let root = *touched.iter().next().unwrap();
                removable_ops.entry(root).or_default().push((bid, op_idx));
                continue;
            }

            // Preserve candidate ownership until the entire allocation is
            // replaced. A heap realization needs these releases when any use
            // blocks SROA; native scoped storage also owns its class edge.
            if matches!(op.opcode, OpCode::IncRef | OpCode::DecRef)
                && op.operands.len() == 1
                && op.results.is_empty()
                && touched.len() == 1
            {
                let root = *touched.iter().next().unwrap();
                removable_ops.entry(root).or_default().push((bid, op_idx));
                continue;
            }

            if let Some((store_obj, offset)) = op.plain_typed_slot_store() {
                let store_root = alias.root(store_obj);
                let value = op.operands[1];
                let value_root = alias.root(value);
                let value_is_neutral = non_heap_values.contains(&value);
                // This is a whole-fresh-root proof, not opcode purity: every
                // allowed write remains nonheap after boxing and every other use of
                // the root blocks removal. Thus replacing stores never release
                // a heap value installed by an unknown or pointer-bearing write.
                if touched.len() == 1
                    && touched.contains(&store_root)
                    && candidate_roots[&store_root].admits_field_offset(offset)
                    && !candidate_roots.contains_key(&value_root)
                    && value_is_neutral
                {
                    removable_ops
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
                        candidate_roots.contains_key(&value_root),
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

        block.terminator.for_each_value(|value| {
            let root = alias.root(value);
            if candidate_roots.contains_key(&root) {
                if report {
                    diag.push(format!("  root v{} BLOCKED by terminator (escape)", root.0));
                }
                blocked.insert(root);
            }
        });
    }

    let promotable: Vec<ValueId> = removable_ops
        .keys()
        .copied()
        .filter(|r| !blocked.contains(r))
        .collect();
    if promotable.is_empty() {
        emit_report(
            report,
            func,
            fixed_layout_allocations,
            candidate_roots.len(),
            0,
            0,
            &diag,
        );
        return stats;
    }

    let mut removals_by_block: HashMap<BlockId, HashSet<usize>> = HashMap::new();
    for root in &promotable {
        for &(bid, op_idx) in &removable_ops[root] {
            removals_by_block.entry(bid).or_default().insert(op_idx);
        }
    }
    for (bid, indices) in removals_by_block {
        let block = func
            .blocks
            .get_mut(&bid)
            .expect("SROA planned store block must still exist");
        let mut op_idx = 0;
        block.ops.retain(|_op| {
            let remove = indices.contains(&op_idx);
            op_idx += 1;
            if remove {
                stats.ops_removed += 1;
            }
            !remove
        });
    }

    emit_report(
        report,
        func,
        fixed_layout_allocations,
        candidate_roots.len(),
        promotable.len(),
        stats.ops_removed,
        &diag,
    );

    stats
}
