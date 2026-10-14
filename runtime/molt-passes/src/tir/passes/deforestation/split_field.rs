//! Split-field allocation elimination over final SSA, before lifetime planning.
//!
//! Every use is checked by ValueId (including terminators and exception payloads).
//! The replacement keeps the producer's observation edge, source site and runtime
//! context, and assigns fresh SSA identities to its three property results.

use std::collections::{HashMap, HashSet};

use super::super::PassStats;
use crate::tir::dominators::ProgramPointDominance;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::opcode_canonical_kind_table;
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

fn kind(op: &TirOp) -> &str {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => kind,
        _ => opcode_canonical_kind_table(op.opcode),
    }
}

/// Explicit callable identity is not a string-primitive observation. A lowered
/// runtime target override cannot be copied onto a different primitive.
fn has_target_override(op: &TirOp) -> bool {
    [
        "runtime_symbol",
        "callee",
        "name",
        "builtin_name",
        "native_callable_symbol",
        "native_callable_export",
        "native_callable_abi",
    ]
    .iter()
    .any(|key| op.attrs.contains_key(*key))
}

fn replace_kind(op: &mut TirOp, name: &str, operands: Vec<ValueId>) {
    op.opcode = crate::tir::op_kinds_generated::kind_to_opcode_table(name).unwrap_or(OpCode::Copy);
    op.attrs
        .insert("_original_kind".into(), AttrValue::Str(name.into()));
    op.operands = operands;
    op.set_argument_custody(&vec![
        crate::ir::ParameterCustody::Borrowed;
        op.operands.len()
    ]);
    // Old inference/fast-path hints describe the removed operation, not the
    // new primitive. Source, context, custody and origin metadata stay intact.
    for key in [
        "type_hint",
        "_type_hint",
        "fast_int",
        "fast_float",
        "stack_eligible",
    ] {
        op.attrs.remove(key);
    }
}

