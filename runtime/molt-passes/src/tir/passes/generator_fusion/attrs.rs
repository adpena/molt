use super::super::super::ops::{AttrValue, OpCode, TirOp};

/// Read an op's integer `value` attr (slot offset / next-state id).
pub(in crate::tir::passes::generator_fusion) fn attr_value_int(op: &TirOp) -> Option<i64> {
    match op.attrs.get("value") {
        Some(AttrValue::Int(v)) => Some(*v),
        _ => None,
    }
}

/// Read an op's `s_value` string attr (poll function name).
pub(in crate::tir::passes::generator_fusion) fn attr_s_value(op: &TirOp) -> Option<&str> {
    match op.attrs.get("s_value") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Read an op's `task_kind` string attr.
pub(in crate::tir::passes::generator_fusion) fn attr_task_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("task_kind") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Read an op's `_original_kind` string attr (the SimpleIR op-name annotation
/// preserved on `Copy`-lowered ops such as `iter`).
pub(in crate::tir::passes::generator_fusion) fn attr_original_kind(op: &TirOp) -> Option<&str> {
    match op.attrs.get("_original_kind") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// True if `op` is the consumer's `GetIter` over a value — either the
/// first-class [`OpCode::GetIter`] or the runtime `iter` op (lowered as a
/// `Copy` carrying `_original_kind == "iter"`, the form the frontend emits for
/// `for x in <expr>`).
pub(in crate::tir::passes::generator_fusion) fn is_get_iter_op(op: &TirOp) -> bool {
    op.opcode == OpCode::GetIter
        || (op.opcode == OpCode::Copy && attr_original_kind(op) == Some("iter"))
}
