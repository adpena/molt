use std::collections::{HashMap, HashSet};

use crate::tir::analysis::LoopForestResult;
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{IntRange, affine_iv_hull};
use crate::tir::op_kinds_generated::{
    ValueRangeCondNarrowRule, opcode_value_range_cond_narrow_rule_table,
};
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

use super::super::counted_loop::recognize_counted_loop_with_loop_forest;
use super::super::scev::find_loop_guard;
use super::ValueRangeResult;
/// Seed IV ranges from the canonical counted-loop recognizer for any header that
/// SCEV could not classify as an `AddRec` (the frontend's nsw-less counted-loop
/// shape). [`counted_loop::recognize_counted_loop`] proves constant `start`,
/// `step` and `trip_count` directly from the constant loop guard, so the IV's
/// range is the exact closed-form hull (see [`affine_iv_hull`]) —
/// independent of the missing nsw tag and of wrap concerns (a bounded constant
/// trip count gives an exact closed-form last value).
///
/// We only ASSIGN a fact to an IV that has none (never widen a tighter SCEV/guard
/// fact), and we range the back-edge update value the same way SCEV's path does,
/// so a value-keyed consumer (`fits_inline_int47`) sees the phi's loop-carried
/// incoming proven too.
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
        // The IV's exact i128-computed hull over the proven constant trip count.
        let Some(iv_range) = affine_iv_hull(c.start, c.step, c.trip_count) else {
            continue;
        };
        // Place the IV range as a weak global + a per-body-block fact.
        result.record_global_range(iv_canon, iv_range);
        if let Some(body) = loop_forest.bodies.get(&header) {
            for &b in body {
                result.meet_block_range(b, iv_canon, iv_range);
            }
        }
        // Range the back-edge update value `iv_next = iv + step` (one step later)
        // so the IV phi's loop-carried incoming is also proven for value-keyed
        // consumers (`fits_inline_int47`). `back_args[iv_arg_index]` is the
        // IV-next value the recognizer validated as `Add(iv, step)`. Its hull is
        // the recurrence shifted by one step: `{start + step, +, step}`.
        if let Some(s0_next) = c.start.checked_add(c.step)
            && let Some(next_range) = affine_iv_hull(s0_next, c.step, c.trip_count)
        {
            let next_canon = result.resolve(c.back_args[c.iv_arg_index]);
            result.meet_global_range(next_canon, next_range);
        }
    }
}

/// The value carried on the loop's back-edge into the header phi `iv` — i.e.
/// the IV's next-iteration value. `iv` is a header block-argument; this returns
/// the argument passed at `iv`'s index by the (single) body block whose
/// terminator branches back to `header`. Returns `None` when the structure is
/// not the canonical single-latch shape (multiple back-edges with differing
/// values, or a missing arg), in which case the next-value range is left
/// unproven (sound: a conservative omission, never a false fact).
pub(super) fn back_edge_update_value(
    func: &TirFunction,
    header: BlockId,
    iv: ValueId,
    body: &HashSet<BlockId>,
) -> Option<ValueId> {
    // The IV's positional index among the header block arguments.
    let header_block = func.blocks.get(&header)?;
    let arg_index = header_block.args.iter().position(|a| a.id == iv)?;

    let mut found: Option<ValueId> = None;
    for &bid in body {
        let Some(block) = func.blocks.get(&bid) else {
            continue;
        };
        // Collect every (target, args) edge from this body block.
        let edges: &[(BlockId, &Vec<ValueId>)] = &match &block.terminator {
            Terminator::Branch { target, args } => vec![(*target, args)],
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => vec![(*then_block, then_args), (*else_block, else_args)],
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            }
            | Terminator::StateDispatch {
                cases,
                default,
                default_args,
                ..
            } => {
                let mut v: Vec<(BlockId, &Vec<ValueId>)> =
                    cases.iter().map(|(_, t, a)| (*t, a)).collect();
                v.push((*default, default_args));
                v
            }
            Terminator::Return { .. } | Terminator::Unreachable => continue,
        };
        for (target, args) in edges {
            if *target != header {
                continue;
            }
            let Some(&val) = args.get(arg_index) else {
                // A back-edge that does not pass this arg → malformed; refuse.
                return None;
            };
            match found {
                None => found = Some(val),
                // Multiple back-edges carrying *different* values → ambiguous;
                // do not assign a (possibly wrong) range.
                Some(prev) if prev != val => return None,
                Some(_) => {}
            }
        }
    }
    found
}

/// Narrow the range an induction variable `{s0, +, k}` takes over a loop body
/// from the loop's exit-test guard `Lt(i, n)` / `Le(i, n)`, and record symbolic
/// `i < len(c)` facts for the symbolic bound proof.
///
/// The guard's `then` successor must be inside the loop body: only then does
/// the body execute under the guard-true condition. We narrow `var`'s range in
/// every body block — sound because, in the canonical single-exit-test loop,
/// every body block is reached only through the guard-true edge.
pub(super) fn narrow_from_header_guards(
    func: &TirFunction,
    loop_bodies: &HashMap<BlockId, HashSet<BlockId>>,
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

    for (&header, body) in loop_bodies {
        // Find the loop's exit-test CondBranch (usually one block below the
        // header after lowering).
        let Some((guard_block, cond)) = find_loop_guard(func, header, body) else {
            continue;
        };
        // The guard-true successor must be inside the loop body for the narrow
        // to be sound. find_loop_guard guarantees a body/non-body split; verify
        // which side is the body and require the THEN edge to be the body one.
        let Some(guard_blk) = func.blocks.get(&guard_block) else {
            continue;
        };
        let Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } = &guard_blk.terminator
        else {
            continue;
        };
        let then_in = body.contains(then_block);
        let else_in = body.contains(else_block);
        // We only model the standard `cond == true → stay in loop` polarity:
        // the then-edge re-enters the body, the else-edge exits. (If the
        // polarity is inverted, the guard fact under `cond==true` does not hold
        // in the body, so we conservatively skip — never narrow unsoundly.)
        // (`!then_in || else_in` ≡ `!(then_in && !else_in)`: skip unless the
        // then-edge re-enters the body and the else-edge does not.)
        if !then_in || else_in {
            continue;
        }
        let Some((opcode, raw_operands)) = def_op.get(&cond) else {
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
        for &b in body {
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
