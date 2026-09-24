use std::collections::{HashMap, HashSet};

use crate::tir::analysis::LoopForestResult;
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{IntRange, TripCount, affine_iv_hull, affine_recurrence_range};
use crate::tir::op_kinds_generated::{
    ValueRangeCondNarrowRule, opcode_value_range_cond_narrow_rule_table,
};
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::super::counted_loop::LoopGuardContext;
use super::super::counted_loop::recognize_counted_loop_with_loop_forest;
use super::ValueRangeResult;

/// One placement authority for both recurrence producers. The header executes
/// once more than the successful body: its final failed-guard value belongs to
/// the global hull. Only success-edge-dominated blocks receive the body hull.
pub(super) fn seed_recurrence_ranges(
    result: &mut ValueRangeResult,
    iv: ValueId,
    start: i64,
    step: i64,
    trip: &TripCount,
    success_blocks: &[BlockId],
) {
    let (global, body) = match trip {
        TripCount::Constant(trips) if *trips >= 0 => {
            let final_value = start as i128 + *trips as i128 * step as i128;
            let global = i64::try_from(final_value)
                .ok()
                .map(|last| IntRange::new(start.min(last), start.max(last)));
            (global, affine_iv_hull(start, step, *trips))
        }
        TripCount::Constant(_) => (None, None),
        _ => (affine_recurrence_range(start, step, trip), None),
    };
    if let Some(global) = global {
        result.record_global_range(iv, global);
    }
    if let Some(body) = body {
        for &bid in success_blocks {
            result.meet_block_range(bid, iv, body);
        }
    }
}
/// Seed IV ranges from the canonical counted-loop recognizer for any header that
/// SCEV could not classify as an `AddRec` (the frontend's nsw-less counted-loop
/// shape). [`counted_loop::recognize_counted_loop`] proves constant `start`,
/// `step` and `trip_count` directly from the constant loop guard, so the IV's
/// range is the exact closed-form hull (see [`affine_iv_hull`]) —
/// independent of the missing nsw tag and of wrap concerns (a bounded constant
/// trip count gives an exact closed-form last value).
///
/// We only assign a fact to an IV that has none. Both recurrence producers use
/// the shared global/body placement rule; ordinary site-aware op propagation
/// derives update ranges from the successful-body operand facts.
pub(super) fn seed_counted_loop_iv_ranges(
    func: &TirFunction,
    loop_forest: &LoopForestResult,
    result: &mut ValueRangeResult,
) {
    for &header in &loop_forest.headers {
        let Some(c) = recognize_counted_loop_with_loop_forest(func, header, loop_forest) else {
            continue;
        };
        let iv_canon = result.resolve(c.induction_var);
        // If SCEV already ranged this header's IV, the SCEV/guard facts are
        // authoritative — do not disturb them.
        if result.has_global_range(iv_canon) {
            continue;
        }
        seed_recurrence_ranges(
            result,
            iv_canon,
            c.start,
            c.step,
            &TripCount::Constant(c.trip_count),
            &c.body_path,
        );
        // Updates are ordinary op definitions. Site-aware propagation derives
        // them from the body hull, rather than a second recurrence formula.
    }
}

/// Narrow the range an induction variable `{s0, +, k}` takes over a loop body
/// from the loop's exit-test guard `Lt(i, n)` / `Le(i, n)`, and record symbolic
/// `i < len(c)` facts for the symbolic bound proof.
///
/// The guard's `then` successor must be inside the loop body: only then does
/// the body execute under the guard-true condition. Only blocks dominated by
/// that normal edge receive the fact. Header/guard prefixes and exception
/// bypasses retain their global range.
pub(super) fn narrow_from_header_guards(
    func: &TirFunction,
    loop_bodies: &HashMap<BlockId, HashSet<BlockId>>,
    guards: &LoopGuardContext,
    result: &mut ValueRangeResult,
) {
    // Op definitions for tracing the comparison condition.
    let mut def_op: HashMap<ValueId, (OpCode, Vec<ValueId>)> = HashMap::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            for &r in &op.results {
                def_op.insert(r, (op.opcode, op.operands.clone()));
            }
        }
    }

    let mut headers: Vec<_> = loop_bodies.keys().copied().collect();
    headers.sort_unstable_by_key(|bid| bid.0);
    for header in headers {
        let body = &loop_bodies[&header];
        // Find the loop's exit-test CondBranch (usually one block below the
        // header after lowering).
        let Some(guard) = guards.material_guard(func, header, body) else {
            continue;
        };
        // The guard-true successor must be inside the loop body for the narrow
        // to be sound. The shared guard authority guarantees a body/non-body split; verify
        // which side is the body and require the THEN edge to be the body one.
        // We only model the standard `cond == true → stay in loop` polarity:
        // the then-edge re-enters the body, the else-edge exits. (If the
        // polarity is inverted, the guard fact under `cond==true` does not hold
        // in the body, so we conservatively skip — never narrow unsoundly.)
        // (`!then_in || else_in` ≡ `!(then_in && !else_in)`: skip unless the
        // then-edge re-enters the body and the else-edge does not.)
        if !guard.continue_on_true {
            continue;
        }
        let Some((opcode, raw_operands)) = def_op.get(&guard.condition) else {
            continue;
        };
        if raw_operands.len() != 2 {
            continue;
        }
        // Resolve operands through copies so `Lt(Copy(i), Copy(n))` names the
        // canonical i / n. Facts are recorded on canonical values; queries
        // resolve identically, so they line up.
        let var = result.resolve(raw_operands[0]);
        let bound = result.resolve(raw_operands[1]);
        // Numeric narrowing if `bound` is a known constant `n`:
        //   Lt(var, n) ⇒ var <= n - 1
        //   Le(var, n) ⇒ var <= n
        let bound_const = result.const_int_of(bound);
        let narrow_rule = opcode_value_range_cond_narrow_rule_table(*opcode);
        // A statically failed first guard has no executed body. Do not invent
        // contradictory numeric/symbolic body facts for that zero-trip path.
        if let Some(n) = bound_const {
            let possible = match narrow_rule {
                ValueRangeCondNarrowRule::LtUpperExclusive => result.range_of(var).lo < n,
                ValueRangeCondNarrowRule::LeUpperInclusive => result.range_of(var).lo <= n,
                ValueRangeCondNarrowRule::None => true,
            };
            if !possible {
                continue;
            }
        }
        for &b in &guard.success_blocks {
            match narrow_rule {
                ValueRangeCondNarrowRule::LtUpperExclusive => {
                    if let Some(n) = bound_const {
                        let narrow = IntRange::new(i64::MIN, n.saturating_sub(1));
                        narrow_block(result, b, var, narrow);
                    }
                    // Symbolic `var < bound` regardless of constancy.
                    result.record_symbolic_lt(b, var, bound);
                }
                ValueRangeCondNarrowRule::LeUpperInclusive => {
                    if let Some(n) = bound_const {
                        let narrow = IntRange::new(i64::MIN, n);
                        narrow_block(result, b, var, narrow);
                    }
                    // Le(var, n) ⇒ var < n+1; the symbolic-len path is Lt-only
                    // (the numeric path covers the constant n+1 length case).
                }
                ValueRangeCondNarrowRule::None => {}
            }
        }
    }
}

/// Meet `range` into the existing per-block fact for `(bid, var)`.
fn narrow_block(result: &mut ValueRangeResult, bid: BlockId, var: ValueId, range: IntRange) {
    result.meet_block_range(bid, var, range);
}
