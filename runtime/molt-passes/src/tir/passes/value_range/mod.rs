//! Integer value-range / interval analysis (a Lazy-Value-Info analog) for TIR —
//! Tier-0 substrate **S6**.
//!
//! For each integer SSA value the analysis computes a conservative interval
//! [`IntRange`] `[lo, hi]` (saturating to the i64 domain). Ranges come from
//! three sources, joined as a lattice:
//!
//!   1. **Constants** — `ConstInt v` ⇒ `[v, v]`.
//!   2. **Scalar evolution** — a canonical induction variable `i` of
//!      `for i in range(stop)` (SCEV `AddRec {start: s0, step: +k}` with a
//!      proven trip count) ranges over `[s0, last]` where `last` is the IV's
//!      value on the final executed iteration. This is the *loop-invariant*
//!      range that holds *everywhere in the loop body*.
//!   3. **Edge-sensitive guard narrowing** — inside the true successor of a
//!      header `CondBranch(Lt(i, n))`, `i < n`; of `Le(i, n)`, `i <= n`. These
//!      narrow the body range further (and are what proves the `while`-loop
//!      bounds cases).
//!
//! The analysis also records **container lengths** (`BuildList`, list-repeat
//! `Mul`, and `len(c)` symbols) so [`ValueRangeResult::proves_index_in_bounds`]
//! can discharge `0 <= index < len(container)`.
//!
//! ## Soundness (a false positive is a silent OOB write)
//!
//!   * Every range op is computed in `i128` and **saturates** to
//!     [`IntRange::FULL_I64`] on overflow — never wraps.
//!   * [`proves_index_in_bounds`] is a CONSERVATIVE over-approximation: it
//!     returns `true` only when it can prove `lo >= 0` AND `hi < len` for a
//!     known length. Any uncertainty (unknown range, unknown length, partial
//!     proof) returns `false`, leaving the runtime bounds check in place.
//!   * [`fits_inline_int47`] returns `true` only when the *entire* proven range
//!     lies within the signed 47-bit inline window `[-2^46, 2^46 - 1]`.

mod lengths;
mod loops;
mod propagation;
mod report;
mod result;
mod transfer;

#[cfg(test)]
mod tests;

use crate::tir::analysis::{Analysis, AnalysisId, LoopForest, LoopForestResult};
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{IntRange, ScevExpr, affine_recurrence_range};

use super::scev::{ScevResult, compute_scev_with_loop_forest};
use super::value_identity::copy_value_source;

use lengths::collect_constants_and_lengths;
use loops::{back_edge_update_value, narrow_from_header_guards, seed_counted_loop_iv_ranges};
use propagation::{narrow_loop_header_phis, propagate_op_ranges};
use report::emit_vrange_report;
pub use result::ValueRangeResult;

// Analysis registration
// ---------------------------------------------------------------------------

/// Value-range analysis marker. Cached by the [`AnalysisManager`].
pub struct ValueRange;

impl Analysis for ValueRange {
    type Result = ValueRangeResult;
    const ID: AnalysisId = AnalysisId::ValueRange;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        let loop_forest = <LoopForest as Analysis>::compute(func);
        let scev = compute_scev_with_loop_forest(func, &loop_forest);
        compute_value_range_with_loop_forest(func, &scev, &loop_forest)
    }
}

// ---------------------------------------------------------------------------
// Computation
// ---------------------------------------------------------------------------

/// Compute value-range facts from the function + its scalar-evolution facts.
pub fn compute_value_range(func: &TirFunction, scev: &ScevResult) -> ValueRangeResult {
    let loop_forest = <LoopForest as Analysis>::compute(func);
    compute_value_range_with_loop_forest(func, scev, &loop_forest)
}

