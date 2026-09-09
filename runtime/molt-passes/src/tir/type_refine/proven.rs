use std::collections::HashMap;

use super::extract_type_map_with_exact;
use super::guards::propagate_guard_types;
use super::result_inference::infer_result_types_with_attrs;
use crate::tir::blocks::BlockId;
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::op_kinds_generated::{
    opcode_is_proven_result_type_seed_table, opcode_operand_independent_result_tir_type,
};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Extract a map of values whose types are **proven** (not speculated).
///
/// A type is proven if it comes from:
/// - A constant op (ConstInt, ConstFloat, etc.)
/// - An operand-independent intrinsic scalar result slot, including status flags
/// - A TypeGuard success path
/// - Arithmetic on proven values
///
/// This map is a subset of the full type_map. The native backend can skip
/// redundant guards for values in this map.
pub fn extract_proven_map(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let exact = super::extract_exact_scalar_map(func);
    // Seed each valid result slot independently. An exact status result neither
    // proves a neighboring payload nor changes the producer's effects.
    let mut proven: HashMap<ValueId, TirType> = HashMap::new();

    let mut block_order: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_order.sort_by_key(|b| b.0);

    for &bid in &block_order {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            if !op.has_valid_result_arity() {
                continue;
            }
            for (index, &result) in op.results.iter().enumerate() {
                if let Some(ty) = opcode_operand_independent_result_tir_type(op.opcode, index)
                    && (opcode_is_proven_result_type_seed_table(op.opcode)
                        || crate::tir::op_kinds_generated::comparison_scalar_domain(&ty).is_some())
                {
                    proven.insert(result, ty);
                }
            }
        }
    }

    // Run the guard propagation to add TypeGuard-proven values.
    let mut env = extract_type_map_with_exact(func, &exact);
    let pred_map = dominators::build_pred_map(func);
    let idoms = dominators::compute_idoms(func, &pred_map);
    let (_refinements, guard_proven) = propagate_guard_types(func, &mut env, &idoms, &exact);
    for (vid, ty) in guard_proven {
        proven.insert(vid, ty);
    }

    // Propagate through arithmetic chains.
    for &bid in &block_order {
        let block = &func.blocks[&bid];
        for op in &block.ops {
            if op.results.is_empty() {
                continue;
            }
            let all_proven =
                !op.operands.is_empty() && op.operands.iter().all(|v| proven.contains_key(v));
            if !all_proven {
                continue;
            }
            let operand_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| proven.get(id).cloned().unwrap_or(TirType::DynBox))
                .collect();
            let result_types = if let Some(facts) =
                crate::tir::predicate_semantics::predicate_facts_for_op(op, &exact)
            {
                vec![Some(facts.result_type); op.results.len()]
            } else {
                infer_result_types_with_attrs(
                    op.opcode,
                    &operand_types,
                    Some(&op.attrs),
                    op.results.len(),
                )
            };
            for (&result_id, result_ty) in op.results.iter().zip(result_types) {
                if let Some(result_ty) = result_ty {
                    proven.insert(result_id, result_ty.clone());
                }
            }
        }
    }

    proven
}
