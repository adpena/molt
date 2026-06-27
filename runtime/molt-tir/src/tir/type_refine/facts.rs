use std::collections::HashMap;

use crate::tir::types::TirType;
use crate::tir::values::ValueId;

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