pub fn run_split_field_reads(func: &mut TirFunction) -> PassStats {
    let mut stats = PassStats {
        name: "split_field_reads",
        ..Default::default()
    };
    if std::env::var("MOLT_DISABLE_SPLIT_FIELD_DEFOREST").is_ok()
        || !func
            .blocks
            .values()
            .any(|block| block.ops.iter().any(|op| kind(op) == "string_split_field"))
    {
        return stats;
    }
    let mut uses = HashMap::<ValueId, Vec<(crate::tir::blocks::BlockId, usize, usize)>>::new();
    let mut escaped = HashSet::new();
    let mut definitions = HashMap::new();
    for block in func.blocks.values() {
        for (index, op) in block.ops.iter().enumerate() {
            for &result in &op.results {
                definitions.insert(result, (block.id, index));
            }
            for (position, &operand) in op.operands.iter().enumerate() {
                uses.entry(operand)
                    .or_default()
                    .push((block.id, index, position));
            }
        }
        block.terminator.for_each_value(|value| {
            escaped.insert(value);
        });
    }
    let dominance = ProgramPointDominance::compute_executable_graph(func);
    let mut candidates = Vec::new();
    let mut blocks: Vec<_> = func.blocks.keys().copied().collect();
    blocks.sort_unstable();
    for &bid in &blocks {
        let block = &func.blocks[&bid];
        for (index, op) in block.ops.iter().enumerate() {
            if kind(op) != "string_split_field"
                || op.operands.len() != 3
                || op.results.len() != 1
                || op.is_async_work_poll()
                || has_target_override(op)
                || (0..op.operands.len())
                    .any(|index| op.operand_custody(index) != crate::ir::ParameterCustody::Borrowed)
            {
                continue;
            }
            let value = op.results[0];
            let Some(check) = block.ops.get(index + 1) else {
                continue;
            };
            if check.opcode != OpCode::CheckException
                || check.is_async_work_poll()
                || !matches!(check.attrs.get("value"), Some(AttrValue::Int(_)))
                || escaped.contains(&value)
            {
                continue;
            }
            let Some(reads) = uses.get(&value) else {
                continue;
            };
            let mut admitted = true;
            for &(use_bid, at, position) in reads {
                let user = &func.blocks[&use_bid].ops[at];
                // A successful producer observation must dominate each use;
                // a handler use cannot assume an allocated string exists.
                if !dominance.definition_available(bid, Some(index + 1), use_bid, at)
                    || user.is_async_work_poll()
                    || has_target_override(user)
                    || (0..user.operands.len()).any(|index| {
                        user.operand_custody(index) != crate::ir::ParameterCustody::Borrowed
                    })
                {
                    admitted = false;
                    break;
                }
                let valid = match kind(user) {
                    "len" => position == 0 && user.operands.len() == 1,
                    "ord_at" => position == 0 && user.operands.len() == 2,
                    "eq" if user.operands.len() == 2 => definitions
                        .get(&user.operands[1 - position])
                        .is_some_and(|&(b, i)| {
                            let producer = &func.blocks[&b].ops[i];
                            kind(producer) == "const_str"
                                && producer.has_valid_shape()
                                && dominance.definition_available(b, Some(i), use_bid, at)
                        }),
                    "guard_tag" | "guard_type"
                        if position == 0 && user.operands.len() == 2 && user.results.is_empty() =>
                    {
                        definitions.get(&user.operands[1]).is_some_and(|&(b, i)| {
                            let tag = &func.blocks[&b].ops[i];
                            tag.opcode == OpCode::ConstInt
                                && tag.has_valid_shape()
                                && tag.attrs.get("value") == Some(&AttrValue::Int(5))
                                && dominance.definition_available(b, Some(i), use_bid, at)
                        })
                    }
                    _ => false,
                };
                if !valid {
                    admitted = false;
                    break;
                }
            }
            if admitted {
                candidates.push((bid, index, value, reads.clone()));
            }
        }
    }
    let mut replacements = HashMap::new();
    let mut removed = HashSet::new();
    for (bid, index, value, reads) in candidates {
        let producer = func.blocks[&bid].ops[index].clone();
        let [hay, sep, field_index] = producer.operands.as_slice() else {
            unreachable!()
        };
        let (hay, sep, field_index) = (*hay, *sep, *field_index);
        let start = func.fresh_value();
        let end = func.fresh_value();
        let ascii = func.fresh_value();
        for (use_bid, at, _) in reads {
            let user = &mut func.blocks.get_mut(&use_bid).unwrap().ops[at];
            match kind(user) {
                "len" => replace_kind(
                    user,
                    "string_split_field_len_from_bounds",
                    vec![hay, start, end, ascii],
                ),
                "ord_at" => {
                    let character = user.operands[1];
                    replace_kind(
                        user,
                        "string_split_field_ord_at_bounds",
                        vec![hay, start, end, ascii, character],
                    );
                }
                "eq" => {
                    let constant = *user
                        .operands
                        .iter()
                        .find(|&&operand| operand != value)
                        .unwrap();
                    replace_kind(
                        user,
                        "string_split_field_eq",
                        vec![hay, sep, field_index, constant],
                    );
                }
                "guard_tag" | "guard_type" => {
                    removed.insert((use_bid, at));
                }
                _ => unreachable!("split-field use changed during rewrite"),
            }
        }
        let mut properties = Vec::new();
        for (name, result) in [
            ("string_split_field_start", start),
            ("string_split_field_end", end),
            ("string_split_field_is_ascii", ascii),
        ] {
            let mut property = producer.clone();
            replace_kind(&mut property, name, vec![hay, sep, field_index]);
            property.results = vec![result];
            property.attrs.remove("_simple_out");
            property
                .attrs
                .retain(|key, _| !key.starts_with("_simple_result_"));
            properties.push(property);
        }
        replacements.insert((bid, index), properties);
        stats.values_changed += 1;
    }
    if replacements.is_empty() {
        return stats;
    }
    for bid in blocks {
        let block = func.blocks.get_mut(&bid).unwrap();
        let mut ops = Vec::with_capacity(block.ops.len());
        for (index, op) in std::mem::take(&mut block.ops).into_iter().enumerate() {
            if let Some(properties) = replacements.remove(&(bid, index)) {
                ops.extend(properties);
            } else if !removed.contains(&(bid, index)) {
                ops.push(op);
            }
        }
        block.ops = ops;
    }
    stats
}
