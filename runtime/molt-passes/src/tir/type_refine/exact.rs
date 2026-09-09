//! Exact scalar provenance, distinct from annotation/guard type refinement.
//!
//! A Python annotation admits overriding subclasses. Intrinsic producers and
//! computations on exact operands do not. This monotone SSA transfer uses the
//! existing generated result/effect rules without importing return hints.

use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    comparison_scalar_domain, opcode_effects_table, opcode_exact_scalar_result_tir_type,
};
use crate::tir::ops::OpCode;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::cfg_edges::collect_branch_edges;
use super::result_inference::infer_result_facts_with_attrs;

fn scalar_or_bottom(ty: TirType) -> TirType {
    if ty == TirType::Never || comparison_scalar_domain(&ty).is_some() {
        ty
    } else {
        TirType::DynBox
    }
}

pub fn extract_exact_scalar_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let reachable = crate::tir::dominators::executable_reachable_blocks(func);
    let mut facts = HashMap::new();
    let mut incoming: HashMap<BlockId, Vec<Vec<ValueId>>> = HashMap::new();
    let mut order: Vec<_> = reachable.iter().copied().collect();
    order.sort_by_key(|id| id.0);
    for block in func
        .blocks
        .values()
        .filter(|block| reachable.contains(&block.id))
    {
        for arg in &block.args {
            facts.insert(arg.id, TirType::Never);
        }
        for op in &block.ops {
            for &result in &op.results {
                facts.insert(result, TirType::Never);
            }
        }
        for (target, args) in collect_branch_edges(block) {
            incoming.entry(target).or_default().push(args);
        }
    }
    // The finite scalar lattice rises from bottom through exact types to top.
    // Cyclic SSA paths retain bottom until an executable incoming fact seeds
    // them; no annotation is used to bootstrap a loop or function parameter.
    loop {
        let mut changed = false;
        for id in &order {
            let block = &func.blocks[id];
            // Match the type-refinement EH admission fact. label_id_map also
            // contains ordinary control-flow labels, not just handler entries.
            let handler = func.has_exception_handling
                && (func.label_id_map.contains_key(&id.0)
                    || block.ops.first().is_some_and(|op| {
                        matches!(op.opcode, OpCode::StateBlockStart | OpCode::CheckException)
                    }));
            for (index, arg) in block.args.iter().enumerate() {
                let ty = if handler || *id == func.entry_block {
                    TirType::DynBox
                } else if let Some(edges) = incoming.get(id) {
                    edges.iter().fold(TirType::Never, |ty, edge| {
                        let incoming = edge
                            .get(index)
                            .and_then(|value| facts.get(value))
                            .unwrap_or(&TirType::DynBox);
                        ty.meet(incoming)
                    })
                } else {
                    TirType::DynBox
                };
                let next = scalar_or_bottom(facts[&arg.id].meet(&ty));
                if facts[&arg.id] != next {
                    facts.insert(arg.id, next);
                    changed = true;
                }
            }
            for op in &block.ops {
                let operand_types: Vec<_> = op
                    .operands
                    .iter()
                    .map(|value| facts.get(value).cloned().unwrap_or(TirType::DynBox))
                    .collect();
                let effects = opcode_effects_table(op.opcode);
                let copied = crate::tir::passes::value_identity::copy_value_source(op);
                let transfers_exact =
                    op.opcode != OpCode::Copy && effects.consistent && effects.effect_free;
                let types = if !op.has_valid_result_arity() {
                    vec![TirType::DynBox; op.results.len()]
                } else if let Some(source) = copied {
                    vec![facts.get(&source).cloned().unwrap_or(TirType::DynBox)]
                } else if let Some(predicate) =
                    crate::tir::predicate_semantics::predicate_facts(op.opcode, &operand_types)
                {
                    vec![predicate.result_type; op.results.len()]
                } else if transfers_exact {
                    infer_result_facts_with_attrs(op.opcode, &operand_types, None, op.results.len())
                } else {
                    vec![TirType::DynBox; op.results.len()]
                };
                for (index, (&result, inferred)) in op.results.iter().zip(types).enumerate() {
                    // Output shape is independent of purity: a mutable runtime
                    // read or checked arithmetic may still produce an exact
                    // Boolean status. Never broadcast slot zero into siblings.
                    let ty = if op.has_valid_result_arity() {
                        opcode_exact_scalar_result_tir_type(op.opcode, index).unwrap_or(inferred)
                    } else {
                        inferred
                    };
                    let next = scalar_or_bottom(facts[&result].meet(&ty));
                    if facts[&result] != next {
                        facts.insert(result, next);
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    facts.retain(|_, ty| comparison_scalar_domain(ty).is_some());
    facts
}
