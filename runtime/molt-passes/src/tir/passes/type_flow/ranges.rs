//! Successful return intervals over the final type-flow context graph.
//! Each context is visited at most once. A back edge reads unknown, so an
//! interval can cross recursion only when a body proves it independently.

use std::cell::OnceCell;

use super::analysis::{Analysis, ContextId, Inputs};
use crate::tir::numeric_facts::IntRange;
use crate::tir::op_kinds_generated::{
    ValueRangeTransferRule, opcode_value_range_transfer_rule_table,
};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::value_identity::forwarded_python_value_source;
use crate::tir::passes::value_range::{ValueRangeResult, transfer_op_range};
use crate::tir::types::TirType;

pub(super) fn solve(
    inputs: &Inputs<'_>,
    analysis: &Analysis,
    reached: &[bool],
) -> Vec<Option<IntRange>> {
    let mut ranges = vec![None; reached.len()];
    let mut visited = vec![false; reached.len()];
    let mut stack = Vec::new();
    // Only a reached caller can publish a return interval. A root body with
    // no callers needs no return analysis of its own.
    for (caller, &live) in analysis.contexts.iter().zip(reached) {
        if !live {
            continue;
        }
        for &target in caller.targets.iter().flatten() {
            if visited[target.0] || analysis.context(target).ret != TirType::I64 {
                continue;
            }
            visited[target.0] = true;
            stack.push((target, 0));
            while let Some((id, next)) = stack.last_mut() {
                let context = analysis.context(*id);
                if let Some(target) = context.targets.get(*next) {
                    *next += 1;
                    if let Some(target) = *target
                        && !visited[target.0]
                        && analysis.context(target).ret == TirType::I64
                    {
                        visited[target.0] = true;
                        stack.push((target, 0));
                    }
                } else {
                    let id = *id;
                    ranges[id.0] = summarize(inputs, analysis, id, &ranges);
                    stack.pop();
                }
            }
        }
    }
    ranges
}

fn summarize(
    inputs: &Inputs<'_>,
    analysis: &Analysis,
    id: ContextId,
    returns: &[Option<IntRange>],
) -> Option<IntRange> {
    let context = analysis.context(id);
    let func = &inputs.module.functions[context.body];
    let plan = inputs.exact_plan(context.body);
    let mut ranges = ValueRangeResult::default();
    if context
        .targets
        .iter()
        .flatten()
        .any(|id| returns[id.0].is_some())
    {
        let sites = &inputs.result_sites[context.body];
        for (success, range) in plan.call_range_values(|call| {
            let site = *sites.get(call.results.first()?)?;
            returns[context.targets[site]?.0]
        }) {
            record(&mut ranges, success, range);
        }
    }
    // Walking executable blocks once suffices for acyclic value definitions.
    // A loop phi with an as-yet unproved latch cannot seed its own bound.
    let exact = OnceCell::new();
    for &bid in plan.block_order() {
        let block = &func.blocks[&bid];
        if let Some(edges) = plan.normal_incoming_edges(bid) {
            for (i, arg) in block.args.iter().enumerate() {
                let mut incoming = edges
                    .iter()
                    .map(|edge| ranges.global_range_of(*edge.get(i)?));
                if let Some(Some(first)) = incoming.next()
                    && let Some(hull) = incoming.try_fold(first, |hull, r| Some(hull.join(r?)))
                {
                    record(&mut ranges, arg.id, hull);
                }
            }
        }
        for op in &block.ops {
            let [result] = op.results[..] else { continue };
            if !op.has_valid_shape() || op.checked_call_result_source().is_some() {
                continue;
            }
            let range = if op.opcode == OpCode::ConstInt {
                match op.attrs.get("value") {
                    Some(AttrValue::Int(value)) => Some(IntRange::point(*value)),
                    _ => None,
                }
            } else if let Some(source) = forwarded_python_value_source(op) {
                ranges.global_range_of(source)
            } else {
                let rule = opcode_value_range_transfer_rule_table(op.opcode);
                if rule == ValueRangeTransferRule::None {
                    continue;
                }
                // An exact Python int can be arbitrarily wide. Only a mask
                // or modulo may bound an input of unknown magnitude. Require
                // an exact integer there (a user-defined operator may return
                // anything), and never divide or shift a saturated endpoint
                // back into the inline window.
                transfer_op_range(op, bid, &ranges).filter(|range| {
                    range.is_bounded()
                        && (!matches!(
                            rule,
                            ValueRangeTransferRule::BitAnd | ValueRangeTransferRule::Mod
                        ) || op.operands.iter().all(|value| {
                            ranges.has_global_range(*value)
                                || matches!(
                                    exact.get_or_init(|| analysis.facts(inputs, id)).get(value),
                                    Some(TirType::I64 | TirType::BigInt | TirType::Bool)
                                )
                        }))
                })
            };
            if let Some(range) = range.filter(|r| r.is_bounded()) {
                record(&mut ranges, result, range);
            }
        }
    }
    let mut values = plan
        .return_values()
        .iter()
        .map(|&v| ranges.global_range_of(v));
    let first = values.next()??;
    values.try_fold(first, |hull, r| Some(hull.join(r?)))
}

fn record(ranges: &mut ValueRangeResult, value: crate::tir::values::ValueId, range: IntRange) {
    ranges.record_global_range(value, range);
    if range.lo == range.hi {
        ranges.record_const_int(value, range.lo);
    }
}
