//! Exact scalar provenance, distinct from annotation/guard type refinement.
//!
//! A Python annotation admits overriding subclasses. Intrinsic producers and
//! computations on exact operands do not. This monotone SSA transfer uses the
//! existing generated result/effect rules without importing return hints.

use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    comparison_scalar_domain, opcode_exact_scalar_result_tir_type,
};
use crate::tir::ops::OpCode;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::cfg_edges::{collect_branch_edges, handler_entry_blocks};
use super::result_inference::infer_result_facts_with_attrs;

fn scalar_or_bottom(ty: TirType) -> TirType {
    if ty == TirType::Never || comparison_scalar_domain(&ty).is_some() {
        ty
    } else {
        TirType::DynBox
    }
}

/// Permission to interpret a producer's scalar payload. Only the exact SSA
/// solver can construct this witness; declarations and guards cannot mint it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ExactScalarWitness {
    kind: crate::repr::ScalarKind,
}

impl ExactScalarWitness {
    pub fn kind(self) -> crate::repr::ScalarKind {
        self.kind
    }

    pub fn ty(self) -> TirType {
        use crate::repr::ScalarKind;
        match self.kind {
            ScalarKind::Int => TirType::I64,
            ScalarKind::Bool => TirType::Bool,
            ScalarKind::Float => TirType::F64,
            ScalarKind::Str => TirType::Str,
            ScalarKind::NoneValue => TirType::None,
        }
    }
}

/// Project the exact solver's facts into non-forgeable lowering permissions.
pub fn exact_scalar_witnesses(func: &TirFunction) -> HashMap<ValueId, ExactScalarWitness> {
    use crate::repr::ScalarKind;
    extract_exact_scalar_map(func)
        .into_iter()
        .filter_map(|(id, ty)| {
            let kind = match ty.semantic_type() {
                TirType::I64 | TirType::BigInt => ScalarKind::Int,
                TirType::Bool => ScalarKind::Bool,
                TirType::F64 => ScalarKind::Float,
                TirType::Str => ScalarKind::Str,
                TirType::None => ScalarKind::NoneValue,
                _ => return None,
            };
            Some((id, ExactScalarWitness { kind }))
        })
        .collect()
}

pub fn extract_exact_scalar_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let reachable = crate::tir::dominators::executable_reachable_blocks(func);
    let handlers = handler_entry_blocks(func);
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
            for (index, arg) in block.args.iter().enumerate() {
                let ty = if handlers.contains(id) || *id == func.entry_block {
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
                let instance =
                    crate::tir::op_semantics::op_instance_facts(op.opcode, &operand_types);
                let effects = instance.as_ref().map_or_else(
                    || crate::tir::op_kinds_generated::opcode_effects_table(op.opcode),
                    |facts| facts.effects,
                );
                let scalar_source = crate::tir::passes::value_identity::scalar_value_source(op);
                let transfers_exact =
                    op.opcode != OpCode::Copy && effects.consistent && effects.effect_free;
                let valid_shape = op.has_valid_shape();
                let types = if !valid_shape {
                    vec![TirType::DynBox; op.results.len()]
                } else if let Some(proven) = op.exact_result_proof() {
                    // The late stage replaced an op whose result was exact.
                    vec![proven; op.results.len()]
                } else if let Some(source) = scalar_source {
                    vec![facts.get(&source).cloned().unwrap_or(TirType::DynBox)]
                } else if op.opcode == OpCode::Copy
                    && op.origin.is_some_and(
                        crate::tir::op_kinds_generated::copy_kind_has_exact_int_result_table,
                    )
                {
                    // An exact int of any magnitude: the carrier stays boxed.
                    vec![TirType::BigInt; op.results.len()]
                } else if let Some(result_type) = instance.and_then(|facts| facts.result_type) {
                    vec![result_type; op.results.len()]
                } else if let Some(ty) =
                    crate::tir::call_targets::gpu_runtime_result_type_for_op(op)
                {
                    vec![ty; op.results.len()]
                } else if transfers_exact {
                    infer_result_facts_with_attrs(
                        op.opcode,
                        op.origin,
                        &operand_types,
                        None,
                        op.results.len(),
                    )
                } else {
                    vec![TirType::DynBox; op.results.len()]
                };
                for (index, (&result, inferred)) in op.results.iter().zip(types).enumerate() {
                    // Output shape is independent of purity: a mutable runtime
                    // read or checked arithmetic may still produce an exact
                    // Boolean status. Never broadcast slot zero into siblings.
                    let ty = if valid_shape {
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
