use std::collections::HashMap;

use crate::tir::op_kinds_generated::{
    LiteralPayloadKind, opcode_literal_payload_kind_table,
    opcode_requires_i64_zero_divisor_guard_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::super::effects::op_may_throw;

/// SimpleIR op kinds that fall through to `OpCode::Copy` in the SSA lift
/// (so they carry `_original_kind`) but are nevertheless provably
/// non-throwing. Anything not on this list is treated as throwing: the
/// conservative choice consistent with DCE's safety policy.
fn original_kind_is_provably_nonthrowing(kind: &str) -> bool {
    matches!(
        kind,
        "guard_tag"
            | "guard_layout"
            | "guard_int"
            | "guard_float"
            | "guard_str"
            | "guard_bool"
            | "guard_none"
            | "store"
            | "load"
            | "exception_clear"
            | "exception_last"
            | "exception_last_pending"
            | "exception_finally_pending_observer"
            | "exception_pop"
            | "exception_push"
            | "exception_new_builtin"
            | "exception_new_builtin_empty"
            | "exception_new_builtin_one"
            | "exception_match_builtin"
            | "exception_stack_enter"
            | "exception_stack_clear"
            | "exception_stack_depth"
            | "exception_context_set"
            | "try_start"
            | "try_end"
            | "context_depth"
            | "trace_enter_slot"
            | "trace_exit"
            | "line"
            | "code_slots_init"
            | "code_slot_set"
            | "code_new"
            | "is"
            | "is_not"
            | "not"
            | "and"
            | "or"
            | "bool"
            | "loop_start"
            | "loop_end"
            | "loop_continue"
            | "loop_break"
            | "loop_break_if_false"
            | "loop_index_start"
            | "loop_index_next"
            | "missing"
            | "phi"
            | "identity_alias"
            | "copy_var"
    )
}

pub(super) fn const_int_values(func: &crate::tir::function::TirFunction) -> HashMap<ValueId, i64> {
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

fn value_is_i64(value_types: &HashMap<ValueId, TirType>, value: ValueId) -> bool {
    matches!(value_types.get(&value), Some(TirType::I64))
}

fn proven_nonzero_i64_divisor(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    op: &TirOp,
) -> bool {
    let [lhs, rhs] = op.operands.as_slice() else {
        return false;
    };
    value_is_i64(value_types, *lhs)
        && value_is_i64(value_types, *rhs)
        && const_ints.get(rhs).is_some_and(|value| *value != 0)
}

pub(super) fn op_may_raise(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    op: &TirOp,
) -> bool {
    if opcode_requires_i64_zero_divisor_guard_table(op.opcode)
        && proven_nonzero_i64_divisor(value_types, const_ints, op)
    {
        return false;
    }
    if op_may_throw(op) {
        return true;
    }
    if op.opcode == OpCode::Copy {
        if let Some(AttrValue::Str(orig)) = op.attrs.get("_original_kind") {
            return !original_kind_is_provably_nonthrowing(orig);
        }
        return false;
    }
    false
}

pub(super) fn op_clears_pending_exception(op: &TirOp) -> bool {
    if op.opcode != OpCode::Copy {
        return false;
    }
    matches!(
        op.attrs.get("_original_kind"),
        Some(AttrValue::Str(orig)) if orig == "exception_clear"
    )
}
