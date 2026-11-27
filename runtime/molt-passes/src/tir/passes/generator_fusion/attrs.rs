use super::super::super::ops::{AttrValue, TirOp};

/// Read an op's integer `value` attr (slot offset / next-state id).
pub(in crate::tir::passes::generator_fusion) fn attr_value_int(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(v)) => Some(*v),
        _ => None,
    }
}

/// The poll function an `AllocTask` names, by its typed callee.
pub(in crate::tir::passes::generator_fusion) fn poll_callee(
    op: &TirOp,
) -> Option<molt_ir::FunctionId> {
    op.callee
}

/// Read an op's typed `origin` field (the SimpleIR op-name annotation
/// preserved on `Copy`-lowered ops such as `iter`).
pub(in crate::tir::passes::generator_fusion) fn attr_original_kind(
    op: &TirOp,
) -> Option<molt_ir::OpKind> {
    op.origin
}
