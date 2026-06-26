use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::tir::analysis::AnalysisManager;
use crate::tir::blocks::{BlockId, Terminator, TirBlock};
use crate::tir::function::TirFunction;
use crate::tir::ops::{OpCode, TirOp};
use crate::tir::passes::ownership_lattice_min::{
    copy_transparent_alias, exception_creation_ref_values,
};
use crate::tir::values::{TirValue, ValueId};

use super::arcs::{ArcDescriptor, retarget_arc, terminator_arcs};
use super::audit::emit_drop_inner_stage_audit;
use super::remap::{
    remap_op_operands, remap_terminator_values, remap_uses_dominated_by_split_continuation,
};
use super::util::make_op;

#[derive(Debug, Default)]
pub(super) struct ExceptionRegionDropInsertion {
    pub(super) dec_refs_added: usize,
    pub(super) cfg_changed: bool,
}

#[derive(Debug, Clone, Copy)]
struct ValueDefinition {
    block: BlockId,
    op_index: Option<usize>,
}

fn value_definitions(func: &TirFunction) -> HashMap<ValueId, ValueDefinition> {
    let mut defs: HashMap<ValueId, ValueDefinition> = HashMap::new();
    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            defs.insert(
                arg.id,
                ValueDefinition {
                    block: bid,
                    op_index: None,
                },
            );
        }
        for (op_index, op) in block.ops.iter().enumerate() {
            for &result in &op.results {
                defs.insert(
                    result,
                    ValueDefinition {
                        block: bid,
                        op_index: Some(op_index),
                    },
                );
            }
        }
    }
    defs
}

pub(super) fn explicit_release_values(op: &TirOp) -> Vec<ValueId> {
    if op.opcode == OpCode::DecRef {
        return op.operands.to_vec();
    }
    if op.opcode == OpCode::DeleteVar {
        return op.operands.get(1).copied().into_iter().collect();
    }
    Vec::new()
}

pub(super) fn insert_exception_creation_drops_at_raise(func: &mut TirFunction) -> usize {
    let creation_refs = exception_creation_ref_values(func);
    if creation_refs.is_empty() {
        return 0;
    }

    let mut inserted = 0usize;
    for block in func.blocks.values_mut() {
        let mut new_ops = Vec::with_capacity(block.ops.len());
        let mut changed = false;
        for op in &block.ops {
            new_ops.push(op.clone());
            if op.opcode != OpCode::Raise {
                continue;
            }
            let mut values: Vec<ValueId> = op
                .operands
                .iter()
                .copied()
                .filter(|value| creation_refs.contains(value))
                .collect();
            values.sort_unstable_by_key(|value| value.0);
            values.dedup();
            for value in values {
                new_ops.push(make_op(OpCode::DecRef, vec![value]));
                inserted += 1;
                changed = true;
            }
        }
        if changed {
            block.ops = new_ops;
        }
    }
    inserted
}

