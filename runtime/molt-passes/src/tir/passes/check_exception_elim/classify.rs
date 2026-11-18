use molt_ir::deterministic_hash::HashMap;

use crate::tir::op_kinds_generated::{
    LiteralPayloadKind, opcode_literal_payload_kind_table,
    opcode_requires_i64_shift_count_guard_table, opcode_requires_i64_zero_divisor_guard_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

use super::super::effects::{guarded_throw_condition_disproven, op_may_throw_with_types};

pub(crate) fn const_int_values(func: &crate::tir::function::TirFunction) -> HashMap<ValueId, i64> {
    let mut values = HashMap::default();
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
                Some(
                    LiteralPayloadKind::Float
                    | LiteralPayloadKind::None
                    | LiteralPayloadKind::Owned(_),
                )
                | None => None,
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

/// A shift whose count is a constant machine count, `[0, 63]`: a negative
/// count raises `ValueError`, and a larger one is not a machine shift.
fn proven_i64_shift_count(const_ints: &HashMap<ValueId, i64>, op: &TirOp) -> bool {
    let [_value, count] = op.operands.as_slice() else {
        return false;
    };
    const_ints.get(count).is_some_and(|count| (0..64).contains(count))
}

/// What every "can this op raise" question reads: the exact scalar types the
/// function proves and its integer constants, built from the function itself.
/// One builder means no pass asks with weaker facts than another: the
/// exception-region verifier and the fast-path expansion passed no constants
/// and the plain `value_types`, so the verifier rejected checks this pass had
/// proven dead (`if i % 3 < 2:` in a range loop crashed the build, HF-891).
#[derive(Debug)]
pub(crate) struct RaiseFacts {
    value_types: HashMap<ValueId, TirType>,
    const_ints: HashMap<ValueId, i64>,
}

impl RaiseFacts {
    pub(crate) fn of(func: &crate::tir::function::TirFunction) -> Self {
        Self {
            value_types: crate::tir::type_refine::extract_exact_scalar_map(func),
            const_ints: const_int_values(func),
        }
    }

    pub(crate) fn op_may_raise(&self, op: &TirOp) -> bool {
        op_may_raise(&self.value_types, &self.const_ints, op)
    }

    /// Whether the op's own facts say it may raise but its operand values
    /// disprove every raise (a nonzero constant divisor, a constant machine
    /// shift count). The caller records the proof on the op, so every reader
    /// of op effects (frame observers, home placement) sees it (HF-866).
    pub(crate) fn guarded_raise_disproven(&self, op: &TirOp) -> bool {
        !op.has_nothrow_proof()
            && op_may_throw_with_types(op, &self.value_types)
            && guards_disprove_raise(&self.value_types, &self.const_ints, op)
    }
}

fn guards_disprove_raise(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    op: &TirOp,
) -> bool {
    (opcode_requires_i64_zero_divisor_guard_table(op.opcode)
        || opcode_requires_i64_shift_count_guard_table(op.opcode))
        && guarded_throw_condition_disproven(
            op,
            value_types,
            proven_i64_shift_count(const_ints, op),
            proven_nonzero_i64_divisor(const_ints, op),
        )
}

fn op_may_raise(
    value_types: &HashMap<ValueId, TirType>,
    const_ints: &HashMap<ValueId, i64>,
    op: &TirOp,
) -> bool {
    if op.is_async_work_poll() {
        return true;
    }
    if guards_disprove_raise(value_types, const_ints, op) {
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
        op.origin,
        Some(orig) if orig == molt_ir::OpKind::ExceptionClear
    )
}

/// Only a targeted check transfers the pending case away from fallthrough.
/// An untargeted observer leaves existing failures untouched, and a fused poll
/// can create a failure even when its incoming state was proven clean.
pub(super) fn check_has_exception_edge(op: &TirOp) -> bool {
    op.opcode == OpCode::CheckException && matches!(op.attrs.get("value"), Some(AttrValue::Int(_)))
}

pub(super) fn pending_after_check(op: &TirOp, pending: bool) -> bool {
    if check_has_exception_edge(op) {
        false
    } else {
        pending || op.is_async_work_poll()
    }
}
