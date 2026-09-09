use std::collections::HashMap;

use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// One return-contract projection shared by initial lifting and refinement.
/// Only exact producer facts may enter this ABI projection, never annotations
/// or subtype guards whose operands can override scalar Python operations.
pub(crate) fn infer_return_type<'a>(
    blocks: impl Iterator<Item = &'a crate::tir::blocks::TirBlock>,
    exact_scalar_types: &HashMap<ValueId, TirType>,
) -> TirType {
    let mut result_type: Option<TirType> = None;
    for block in blocks {
        if let crate::tir::blocks::Terminator::Return { values } = &block.terminator {
            let ty = if values.is_empty() {
                TirType::None
            } else {
                exact_scalar_types
                    .get(&values[0])
                    .cloned()
                    .unwrap_or(TirType::DynBox)
            };
            result_type = Some(result_type.map_or_else(|| ty.clone(), |old| old.meet(&ty)));
        }
    }
    result_type.unwrap_or(TirType::None)
}

pub(super) fn fact_or_bottom(facts: &HashMap<ValueId, TirType>, id: ValueId) -> TirType {
    facts.get(&id).cloned().unwrap_or(TirType::Never)
}

pub(super) fn is_bottom_type(ty: &TirType) -> bool {
    matches!(ty, TirType::Never)
}

pub(super) fn contains_bottom_type(ty: &TirType) -> bool {
    match ty {
        TirType::Never => true,
        TirType::List(inner)
        | TirType::Set(inner)
        | TirType::Iterator(inner)
        | TirType::Box(inner)
        | TirType::Ptr(inner) => contains_bottom_type(inner),
        TirType::Dict(key, value) => contains_bottom_type(key) || contains_bottom_type(value),
        TirType::Tuple(items) | TirType::Union(items) => items.iter().any(contains_bottom_type),
        _ => false,
    }
}

pub(super) fn publish_fact_type(ty: TirType) -> TirType {
    if is_bottom_type(&ty) {
        TirType::DynBox
    } else {
        ty
    }
}

pub(super) fn is_refined_public_type(ty: &TirType) -> bool {
    !matches!(ty, TirType::DynBox | TirType::Never)
}

pub(super) fn join_assign_type_fact(
    facts: &mut HashMap<ValueId, TirType>,
    id: ValueId,
    incoming: TirType,
) -> bool {
    let current = fact_or_bottom(facts, id);
    let joined = current.meet(&incoming);
    if joined != current {
        facts.insert(id, joined);
        true
    } else {
        false
    }
}