fn definition_available_before_position(
    def: ValueDefinition,
    position: crate::tir::exception_regions::ExceptionOpPosition,
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> bool {
    if def.block == position.block {
        return def
            .op_index
            .is_none_or(|op_index| op_index < position.op_index);
    }
    crate::tir::dominators::dominates(def.block, position.block, idoms)
}

fn definition_available_on_edge(
    def: ValueDefinition,
    pred: BlockId,
    idoms: &HashMap<BlockId, Option<BlockId>>,
) -> bool {
    def.block == pred || crate::tir::dominators::dominates(def.block, pred, idoms)
}

pub(super) fn insert_exception_region_match_drops(
    func: &mut TirFunction,
    am: &mut AnalysisManager,
) -> ExceptionRegionDropInsertion {
    let audit_start = std::time::Instant::now();
    emit_drop_inner_stage_audit(
        func,
        "exception-region-before-analysis",
        None,
        None,
        None,
        None,
        audit_start.elapsed().as_millis(),
    );
    let release_to_matches = am
        .get::<crate::tir::exception_regions::ExceptionRegions>(func)
        .release_to_match_facts
        .clone();
    let release_fact_count: usize = release_to_matches.values().map(Vec::len).sum();
    emit_drop_inner_stage_audit(
        func,
        "exception-region-after-analysis",
        None,
        None,
        Some(release_fact_count),
        Some(release_to_matches.len()),
        audit_start.elapsed().as_millis(),
    );
    if release_to_matches.is_empty() {
        return ExceptionRegionDropInsertion::default();
    }

    let pred_map_term = crate::tir::dominators::build_pred_map_with(
        func,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    let idoms = crate::tir::dominators::compute_idoms_with(
        func,
        &pred_map_term,
        crate::tir::dominators::CfgEdgePolicy::Full,
    );
    let defs = value_definitions(func);
    let mut result = ExceptionRegionDropInsertion::default();
    emit_drop_inner_stage_audit(
        func,
        "exception-region-after-dominators",
        None,
        None,
        Some(defs.len()),
        Some(idoms.len()),
        audit_start.elapsed().as_millis(),
    );

    for (position, release_facts) in release_to_matches {
        emit_drop_inner_stage_audit(
            func,
            "exception-region-position-start",
            None,
            None,
            Some(release_facts.len()),
            Some(position.block.0 as usize),
            audit_start.elapsed().as_millis(),
        );
        let (original_args, pop_op, prefix_source_ops, tail_source_ops, tail_source_terminator) = {
            let Some(block) = func.blocks.get(&position.block) else {
                continue;
            };
            if position.op_index >= block.ops.len() {
                continue;
            }
            debug_assert_eq!(
                block.ops[position.op_index].opcode,
                OpCode::Copy,
                "ExceptionRegions release position must point at an exception_pop carrier"
            );
            (
                block.args.clone(),
                block.ops[position.op_index].clone(),
                block.ops[..position.op_index].to_vec(),
                block.ops[position.op_index + 1..].to_vec(),
                block.terminator.clone(),
            )
        };

        let mut incoming_arcs: Vec<(BlockId, ArcDescriptor, Vec<ValueId>)> = pred_map_term
            .get(&position.block)
            .into_iter()
            .flat_map(|preds| preds.iter().copied())
            .flat_map(|pred| {
                func.blocks
                    .get(&pred)
                    .map(|pred_block| {
                        terminator_arcs(&pred_block.terminator)
                            .into_iter()
                            .filter(move |arc| arc.target == position.block)
                            .map(move |arc| (pred, arc.descriptor, arc.args))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .collect();
        incoming_arcs.sort_by_key(|(pred, _, _)| pred.0);
        emit_drop_inner_stage_audit(
            func,
            "exception-region-after-incoming-arcs",
            None,
            None,
            Some(incoming_arcs.len()),
            Some(position.block.0 as usize),
            audit_start.elapsed().as_millis(),
        );

        let all_incoming_preds: BTreeSet<BlockId> =
            incoming_arcs.iter().map(|(pred, _, _)| *pred).collect();
        let mut value_entry_preds: BTreeMap<ValueId, BTreeSet<BlockId>> = BTreeMap::new();
        let mut direct_values = BTreeSet::new();
        for fact in release_facts {
            let Some(&def) = defs.get(&fact.value) else {
                continue;
            };
            if fact.entry_predecessors.is_empty() {
                if definition_available_before_position(def, position, &idoms) {
                    direct_values.insert(fact.value);
                }
                continue;
            }
            value_entry_preds
                .entry(fact.value)
                .or_default()
                .extend(fact.entry_predecessors.iter().copied());
        }

        let mut global_values: Vec<ValueId> = value_entry_preds
            .iter()
            .filter_map(|(&value, preds)| {
                let def = *defs.get(&value)?;
                (!all_incoming_preds.is_empty()
                    && preds.is_superset(&all_incoming_preds)
                    && definition_available_before_position(def, position, &idoms))
                .then_some(value)
            })
            .collect();
        global_values.extend(direct_values.iter().copied());
        global_values.sort_unstable_by_key(|value| value.0);
        global_values.dedup();

        let global_set: BTreeSet<ValueId> = global_values.iter().copied().collect();
        let mut path_values: Vec<ValueId> = value_entry_preds
            .keys()
            .copied()
            .filter(|value| !global_set.contains(value))
            .collect();
        path_values.sort_unstable_by_key(|value| value.0);

        if path_values.is_empty() {
            let Some(block) = func.blocks.get_mut(&position.block) else {
                continue;
            };
            let mut new_ops = Vec::with_capacity(block.ops.len() + global_values.len());
            for (idx, op) in block.ops.iter().enumerate() {
                new_ops.push(op.clone());
                if idx == position.op_index {
                    for value in &global_values {
                        new_ops.push(make_op(OpCode::DecRef, vec![*value]));
                        result.dec_refs_added += 1;
                    }
                }
            }
            block.ops = new_ops;
            continue;
        }

        if incoming_arcs.is_empty() {
            continue;
        }

        let mut split_plans = Vec::new();
        for (pred, arc, args) in &incoming_arcs {
            let mut edge_values = global_values.clone();
            let mut edge_specific = Vec::new();
            for value in &path_values {
                let Some(preds) = value_entry_preds.get(value) else {
                    continue;
                };
                if !preds.contains(pred) {
                    continue;
                }
                let Some(&def) = defs.get(value) else {
                    continue;
                };
                if definition_available_on_edge(def, *pred, &idoms) {
                    edge_specific.push(*value);
                }
            }
            if edge_specific.is_empty() {
                continue;
            }
            edge_values.extend(edge_specific);
            edge_values.sort_unstable_by_key(|value| value.0);
            edge_values.dedup();
            split_plans.push((*pred, *arc, args.clone(), edge_values));
        }
        if split_plans.is_empty() {
            continue;
        }
        emit_drop_inner_stage_audit(
            func,
            "exception-region-before-split",
            None,
            Some(split_plans.len()),
            Some(
                split_plans
                    .iter()
                    .map(|(_, _, _, values)| values.len())
                    .sum(),
            ),
            Some(position.block.0 as usize),
            audit_start.elapsed().as_millis(),
        );

        let mut tail_arg_remap: HashMap<ValueId, ValueId> = HashMap::new();
        let after_args: Vec<TirValue> = original_args
            .iter()
            .map(|arg| {
                let new_id = func.fresh_value();
                tail_arg_remap.insert(arg.id, new_id);
                func.value_types.insert(new_id, arg.ty.clone());
                TirValue {
                    id: new_id,
                    ty: arg.ty.clone(),
                }
            })
            .collect();
        let mut tail_value_remap = tail_arg_remap.clone();
        for op in &prefix_source_ops {
            if let Some(alias) = copy_transparent_alias(op)
                && let Some(mapped_operand) = tail_value_remap.get(&alias.source).copied()
            {
                tail_value_remap.insert(alias.result, mapped_operand);
            }
        }
        let original_arg_values: Vec<ValueId> = original_args.iter().map(|arg| arg.id).collect();
        let tail_ops: Vec<TirOp> = tail_source_ops
            .iter()
            .map(|op| remap_op_operands(op, &tail_value_remap))
            .collect();
        let tail_terminator = remap_terminator_values(&tail_source_terminator, &tail_value_remap);
        let after_block = func.fresh_block();
        func.blocks.insert(
            after_block,
            TirBlock {
                id: after_block,
                args: after_args,
                ops: tail_ops,
                terminator: tail_terminator,
            },
        );

        if let Some(block) = func.blocks.get_mut(&position.block) {
            block.ops.truncate(position.op_index + 1);
            let mut original_ops = Vec::with_capacity(block.ops.len() + global_values.len());
            for (idx, op) in block.ops.iter().enumerate() {
                original_ops.push(op.clone());
                if idx == position.op_index {
                    for value in &global_values {
                        original_ops.push(make_op(OpCode::DecRef, vec![*value]));
                        result.dec_refs_added += 1;
                    }
                }
            }
            block.ops = original_ops;
            block.terminator = Terminator::Branch {
                target: after_block,
                args: original_arg_values.clone(),
            };
        }

        for (pred, arc, args, edge_values) in split_plans {
            let split_block = func.fresh_block();
            let mut ops = Vec::with_capacity(1 + edge_values.len());
            ops.push(pop_op.clone());
            for value in edge_values {
                ops.push(make_op(OpCode::DecRef, vec![value]));
                result.dec_refs_added += 1;
            }
            func.blocks.insert(
                split_block,
                TirBlock {
                    id: split_block,
                    args: vec![],
                    ops,
                    terminator: Terminator::Branch {
                        target: after_block,
                        args,
                    },
                },
            );
            if let Some(pred_block) = func.blocks.get_mut(&pred) {
                retarget_arc(&mut pred_block.terminator, &arc, split_block);
            }
        }
        emit_drop_inner_stage_audit(
            func,
            "exception-region-before-remap",
            None,
            None,
            Some(tail_value_remap.len()),
            Some(after_block.0 as usize),
            audit_start.elapsed().as_millis(),
        );
        remap_uses_dominated_by_split_continuation(func, after_block, &tail_value_remap);
        emit_drop_inner_stage_audit(
            func,
            "exception-region-after-remap",
            None,
            None,
            Some(tail_value_remap.len()),
            Some(after_block.0 as usize),
            audit_start.elapsed().as_millis(),
        );
        result.cfg_changed = true;
    }

    emit_drop_inner_stage_audit(
        func,
        "exception-region-complete",
        None,
        None,
        Some(result.dec_refs_added),
        None,
        audit_start.elapsed().as_millis(),
    );
    result
}
