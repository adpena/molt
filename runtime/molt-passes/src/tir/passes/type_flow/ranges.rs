//! Successful return intervals over the final type-flow context graph.
//! Each context is visited at most once. A back edge reads unknown, so an
//! interval can cross recursion only when a body proves it independently.

use std::cell::OnceCell;

use molt_ir::deterministic_hash::HashMap;

use super::analysis::{Analysis, ContextId, Inputs};
use crate::tir::numeric_facts::IntRange;
use crate::tir::op_kinds_generated::{
    ValueRangeTransferRule, opcode_value_range_transfer_rule_table,
};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::passes::value_identity::forwarded_python_value_source;
use crate::tir::passes::value_range::{ValueRangeResult, transfer_op_range};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

#[derive(Default)]
pub(super) struct ContextRanges {
    pub(super) returned: Option<IntRange>,
    /// Intervals belonging to the actual at one invocation of a forwarding callee.
    pub(super) forwarded: HashMap<usize, IntRange>,
}

pub(super) fn solve(
    inputs: &Inputs<'_>,
    analysis: &Analysis,
    reached: &[bool],
) -> Vec<ContextRanges> {
    let mut ranges: Vec<_> = reached.iter().map(|_| ContextRanges::default()).collect();
    let mut needed = vec![false; reached.len()];
    let mut actuals: Vec<HashMap<usize, ValueId>> =
        reached.iter().map(|_| HashMap::default()).collect();
    for (index, caller) in analysis.contexts.iter().enumerate() {
        if !reached[index] {
            continue;
        }
        for (site, target) in caller.targets.iter().enumerate() {
            let Some(target) = target else { continue };
            let callee = analysis.context(*target);
            needed[target.0] |= callee.ret == TirType::I64;
            if caller.results[site] == TirType::I64
                && let Some(actual) =
                    inputs.forwarded_argument(callee.body, &inputs.sites[caller.body][site])
            {
                actuals[index].insert(site, actual);
                needed[index] = true;
            }
        }
    }
    let mut visited = vec![false; reached.len()];
    let mut stack = Vec::new();
    for (index, need) in needed.iter().enumerate() {
        if !need || visited[index] {
            continue;
        }
        visited[index] = true;
        stack.push((ContextId(index), 0));
        while let Some((id, next)) = stack.last_mut() {
            let context = analysis.context(*id);
            if let Some(target) = context.targets.get(*next) {
                *next += 1;
                if let Some(target) = *target
                    && !visited[target.0]
                    && needed[target.0]
                {
                    visited[target.0] = true;
                    stack.push((target, 0));
                }
            } else {
                let id = *id;
                ranges[id.0] = summarize(inputs, analysis, id, &ranges, &actuals[id.0]);
                stack.pop();
            }
        }
    }
    ranges
}

fn summarize(
    inputs: &Inputs<'_>,
    analysis: &Analysis,
    id: ContextId,
    returns: &[ContextRanges],
    actuals: &HashMap<usize, ValueId>,
) -> ContextRanges {
    let context = analysis.context(id);
    let func = &inputs.module.functions[context.body];
    let plan = inputs.exact_plan(context.body);
    let mut ranges = ValueRangeResult::default();
    let mut summary = ContextRanges::default();
    let sites = &inputs.result_sites[context.body];
    if context
        .targets
        .iter()
        .flatten()
        .any(|id| returns[id.0].returned.is_some())
    {
        for (success, range) in plan.checked_call_values(|call| {
            let site = *sites.get(call.results.first()?)?;
            returns[context.targets[site]?.0].returned
        }) {
            record(&mut ranges, success, range);
        }
    }
    let mut forwarded = HashMap::default();
    if !actuals.is_empty() {
        forwarded.extend(plan.checked_call_values(|call| {
            let site = *sites.get(call.results.first()?)?;
            Some((site, *actuals.get(&site)?))
        }));
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
            if !op.has_valid_shape() {
                continue;
            }
            if let Some(&(site, actual)) = forwarded.get(&result)
                && let Some(range) = ranges.global_range_of(actual)
            {
                record(&mut ranges, result, range);
                summary.forwarded.insert(site, range);
                continue;
            }
            if op.checked_call_result_source().is_some() {
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
    if context.ret == TirType::I64
        && let Some(Some(first)) = values.next()
    {
        summary.returned = values.try_fold(first, |hull, r| Some(hull.join(r?)));
    }
    summary
}

fn record(ranges: &mut ValueRangeResult, value: crate::tir::values::ValueId, range: IntRange) {
    ranges.record_global_range(value, range);
    if range.lo == range.hi {
        ranges.record_const_int(value, range.lo);
    }
}
