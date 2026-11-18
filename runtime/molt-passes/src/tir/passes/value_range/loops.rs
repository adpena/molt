use molt_ir::deterministic_hash::{HashMap, HashSet};

use crate::tir::analysis::LoopForestResult;
use crate::tir::blocks::BlockId;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{IntRange, TripCount, affine_iv_hull, affine_recurrence_range};
use crate::tir::op_kinds_generated::{
    ValueRangeCondNarrowRule, opcode_value_range_cond_narrow_rule_table,
};
use crate::tir::ops::{OpCode, TirOp};
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
    independent: &ValueRangeResult,
    result: &mut ValueRangeResult,
) {
    // Op definitions for tracing the comparison condition.
    let defs = op_definitions(func);

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
        let Some(comparison) = guard_comparison(&defs, result, guard.condition) else {
            continue;
        };
        let (opcode, raw_operands) = (&comparison.opcode, &comparison.operands);
        if raw_operands.len() != 2 {
            continue;
        }
        // Resolve operands through copies so `Lt(Copy(i), Copy(n))` names the
        // canonical i / n. Facts are recorded on canonical values; queries
        // resolve identically, so they line up.
        let var = result.resolve(raw_operands[0]);
        let bound = result.resolve(raw_operands[1]);
        // Numeric narrowing from the bound's largest value `n`: a constant,
        // or the top of a bound proven without any recurrence or guard fact
        // (an unboxed inline-int bound lies in the inline window):
        //   Lt(var, bound) ⇒ var <= n - 1
        //   Le(var, bound) ⇒ var <= n
        let bound_const = result
            .const_int_of(bound)
            .or_else(|| interior_bound(independent.range_of(bound)).map(|range| range.hi));
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

/// The op that defines each value.
fn op_definitions(func: &TirFunction) -> HashMap<ValueId, &TirOp> {
    let mut defs = HashMap::default();
    for block in func.blocks.values() {
        for op in &block.ops {
            for &value in &op.results {
                defs.insert(value, op);
            }
        }
    }
    defs
}

/// The comparison a loop guard tests: the op defining its condition, or the
/// comparison under a truthiness op (`while i < n` where `n` is not known to
/// be an int boxes the comparison's result, then tests its truth). Callers
/// narrow only ints, and a comparison of two ints yields a bool, whose truth
/// is itself.
fn guard_comparison<'a>(
    defs: &HashMap<ValueId, &'a TirOp>,
    result: &ValueRangeResult,
    condition: ValueId,
) -> Option<&'a TirOp> {
    let op = *defs.get(&result.resolve(condition))?;
    if op.opcode == OpCode::Bool
        && let [compared] = op.operands[..]
    {
        return defs.get(&result.resolve(compared)).copied();
    }
    Some(op)
}

/// `range` when both bounds are constants drawn from the program, never an i64
/// extreme a saturated transfer reached: only such a range bounds a value
/// independently of the loop phis (see `is_phi_independent_bound`).
pub(super) fn interior_bound(range: IntRange) -> Option<IntRange> {
    (range.lo > i64::MIN && range.hi < i64::MAX).then_some(range)
}

/// One header phi that changes by a bounded step on its only back-edge value:
/// `p = phi(init, p + d)` with `init` in `start` and `d` in `step`.
struct BoundedRecurrence {
    phi: ValueId,
    start: IntRange,
    step: IntRange,
}

