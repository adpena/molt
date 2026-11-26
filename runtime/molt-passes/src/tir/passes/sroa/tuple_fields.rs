//! Forward constant-index reads of a materialized exact tuple after drop insertion.
//! Construction can fail and a published frame home can expose the tuple. Keep the
//! allocation, its exception observation, publication, and every planned release.
//! A boxed-neutral member can be read again at the index site without extending
//! a reference or changing the independent ownership of the original subscript.

use molt_ir::OpKind;
use molt_ir::deterministic_hash::HashMap;

use crate::tir::analysis::AnalysisManager;
use crate::tir::dominators::{
    ProgramPointDominance, checked_exception_observer, exception_label_to_block,
};
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::passes::PassStats;
use crate::tir::passes::typed_slot_access::fixed_layout_inputs;
use crate::tir::passes::value_identity::{
    ScalarFacts, build_forwarding_map, forwarded_python_value_source,
};

fn plain_borrowing_op(op: &TirOp) -> bool {
    op.runtime.is_none()
        && op.callee.is_none()
        && !op.is_async_work_poll()
        && !op.attrs.contains_key("runtime_symbol")
        && !op.attrs.contains_key("native_callable_symbol")
        && (0..op.operands.len())
            .all(|position| op.operand_custody(position) == molt_ir::ParameterCustody::Borrowed)
}

pub(crate) fn forward_materialized_tuple_fields(
    func: &mut TirFunction,
    am: &mut AnalysisManager,
) -> PassStats {
    let mut stats = PassStats {
        name: "materialized_tuple_fields",
        ..Default::default()
    };
    if !func.blocks.values().any(|block| {
        block
            .ops
            .iter()
            .any(|op| op.opcode == OpCode::Copy && op.origin == Some(OpKind::TupleNew))
    }) {
        return stats;
    }
    let mut tuples = HashMap::default();
    let mut constants = HashMap::default();
    let mut reads = Vec::new();
    for block in func.blocks.values() {
        for (index, op) in block.ops.iter().enumerate() {
            match (op.opcode, op.origin, op.results.as_slice()) {
                (OpCode::Copy, Some(OpKind::TupleNew), [result]) if plain_borrowing_op(op) => {
                    if let Some(check) = block.ops.get(index + 1)
                        && check.opcode == OpCode::CheckException
                        && !check.is_async_work_poll()
                        && !check.has_checked_invocation_binding()
                    {
                        tuples.insert(*result, (block.id, index + 1, &op.operands));
                    }
                }
                (OpCode::ConstInt, None | Some(OpKind::Const), [result]) => {
                    if op.operands.is_empty()
                        && let Some(AttrValue::Int(value)) = op.attrs.get("value")
                    {
                        constants.insert(*result, *value);
                    }
                }
                (OpCode::Index, None | Some(OpKind::Index), [_]) if plain_borrowing_op(op) => {
                    if let [receiver, key] = op.operands.as_slice() {
                        reads.push((block.id, index, *receiver, *key));
                    }
                }
                _ => {}
            }
        }
    }
    if tuples.is_empty() || reads.is_empty() || constants.is_empty() {
        return stats;
    }
    let forwarded = build_forwarding_map(func, forwarded_python_value_source);
    let origin = |value| forwarded.get(&value).copied().unwrap_or(value);
    let labels = exception_label_to_block(func);
    let mut candidates = Vec::new();
    for (bid, at, receiver, key) in reads {
        let Some(&(owner, check, elements)) = tuples.get(&origin(receiver)) else {
            continue;
        };
        let Some(&index) = constants.get(&origin(key)) else {
            continue;
        };
        let Ok(len) = i64::try_from(elements.len()) else {
            continue;
        };
        let normalized = if index < 0 { len + index } else { index };
        let Some(&element) = usize::try_from(normalized)
            .ok()
            .and_then(|pos| elements.get(pos))
        else {
            continue;
        };
        if checked_exception_observer(&func.blocks[&owner].ops[check], |label| {
            labels.get(&label).copied()
        })
        .is_some()
        {
            candidates.push((bid, at, owner, check, element));
        }
    }
    if candidates.is_empty() {
        return stats;
    }
    let dominance = ProgramPointDominance::compute_executable_graph(func);
    let (alias, ranges) = fixed_layout_inputs(func, am);
    let facts = ScalarFacts::with_types(func, alias.exact_scalar_types());
    let neutral = crate::representation_facts::non_heap_boxed_values_from(&facts, ranges);
    for (bid, at, owner, check, element) in candidates {
        if !neutral.contains(&element)
            || !dominance.definition_available(owner, Some(check), bid, at)
        {
            continue;
        }
        let op = &mut func
            .blocks
            .get_mut(&bid)
            .expect("planned tuple read block")
            .ops[at];
        let mut copy = TirOp::new(OpCode::Copy, vec![element], op.results.clone());
        copy.inherit_source_from(op);
        *op = copy;
        stats.values_changed += 1;
    }
    stats
}
