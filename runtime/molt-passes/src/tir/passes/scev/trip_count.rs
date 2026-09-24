use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{ScevExpr, TripCount, ordered_comparison_trip_count};
use crate::tir::op_kinds_generated::{
    opcode_counted_loop_comparison_role_table, opcode_counted_loop_inverted_comparison_table,
};
use crate::tir::values::ValueId;

use super::builder::ScevBuilder;
use super::index::DefIndex;

/// Derive a loop's trip count from its canonical guard `Lt(iv, stop)` /
/// `Gt(iv, stop)` and the IV's `AddRec`.
pub(super) fn compute_trip_count(
    func: &TirFunction,
    defs: &DefIndex,
    iv_of_header: &HashMap<BlockId, ValueId>,
    builder: &mut ScevBuilder,
    header: BlockId,
) -> TripCount {
    let iv = match iv_of_header.get(&header) {
        Some(&iv) => iv,
        None => return TripCount::Unknown,
    };
    let body = match builder.loops.bodies.get(&header) {
        Some(b) => b.clone(),
        None => return TripCount::Unknown,
    };
    let guard = match builder.guards.material_guard(func, header, &body) {
        Some(guard) => guard,
        None => return TripCount::Unknown,
    };
    // Every recurrence advance must have passed the continuing edge. Handler
    // reentry or an exceptional jump to the header cannot carry that proof.
    if !builder
        .guards
        .recurrence_is_guarded(func, header, &body, &guard)
    {
        return TripCount::Unknown;
    }
    let (mut opcode, raw_operands, _nsw) = match defs.def_op.get(&guard.condition).cloned() {
        Some(t) => t,
        None => return TripCount::Unknown,
    };
    if !guard.continue_on_true {
        let Some(inverted) = opcode_counted_loop_inverted_comparison_table(opcode) else {
            return TripCount::Unknown;
        };
        opcode = inverted;
    }
    if raw_operands.len() != 2 {
        return TripCount::Unknown;
    }
    // Resolve guard operands through plain copies so `Lt(Copy(iv), Copy(stop))`
    // names the canonical iv / stop values.
    let operands: Vec<ValueId> = raw_operands.iter().map(|&o| defs.resolve(o)).collect();
    let iv = defs.resolve(iv);
    // Recover the IV's recurrence: start, step.
    let (start, step) = match builder.scev(iv) {
        ScevExpr::AddRec { start, step, .. } => (*start, *step),
        _ => return TripCount::Unknown,
    };

    // Identify which operand is the iv and which is the bound.
    let (lhs, rhs) = (operands[0], operands[1]);
    let (bound_val, iv_is_lhs) = if lhs == iv {
        (rhs, true)
    } else if rhs == iv {
        (lhs, false)
    } else {
        return TripCount::Unknown;
    };

    // Canonical positive loop: `Lt(iv, stop)` with start s0, step +k>0.
    // trip = ceil((stop - s0) / k) when stop > s0, else 0.
    let step_const = match step.as_constant() {
        Some(k) => k,
        None => return TripCount::Unknown,
    };

    let role = opcode_counted_loop_comparison_role_table(opcode);
    let positive_guard =
        role.is_ordered() && role.requires_positive_step() && iv_is_lhs && step_const > 0;
    let negative_guard =
        role.is_ordered() && !role.requires_positive_step() && iv_is_lhs && step_const < 0;
    if !positive_guard && !negative_guard {
        return TripCount::Unknown;
    }

    let start_const = start.as_constant();
    let bound_const = defs.const_int.get(&bound_val).copied();

    if let (Some(s0), Some(stop), k) = (start_const, bound_const, step_const) {
        return ordered_comparison_trip_count(role, s0, stop, k)
            .map(TripCount::Constant)
            .unwrap_or(TripCount::Unknown);
    }

    // Symbolic: positive unit-step loop `for i in range(stop)` from 0 with
    // step +1 → trip count == stop (a loop-invariant expression). Only emit a
    // symbolic trip when start==0 and step==1 (the dominant `range(stop)`
    // shape), where trip == stop exactly.
    if positive_guard && !role.is_inclusive() && step_const == 1 && start_const == Some(0) {
        let bound_scev = builder.scev(bound_val);
        if !matches!(bound_scev, ScevExpr::Unknown) {
            return TripCount::Symbolic(Box::new(bound_scev));
        }
    }

    TripCount::Unknown
}