/// Range the header phis of each loop that change by a bounded step per
/// iteration: an induction variable (`i += 1`) and an accumulator whose
/// increment has a bound of its own (`total += i & 7`).
///
/// A phi `p = phi(init, p + d)`, with `init` in `[s, S]` and `d` in `[a, b]`,
/// holds `init + d_1 + ... + d_j` after `j` back edges: Python ints do not
/// wrap. When the loop's guard bounds the back edges to `T`, `p` lies in
/// `[s + T*min(a, 0), S + T*max(b, 0)]`. The guard `i < n` bounds them when
/// `i` is such a phi with `a >= 1`, the guard tests the phi itself, every back
/// edge leaves a block the guard's success edge dominates, and `n` has a
/// largest value `N` (a constant, or a range like the inline window of an
/// unboxed bound): the `j`-th back edge needs `i_j <= N - 1` and
/// `i_j >= s + j*a`. A loop without such a guard gets no fact here.
///
/// Soundness rests on the inputs, each read from the frozen `independent`
/// snapshot, where every phi is unknown: `init`, `d` and `n` have interior
/// ranges there, so none of them assumed a bound on a phi. Only an interior
/// result is recorded: a saturated bound would claim an i64 limit on a value
/// that can grow past it. A phi with another fact (an `AddRec` induction
/// variable) keeps it.
pub(super) fn seed_bounded_recurrence_ranges(
    func: &TirFunction,
    loop_bodies: &HashMap<BlockId, HashSet<BlockId>>,
    guards: &LoopGuardContext,
    independent: &ValueRangeResult,
    result: &mut ValueRangeResult,
) {
    use crate::tir::op_kinds_generated::{
        ValueRangeTransferRule, opcode_value_range_transfer_rule_table,
    };
    if loop_bodies.is_empty() {
        return;
    }
    let reachable = crate::tir::dominators::executable_reachable_blocks(func);
    let labels = crate::tir::dominators::exception_label_to_block(func);
    // (header, arg index) -> (source block, value) on every reachable edge.
    let mut incomings: HashMap<(BlockId, usize), Vec<(BlockId, ValueId)>> = HashMap::default();
    let mut invalid: HashSet<BlockId> = HashSet::default();
    for block in func.blocks.values() {
        if !reachable.contains(&block.id) {
            continue;
        }
        block.terminator.for_each_edge(|target, args| {
            if !loop_bodies.contains_key(&target) {
                return;
            }
            if func
                .blocks
                .get(&target)
                .is_none_or(|header| header.args.len() != args.len())
            {
                invalid.insert(target);
            }
            for (index, &arg) in args.iter().enumerate() {
                incomings
                    .entry((target, index))
                    .or_default()
                    .push((block.id, arg));
            }
        });
        // An exception edge carries no phi payload.
        invalid.extend(crate::tir::dominators::exception_successors(block, &labels));
    }
    let defs = op_definitions(func);

    let mut headers: Vec<BlockId> = loop_bodies.keys().copied().collect();
    headers.sort_unstable_by_key(|bid| bid.0);
    let mut seeds: Vec<(ValueId, IntRange)> = Vec::new();
    for header in headers {
        if invalid.contains(&header) {
            continue;
        }
        let body = &loop_bodies[&header];
        let Some(header_block) = func.blocks.get(&header) else {
            continue;
        };
        let mut recurrences: Vec<BoundedRecurrence> = Vec::new();
        for (index, arg) in header_block.args.iter().enumerate() {
            let phi = result.resolve(arg.id);
            let Some(edges) = incomings.get(&(header, index)) else {
                continue;
            };
            let mut start: Option<IntRange> = None;
            let mut back: Option<ValueId> = None;
            let mut shape = true;
            for &(source, value) in edges {
                if body.contains(&source) {
                    let value = result.resolve(value);
                    shape &= back.is_none_or(|known| known == value);
                    back = Some(value);
                } else if let Some(range) = interior_bound(independent.range_of(value)) {
                    start = Some(start.map_or(range, |known| known.join(range)));
                } else {
                    shape = false;
                }
            }
            let (true, Some(start), Some(back)) = (shape, start, back) else {
                continue;
            };
            let Some(op) = defs.get(&back) else {
                continue;
            };
            let [lhs, rhs] = op.operands[..] else {
                continue;
            };
            let (lhs, rhs) = (result.resolve(lhs), result.resolve(rhs));
            let step = match opcode_value_range_transfer_rule_table(op.opcode) {
                ValueRangeTransferRule::Add if lhs == phi => independent.range_of(rhs),
                ValueRangeTransferRule::Add if rhs == phi => independent.range_of(lhs),
                ValueRangeTransferRule::Sub if lhs == phi => independent.range_of(rhs).neg(),
                _ => continue,
            };
            let Some(step) = interior_bound(step) else {
                continue;
            };
            recurrences.push(BoundedRecurrence { phi, start, step });
        }
        if recurrences.is_empty() {
            continue;
        }
        let Some(trips) = back_edge_bound(
            func,
            header,
            body,
            guards,
            independent,
            result,
            &defs,
            &recurrences,
        ) else {
            continue;
        };
        for recurrence in &recurrences {
            if result.has_global_range(recurrence.phi) {
                continue;
            }
            let (start, step) = (recurrence.start, recurrence.step);
            let range = IntRange::from_i128(
                start.lo as i128 + trips * (step.lo.min(0) as i128),
                start.hi as i128 + trips * (step.hi.max(0) as i128),
            );
            // A saturated bound means the phi may leave i64 (a big int).
            if let Some(range) = interior_bound(range) {
                seeds.push((recurrence.phi, range));
            }
        }
    }
    for (phi, range) in seeds {
        result.meet_global_range(phi, range);
    }
}

/// The most back edges the loop at `header` can take: its guard `i < n` (or
/// `i <= n`) tests a recurrence `i` that grows by at least one per iteration
/// toward a bound `n` whose largest value is known.
#[allow(clippy::too_many_arguments)]
fn back_edge_bound(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
    guards: &LoopGuardContext,
    independent: &ValueRangeResult,
    result: &ValueRangeResult,
    defs: &HashMap<ValueId, &TirOp>,
    recurrences: &[BoundedRecurrence],
) -> Option<i128> {
    let guard = guards.material_guard(func, header, body)?;
    if !guard.continue_on_true || !guards.recurrence_is_guarded(func, header, body, &guard) {
        return None;
    }
    let condition = guard_comparison(defs, result, guard.condition)?;
    let [var, bound] = condition.operands[..] else {
        return None;
    };
    let var = result.resolve(var);
    let induction = recurrences
        .iter()
        .find(|recurrence| recurrence.phi == var && recurrence.step.lo >= 1)?;
    let bound = result.resolve(bound);
    let top = result
        .const_int_of(bound)
        .or_else(|| interior_bound(independent.range_of(bound)).map(|range| range.hi))?;
    // The largest value `i` may hold where the guard lets the body run.
    let last = match opcode_value_range_cond_narrow_rule_table(condition.opcode) {
        ValueRangeCondNarrowRule::LtUpperExclusive => top as i128 - 1,
        ValueRangeCondNarrowRule::LeUpperInclusive => top as i128,
        ValueRangeCondNarrowRule::None => return None,
    };
    let first = induction.start.lo as i128;
    if last < first {
        return Some(0);
    }
    Some((last - first) / induction.step.lo as i128 + 1)
}
