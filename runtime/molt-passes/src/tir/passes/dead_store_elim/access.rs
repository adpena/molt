use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;
/// Returns `Some(offset)` when this op is a `store` or `store_init`
/// against a typed-class instance slot at a known integer offset.
///
/// Conservatism: any other StoreAttr variant (set_attr_name,
/// guarded_field_set, etc.) returns `None`, leaving the op untouched.
fn store_offset(op: &TirOp) -> Option<i64> {
    if op.opcode != OpCode::StoreAttr {
        return None;
    }
    let original = match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => s.as_str(),
        _ => return None,
    };
    if !matches!(original, "store" | "store_init") {
        return None;
    }
    match op.attrs.get("value") {
        Some(AttrValue::Int(v)) => Some(*v),
        _ => None,
    }
}

/// Returns `Some((target, offset))` for the narrow typed-class slot
/// store contract this pass understands.
pub(super) fn typed_slot_store(op: &TirOp) -> Option<(ValueId, i64)> {
    if op.operands.len() != 2 {
        return None;
    }
    Some((op.operands[0], store_offset(op)?))
}

pub(super) fn stack_object_alloc_result(op: &TirOp) -> Option<ValueId> {
    if op.opcode != OpCode::ObjectNewBoundStack {
        return None;
    }
    if !matches!(op.attrs.get("value"), Some(AttrValue::Int(_))) {
        return None;
    }
    if op.results.len() != 1 {
        return None;
    }
    Some(op.results[0])
}
