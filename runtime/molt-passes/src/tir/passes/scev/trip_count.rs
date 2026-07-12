use std::collections::{HashMap, HashSet};

use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{ScevExpr, TripCount, ordered_comparison_trip_count};
use crate::tir::op_kinds_generated::CountedLoopComparisonRole;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::builder::ScevBuilder;
use super::index::DefIndex;

/// Find the loop's exit-test `CondBranch` condition value. The condition is
/// usually not in the header itself: after lowering, the header unconditionally
/// branches to a *guard block* that holds the `CondBranch`. We walk from the
/// header through unconditional `Branch`es (staying inside the loop body) to the
/// first `CondBranch` whose successors split the loop body from outside it — the
/// canonical single loop exit test. Returns `(guard_block, cond_value)`.
///
/// This is shared (imported by `value_range`) so SCEV trip counts and
/// value-range guard narrowing reason about the exact same guard.
pub(crate) fn find_loop_guard(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
) -> Option<(BlockId, ValueId)> {
    let mut cur = header;
    // Bounded walk through the unconditional-branch chain from the header.
    for _ in 0..8 {
        let block = func.blocks.get(&cur)?;
        match &block.terminator {
            Terminator::CondBranch {
                cond,
                then_block,
                else_block,
                ..
            } => {
                // A genuine loop exit test: exactly one successor stays in the
                // body and the other leaves it.
                let then_in = body.contains(then_block);
                let else_in = body.contains(else_block);
                if then_in != else_in {
                    return Some((cur, *cond));
                }
                return None;
            }
            Terminator::Branch { target, .. } => {
                if !body.contains(target) || *target == header {
                    return None;
                }
                cur = *target;
            }
            _ => return None,
        }
    }
    None
}

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
    let cond = match find_loop_guard(func, header, &body) {
        Some((_, c)) => c,
        None => return TripCount::Unknown,
    };
    let (opcode, raw_operands, _nsw) = match defs.def_op.get(&cond).cloned() {
        Some(t) => t,
        None => return TripCount::Unknown,
    };
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

    let positive_guard = matches!(opcode, OpCode::Lt) && iv_is_lhs && step_const > 0;
    let negative_guard = matches!(opcode, OpCode::Gt) && iv_is_lhs && step_const < 0;
    if !positive_guard && !negative_guard {
        return TripCount::Unknown;
    }

    let start_const = start.as_constant();
    let bound_const = defs.const_int.get(&bound_val).copied();

    if let (Some(s0), Some(stop), k) = (start_const, bound_const, step_const) {
        let role = if positive_guard {
            CountedLoopComparisonRole::IncreasingExclusive
        } else {
            CountedLoopComparisonRole::DecreasingExclusive
        };
        return ordered_comparison_trip_count(role, s0, stop, k)
            .map(TripCount::Constant)
            .unwrap_or(TripCount::Unknown);
    }

    // Symbolic: positive unit-step loop `for i in range(stop)` from 0 with
    // step +1 → trip count == stop (a loop-invariant expression). Only emit a
    // symbolic trip when start==0 and step==1 (the dominant `range(stop)`
    // shape), where trip == stop exactly.
    if positive_guard && step_const == 1 && start_const == Some(0) {
        let bound_scev = builder.scev(bound_val);
        if !matches!(bound_scev, ScevExpr::Unknown) {
            return TripCount::Symbolic(Box::new(bound_scev));
        }
    }

    TripCount::Unknown
}
