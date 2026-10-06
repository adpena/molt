use super::super::{ConstVal, admits_constant_result};
use crate::tir::ops::{OpCode, builtin_call_view};

/// Only an explicit primitive identity can establish a constant call result.
/// Public names, including constant dynamic-name operands, select mutable
/// namespace lookup and provide no proof about the callee or its result.
pub(in crate::tir::passes::sccp) fn evaluate_builtin_call(
    op: &crate::tir::ops::TirOp,
    operands: &[Option<&ConstVal>],
) -> Option<ConstVal> {
    if op.opcode != OpCode::CallBuiltin
        || !admits_constant_result(op)
        || operands.len() != op.operands.len()
    {
        return None;
    }
    let call = builtin_call_view(op.opcode, &op.attrs, operands)?;
    if call.wire_kind != "range_new" {
        return None;
    }
    let [
        Some(ConstVal::Int(start)),
        Some(ConstVal::Int(stop)),
        Some(ConstVal::Int(step)),
    ] = call.arguments
    else {
        return None;
    };
    (*step != 0).then_some(ConstVal::Range {
        start: *start,
        stop: *stop,
        step: *step,
    })
}