/// Compute value-range facts using the caller-provided canonical LoopForest.
pub(crate) fn compute_value_range_with_loop_forest(
    func: &TirFunction,
    scev: &ScevResult,
    loop_forest: &LoopForestResult,
) -> ValueRangeResult {
    let mut result = ValueRangeResult::default();

    // ---- transparent-copy map (built first; every fact resolves through it) --
    // Resolve both *plain* SSA copies AND the frontend's value-identity `Copy`
    // carriers (its stack-machine `copy` / `copy_var` / `store_var` / `load_var`
    // moves, which carry `_original_kind`/`_simple_out`/`_col_offset` attrs but
    // are still pure value moves). The IV reaches a hot-loop field store through
    // exactly these tagged copies (`store_val = Copy(Copy(Copy(iv)))`); resolving
    // them is what lets a fact recorded on the canonical IV be found when a
    // consumer queries the stored value. This mirrors the alias oracle's
    // `copy_is_known_local_alias` value-forwarding kinds — the single source of
    // truth for "this Copy holds the same value as its operand".
    for block in func.blocks.values() {
        for op in &block.ops {
            if let Some(src) = copy_value_source(op) {
                result.copy_src.insert(op.results[0], src);
            }
        }
    }

    // ---- constants + container lengths --------------------------------------
    collect_constants_and_lengths(func, &mut result);

    // ---- loop bodies (for IV-range placement) -------------------------------
    let loop_bodies = &loop_forest.bodies;

    // ---- global ranges from constants ---------------------------------------
    for (&v, &c) in &result.const_int {
        result.global_range.insert(v, IntRange::point(c));
    }

    // ---- IV ranges from SCEV ------------------------------------------------
    // For each loop header with a canonical IV (AddRec) and a known trip count,
    // the IV ranges over [start, last] for the whole loop body.
    for &header in scev.headers() {
        let Some(body) = loop_bodies.get(&header) else {
            continue;
        };
        // Find the header's IV: the header block-arg whose SCEV is an AddRec
        // over this header.
        let Some(header_block) = func.blocks.get(&header) else {
            continue;
        };
        for arg in &header_block.args {
            let iv = arg.id;
            let ScevExpr::AddRec {
                start,
                step,
                loop_header,
            } = scev.scev_of(iv)
            else {
                continue;
            };
            if loop_header != header {
                continue;
            }
            let (Some(s0), Some(k)) = (start.as_constant(), step.as_constant()) else {
                continue;
            };
            let trip = scev.trip_count(header);
            // Compute the IV's range over the body from start, step, trip count.
            let iv_range = match affine_recurrence_range(s0, k, &trip) {
                Some(r) => r,
                None => continue,
            };
            // The IV range holds everywhere in the loop body. Place it as a
            // per-block fact for each body block (and as a weak global so a
            // query outside any guarded block still sees it).
            result.global_range.insert(iv, iv_range);
            for &b in body {
                // meet with any existing (e.g. a tighter guard placed later).
                let existing = result
                    .block_range
                    .get(&(b, iv))
                    .copied()
                    .unwrap_or(IntRange::FULL_I64);
                result.block_range.insert((b, iv), existing.meet(iv_range));
            }
            // Also range the **back-edge update value** `next = iv + k` (the
            // value carried across the latch into the IV phi). It takes the IV's
            // values one step later — `{s0 + k, +, k}` — so its range is the same
            // recurrence shifted by one step. Ranging it is what lets a consumer
            // prove the *phi's incoming* fits the inline window (e.g. the
            // representation plan's `RawI64Safe` carrier requires every phi
            // incoming proven, not just the phi). Without this the loop-carried
            // update would be unproven and force the IV phi back to the boxed
            // carrier — a perf cliff on the canonical `for i in range(n)` loop.
            // All arithmetic saturates in i128; an `s0 + k` that would overflow
            // simply yields no fact (sound: the value stays unproven).
            if let Some(next_val) = back_edge_update_value(func, header, iv, body)
                && let Some(s0_next) = s0.checked_add(k)
                && let Some(next_range) = affine_recurrence_range(s0_next, k, &trip)
            {
                // `next_val = iv + k` takes exactly the recurrence's values one
                // step later, so `next_range` is its precise range. Store it on
                // the **canonical** (copy-resolved) value, matching how queries
                // (`fits_inline_int47`, `range_of`) resolve through plain copies,
                // and meet with any existing fact (never widen). This lets a
                // value-keyed consumer prove the IV phi's loop-carried incoming
                // fits the inline window.
                let next_canon = result.resolve(next_val);
                let existing = result
                    .global_range
                    .get(&next_canon)
                    .copied()
                    .unwrap_or(IntRange::FULL_I64);
                result
                    .global_range
                    .insert(next_canon, existing.meet(next_range));
            }
        }
    }

    // ---- IV ranges from the counted-loop recognizer -------------------------
    // SCEV only forms an `AddRec` when the IV increment carries `no_signed_wrap`.
    // The frontend lowers `for i in range(C):` / `for i in range(start, stop):`
    // directly to a counted *arithmetic* loop (no `CallBuiltin("range")` iterator
    // for `range_devirt` to match, and its `Add(iv, step)` is NOT nsw-tagged), so
    // SCEV gives that IV no recurrence and the loop above places no fact. The
    // canonical counted-loop recognizer ([`counted_loop::recognize_counted_loop`])
    // proves `start` / `step` / `trip_count` as *constants* directly from the
    // constant loop guard `Lt(iv, stop_const)` — independent of the nsw tag and
    // of wrap concerns (a bounded constant trip count gives an exact closed-form
    // last value). We seed the IV's range from that descriptor for any header SCEV
    // left un-ranged. This is the producer that unblocks SROA's hot-loop field
    // promotion on the dominant `for i in range(C): obj.field = <i-derived>` shape.
    seed_counted_loop_iv_ranges(func, loop_forest, &mut result);

    // ---- forward transfer-function propagation ------------------------------
    // Compute ranges for op-defined values (`i + 1`, `i & 15`, `i % 4`, `i >> 2`,
    // …) from their operands' already-proven ranges, to a fixpoint. This is the
    // producer that lets a value DERIVED from an induction variable — not just
    // the IV itself — be proven inline (the SROA hot-loop field-promotion gap).
    //
    // CRUCIAL INVARIANT this first sweep establishes (relied on by the phi-range
    // narrowing below): it NEVER assigns a range to a phi / block argument, so
    // every op-result range it computes is derived under the assumption that all
    // phis are FULL (unknown). A *bounded interior* range it produces for any
    // value (see `is_phi_independent_bound`) is therefore phi-independent by
    // construction — it did not assume any range for any phi.
    propagate_op_ranges(func, &mut result);

    // ---- loop-header phi-range narrowing ------------------------------------
    // Narrow a loop-header phi to the JOIN of its incoming-edge ranges when every
    // incoming range is phi-INDEPENDENT (a bounded interior range proven by the
    // FULL-phi sweep above). The licensing structure is a re-bounding op on the
    // back edge — a
    // `x & const_mask` makes the carried value's range `[0, mask]` REGARDLESS of
    // the phi, so a masked-shift accumulator (`s = (s << 1) & MASK`) recovers its
    // raw-i64 lane. An unbounded accumulator (`total = total + i`, `acc = acc <<
    // 1`) has a FULL back-edge range under the FULL-phi sweep, so it is never
    // narrowed (the mandatory bigint soundness gate). After narrowing, re-run the
    // forward sweep so values DERIVED from the now-narrowed phi (`s << 1`) are
    // ranged too — the producer that actually feeds the raw-i64 seed.
    if narrow_loop_header_phis(func, loop_bodies, &mut result) {
        propagate_op_ranges(func, &mut result);
    }

    // ---- edge-sensitive guard narrowing -------------------------------------
    // For a header `CondBranch(cond -> then=body, else=exit)` where
    // `cond = Lt(i, n)` / `Le(i, n)`, the body sees `i < n` / `i <= n`.
    narrow_from_header_guards(func, loop_bodies, &mut result);

    // Producer-evidence instrument (`MOLT_VRANGE_REPORT=1`): per-function dump of
    // the proven loop-header IV recurrence + every global integer range, to the
    // debug-artifact channel. The sibling of `MOLT_SROA_REPORT`/`MOLT_MEMGVN_REPORT`
    // — used to verify the IV-range seed and transfer-function precision fire on
    // real code (a `fits_inline_int47=false` here explains a refused SROA there).
    if std::env::var("MOLT_VRANGE_REPORT").as_deref() == Ok("1") {
        emit_vrange_report(func, scev, loop_bodies, &result);
    }

    result
}
