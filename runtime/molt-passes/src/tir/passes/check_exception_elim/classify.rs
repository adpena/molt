use std::collections::HashMap;

use crate::tir::op_kinds_generated::{
    LiteralPayloadKind, opcode_literal_payload_kind_table,
    opcode_requires_i64_zero_divisor_guard_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::super::effects::{guarded_throw_condition_disproven, op_may_throw_with_types};

pub(crate) fn const_int_values(func: &crate::tir::function::TirFunction) -> HashMap<ValueId, i64> {
    let mut values = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            let value = match opcode_literal_payload_kind_table(op.opcode) {
                Some(LiteralPayloadKind::Int) => match op.attrs.get("value") {
                    Some(AttrValue::Int(value)) => Some(*value),
                    _ => None,
                },
                Some(LiteralPayloadKind::Bool) => match op.attrs.get("value") {
                    Some(AttrValue::Bool(value)) => Some(i64::from(*value)),
                    Some(AttrValue::Int(value)) => Some(i64::from(*value != 0)),
                    _ => None,
                },
                None => None,
            };
            if let Some(value) = value {
                for result in &op.results {
                    values.insert(*result, value);
                }
            }
        }
    }
    values
}

fn proven_nonzero_i64_divisor(const_ints: &HashMap<ValueId, i64>, op: &TirOp) -> bool {
    let [_lhs, rhs] = op.operands.as_slice() else {
        return false;
    };
    const_ints.get(rhs).is_some_and(|value| *value != 0)
}

pub(crate) fn op_may_raise(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    op: &TirOp,
) -> bool {
    if op.is_async_work_poll() {
        return true;
    }
    if opcode_requires_i64_zero_divisor_guard_table(op.opcode)
        && guarded_throw_condition_disproven(
            op,
            value_types,
            false,
            proven_nonzero_i64_divisor(const_ints, op),
        )
    {
        return false;
    }
    if op_may_throw_with_types(op, value_types) {
        return true;
    }
    false
}

pub(crate) fn op_clears_pending_exception(op: &TirOp) -> bool {
    if op.opcode != OpCode::Copy {
        return false;
    }
    matches!(
        op.attrs.get("_original_kind"),
        Some(AttrValue::Str(orig)) if orig == "exception_clear"
    )
}
