//! Structural type inference helpers for SimpleIR to TIR lowering.
//!
//! The lowering pipeline seeds function parameter types, then uses these
//! helpers to infer scalar operation results and the assembled return type
//! before backend consumers inspect the TIR function contract.

use std::collections::HashMap;

use super::super::blocks::TirBlock;
use super::super::types::TirType;
use super::super::values::ValueId;

/// Forward type propagation for scalar return-relevant operation results.
///
/// Parameter signatures are seeded before this pass, so the same canonical
/// scalar result inference used by TIR refinement can derive return contracts
/// from typed parameters, constants, and prior propagation. Container and
/// aggregate types are deliberately left to the full refinement pipeline after
/// TIR construction so this pre-assembly pass cannot duplicate richer type
/// lattice behavior.
/// This runs iteratively until no new types are discovered.
pub(super) fn propagate_arithmetic_types(
    blocks: &[TirBlock],
    types: &mut HashMap<ValueId, TirType>,
) {
    let mut changed = true;
    while changed {
        changed = false;
        for block in blocks {
            for op in &block.ops {
                if op.results.is_empty() {
                    continue;
                }
                let result_id = op.results[0];
                // Skip if already typed
                if types.get(&result_id).is_some_and(|t| *t != TirType::DynBox) {
                    continue;
                }

                let operand_types: Vec<TirType> = op
                    .operands
                    .iter()
                    .map(|id| types.get(id).cloned().unwrap_or(TirType::DynBox))
                    .collect();
                if let Some(inferred) = super::super::type_refine::infer_scalar_return_result_type(
                    op.opcode,
                    &operand_types,
                    Some(&op.attrs),
                ) {
                    types.insert(result_id, inferred);
                    changed = true;
                }
            }
        }
    }
}

/// Convert a string type annotation to a `TirType`.
pub(super) fn string_to_tir_type(s: &str) -> TirType {
    match s {
        "int" | "i64" => TirType::I64,
        "float" | "f64" => TirType::F64,
        _ => match TirType::from_type_hint(s) {
            TirType::UserClass(_) => TirType::DynBox,
            ty => ty,
        },
    }
}

pub(super) fn param_string_to_tir_type(s: &str) -> TirType {
    match s {
        "i64" => TirType::DynBox,
        _ => string_to_tir_type(s),
    }
}

/// Infer the function return type by examining all Return terminators.
/// Uses a lattice meet to combine multiple return types.
pub(super) fn infer_return_type(blocks: &[TirBlock], types: &HashMap<ValueId, TirType>) -> TirType {
    use super::super::blocks::Terminator;

    let mut result_type: Option<TirType> = None;

    for block in blocks {
        if let Terminator::Return { values } = &block.terminator {
            let ret_ty = if values.is_empty() {
                TirType::None
            } else {
                // Use the type of the first return value.
                values
                    .first()
                    .and_then(|vid| types.get(vid))
                    .cloned()
                    .unwrap_or(TirType::DynBox)
            };

            result_type = Some(match result_type {
                None => ret_ty,
                Some(existing) => existing.meet(&ret_ty),
            });
        }
    }

    result_type.unwrap_or(TirType::None)
}
