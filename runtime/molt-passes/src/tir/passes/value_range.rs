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

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{Analysis, AnalysisId, LoopForest, LoopForestResult};
use crate::tir::blocks::{BlockId, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{
    INLINE_INT47_HI, IntRange, ScevExpr, affine_iv_hull, affine_recurrence_range,
};
use crate::tir::op_kinds_generated::{
    ValueRangeCondNarrowRule, ValueRangeConstFoldRule, ValueRangeContainerLengthRule,
    ValueRangeTransferRule, opcode_value_range_cond_narrow_rule_table,
    opcode_value_range_const_fold_rule_table, opcode_value_range_container_length_rule_table,
    opcode_value_range_transfer_rule_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

use super::scev::{ScevResult, compute_scev_with_loop_forest, find_loop_guard};
use super::value_identity::copy_value_source;

// ---------------------------------------------------------------------------
// Container length
// ---------------------------------------------------------------------------

/// A known container length: a compile-time constant or "same SSA value as".
#[derive(Debug, Clone, PartialEq, Eq)]
enum KnownLength {
    Constant(i64),
    SameAs(ValueId),
}

// ---------------------------------------------------------------------------
// Result
// ---------------------------------------------------------------------------

/// Per-function integer value-range facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ValueRangeResult {
    /// Loop-invariant range that holds for a value *everywhere in the function*
    /// (constants) or *everywhere in its loop body* (induction variables).
    global_range: HashMap<ValueId, IntRange>,
    /// Per-(block, value) narrowed range from edge-sensitive guards. A query
    /// at block `b` for value `v` first consults this, then `global_range`.
    block_range: HashMap<(BlockId, ValueId), IntRange>,
    /// container value → known length.
    container_length: HashMap<ValueId, KnownLength>,
    /// `len(c)` result value → the container `c` (for `i < len(c)` proofs).
    len_of: HashMap<ValueId, ValueId>,
    /// constant-int values (for length/bound comparison).
    const_int: HashMap<ValueId, i64>,
    /// Edge-sensitive symbolic upper bound: at block `bid`, value `var` is
    /// provably `< bound` (an SSA value). Recorded from header guards
    /// `Lt(var, bound)` and used for the `index < len(container)` symbolic
    /// proof when the numeric length is not a constant.
    symbolic_lt_bound: HashMap<(BlockId, ValueId), ValueId>,
    /// Transparent-copy resolution: value → canonical source through plain SSA
    /// copies (`is_plain_value_copy`). Lowering threads the IV / length / index
    /// through copies; query methods resolve to the canonical value so a fact
    /// recorded on the source is found when querying any copy of it (and vice
    /// versa). A plain copy is the identity, so this is exact, not lossy.
    copy_src: HashMap<ValueId, ValueId>,
}

impl ValueRangeResult {
    /// Follow plain-copy edges to the canonical source of `v` (bounded walk).
    fn resolve(&self, mut v: ValueId) -> ValueId {
        for _ in 0..64 {
            match self.copy_src.get(&v) {
                Some(&src) if src != v => v = src,
                _ => break,
            }
        }
        v
    }

    /// The proven range of `v` at block `bid`: the guard-narrowed range if one
    /// exists, else the global (loop-invariant / constant) range, else
    /// `FULL_I64` (unknown). Resolves `v` through plain copies first.
    pub fn range_at(&self, bid: BlockId, v: ValueId) -> IntRange {
        let v = self.resolve(v);
        if let Some(r) = self.block_range.get(&(bid, v)) {
            return *r;
        }
        self.global_range
            .get(&v)
            .copied()
            .unwrap_or(IntRange::FULL_I64)
    }

    /// The proven loop-invariant / constant range of `v` (ignoring per-block
    /// guard narrowing). `FULL_I64` if unknown.
    pub fn range_of(&self, v: ValueId) -> IntRange {
        let v = self.resolve(v);
        self.global_range
            .get(&v)
            .copied()
            .unwrap_or(IntRange::FULL_I64)
    }

    /// CONSERVATIVELY prove `0 <= index < len(container)` for an `Index` /
    /// `StoreIndex` at block `bid`. Returns `true` only when both bounds are
    /// provable; any uncertainty returns `false` (the bounds check stays).
    ///
    /// This is the BCE memory-safety query. A false positive is a silent
    /// out-of-bounds access, so every path that does not *prove* safety must
    /// fall through to `false`.
    pub fn proves_index_in_bounds(&self, bid: BlockId, container: ValueId, index: ValueId) -> bool {
        let container = self.resolve(container);
        // `range_at` resolves the index itself.
        let idx_range = self.range_at(bid, index);

        // Lower bound: index >= 0. A negative index needs Python wraparound, so
        // it is never bce_safe here.
        if !idx_range.is_non_negative() {
            return false;
        }

        // Upper bound: index < len(container). We need a known upper bound on
        // the index AND a known length.
        let idx_hi = idx_range.hi;
        if idx_hi == i64::MAX {
            // Unbounded above → cannot prove.
            return false;
        }

        match self.container_length.get(&container) {
            Some(KnownLength::Constant(len)) => {
                // index <= idx_hi < len  ⟺  idx_hi < len.
                idx_hi < *len
            }
            Some(KnownLength::SameAs(len_val)) => {
                // The length equals SSA value `len_val`. Prove `idx_hi < len_val`
                // only when `len_val` has a known constant value `> idx_hi`.
                if let Some(len_lo) = self.const_int.get(&self.resolve(*len_val)) {
                    return idx_hi < *len_lo;
                }
                // Otherwise the numeric bound is unprovable here; the symbolic
                // `index < len(container)` path discharges it instead.
                false
            }
            None => false,
        }
    }

    /// True if a guard at `bid` proves `index < len(container)` *symbolically*,
    /// i.e. the index is guarded `Lt(index, b)` where `b == len(container)`
    /// (the post-`iter_devirt` `while i < len(lst)` shape). Combined with the
    /// numeric `index >= 0` proof, this discharges the bound when the numeric
    /// length is not a constant.
    pub fn proves_index_lt_len_symbolically(
        &self,
        bid: BlockId,
        container: ValueId,
        index: ValueId,
    ) -> bool {
        let container = self.resolve(container);
        let index = self.resolve(index);
        // index must be provably >= 0 at bid.
        if !self.range_at(bid, index).is_non_negative() {
            return false;
        }
        // Look for a recorded symbolic bound `index < bound_val` where
        // `bound_val == len(container)`.
        if let Some(&bound_val) = self.symbolic_lt_bound.get(&(bid, index)) {
            let bound_val = self.resolve(bound_val);
            if let Some(&bound_container) = self.len_of.get(&bound_val) {
                return self.resolve(bound_container) == container;
            }
        }
        false
    }

    /// BCE-only index safety query. It is strictly narrower than the raw-int
    /// carrier proof: even when the ordinary numeric or symbolic bound proves
    /// the access safe, the index must also fit the inline-int47 window. A
    /// full-range checked-overflow carrier can therefore never become `bce_safe`
    /// by sharing representation facts.
    pub fn proves_index_in_bounds_conservatively(
        &self,
        bid: BlockId,
        container: ValueId,
        index: ValueId,
    ) -> bool {
        let proven = self.proves_index_in_bounds(bid, container, index)
            || self.proves_index_lt_len_symbolically(bid, container, index);
        proven
            && (self.range_at(bid, index).fits_inline_int47()
                || self.symbolic_index_bound_fits_inline_window(bid, container, index))
    }

    /// CONSERVATIVELY prove `v`'s entire proven range fits the signed 47-bit
    /// inline window. Unknown range ⇒ `false`.
    pub fn fits_inline_int47(&self, v: ValueId) -> bool {
        match self.global_range.get(&self.resolve(v)) {
            Some(r) => r.fits_inline_int47(),
            None => false,
        }
    }

    fn symbolic_index_bound_fits_inline_window(
        &self,
        bid: BlockId,
        container: ValueId,
        index: ValueId,
    ) -> bool {
        let container = self.resolve(container);
        let index = self.resolve(index);
        let Some(&bound_val) = self.symbolic_lt_bound.get(&(bid, index)) else {
            return false;
        };
        let bound_range = self.range_at(bid, bound_val);
        if bound_range.hi <= INLINE_INT47_HI.saturating_add(1) {
            return true;
        }
        let bound_val = self.resolve(bound_val);
        if self
            .len_of
            .get(&bound_val)
            .is_none_or(|bound_container| self.resolve(*bound_container) != container)
        {
            return false;
        }
        match self.container_length.get(&container) {
            Some(KnownLength::Constant(len)) => *len <= INLINE_INT47_HI.saturating_add(1),
            Some(KnownLength::SameAs(len_val)) => {
                self.range_at(bid, *len_val).hi <= INLINE_INT47_HI.saturating_add(1)
            }
            None => false,
        }
    }

    /// Record the edge-sensitive symbolic fact `var < bound` at block `bid`.
    /// `var` and `bound` are stored as their canonical (resolved) sources.
    fn record_symbolic_lt(&mut self, bid: BlockId, var: ValueId, bound: ValueId) {
        let var = self.resolve(var);
        let bound = self.resolve(bound);
        self.symbolic_lt_bound.insert((bid, var), bound);
    }

    /// Test-only: directly seed the global range of `v` to `[lo, hi]`. Used by
    /// sibling-pass unit tests (e.g. LICM's throw-disproof gate) that need to
    /// exercise range-dependent logic against a hand-built result without
    /// standing up a full TIR function + the analysis pipeline. The
    /// `global_range` field is private, so this is the sanctioned cross-module
    /// test seam.
    #[cfg(any(test, feature = "test-util"))]
    pub fn set_global_range_for_test(&mut self, v: ValueId, lo: i64, hi: i64) {
        self.global_range.insert(v, IntRange::new(lo, hi));
    }
}

// ---------------------------------------------------------------------------
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

/// Write the `MOLT_VRANGE_REPORT` per-function diagnostics: the loop headers with
/// their trip count + per-arg SCEV, then every proven global integer range
/// (ascending by value id). A no-op unless the report flag is set by the caller.
fn emit_vrange_report(
    func: &TirFunction,
    scev: &ScevResult,
    loop_bodies: &HashMap<BlockId, HashSet<BlockId>>,
    result: &ValueRangeResult,
) {
    let mut lines = vec![format!(
        "[VRANGE] fn={} headers={:?} loop_headers={:?}",
        func.name,
        scev.headers(),
        loop_bodies.keys().collect::<Vec<_>>()
    )];
    for h in scev.headers() {
        lines.push(format!("  header {:?} trip={:?}", h, scev.trip_count(*h)));
        if let Some(hb) = func.blocks.get(h) {
            for arg in &hb.args {
                lines.push(format!(
                    "    arg v{} scev={:?}",
                    arg.id.0,
                    scev.scev_of(arg.id)
                ));
            }
        }
    }
    let mut gr: Vec<_> = result.global_range.iter().collect();
    gr.sort_by_key(|(v, _)| v.0);
    for (v, r) in gr {
        lines.push(format!("  v{} -> [{}, {}]", v.0, r.lo, r.hi));
    }
    let sanitized: String = func
        .name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let _ = crate::debug_artifacts::write_debug_artifact(
        format!("vrange_report/{sanitized}.txt"),
        lines.join("\n") + "\n",
    );
}

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
fn seed_counted_loop_iv_ranges(
    func: &TirFunction,
    loop_forest: &LoopForestResult,
    result: &mut ValueRangeResult,
) {
    use super::counted_loop::recognize_counted_loop_with_loop_forest;

    for &header in &loop_forest.headers {
        let Some(c) = recognize_counted_loop_with_loop_forest(func, header, loop_forest) else {
            continue;
        };
        let iv_canon = result.resolve(c.induction_var);
        // If SCEV already ranged this header's IV, the SCEV/guard facts are
        // authoritative — do not disturb them.
        if result.global_range.contains_key(&iv_canon) {
            continue;
        }
        // The IV's exact i128-computed hull over the proven constant trip count.
        let Some(iv_range) = affine_iv_hull(c.start, c.step, c.trip_count) else {
            continue;
        };
        // Place the IV range as a weak global + a per-body-block fact.
        result.global_range.insert(iv_canon, iv_range);
        if let Some(body) = loop_forest.bodies.get(&header) {
            for &b in body {
                let existing = result
                    .block_range
                    .get(&(b, iv_canon))
                    .copied()
                    .unwrap_or(IntRange::FULL_I64);
                result
                    .block_range
                    .insert((b, iv_canon), existing.meet(iv_range));
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

/// Forward transfer-function sweep: compute a sound loop-invariant range for
/// every op-defined integer value from its operands' ranges, to a fixpoint.
///
/// ## Why this is sound and terminating
///
/// The sweep is strictly *monotone-additive*: it only ever ASSIGNS a range to a
/// value that currently has **none** (`global_range` miss ⇒ implicitly
/// `FULL_I64`). Once a value gains a range it is never revisited. Each iteration
/// therefore strictly shrinks the set of un-ranged op results, so the fixpoint
/// is reached in at most `#values` iterations. Crucially, it **never re-derives
/// a phi / block-argument's range** (phis are not ops) and **never widens** an
/// existing fact, so:
///
///   * The IV's SCEV-proven recurrence range (seeded above) is authoritative and
///     untouched — the sweep cannot loosen it.
///   * An unbounded accumulator (`total = total + i`, a header phi with no proven
///     AddRec) keeps its `FULL_I64` (absent) range: its `Add` needs the phi's
///     range, which is FULL, so the transfer yields FULL ⇒ no fact assigned. The
///     accumulator stays un-proven and correctly falls to the boxed BigInt
///     carrier. This is the mandatory `bigint_accumulator` soundness gate: a
///     value that can exceed the inline window must never be proven inline.
///
/// Every transfer is computed in i128 and saturates to the i64 domain — a result
/// that would overflow yields a wider (sound) range, never a wrapped one.
fn propagate_op_ranges(func: &TirFunction, result: &mut ValueRangeResult) {
    // Seed `bool`-typed values as `[0, 1]` (a bool is an integer 0/1). Trivially
    // sound and lets `bool`-derived arithmetic (`a + (x < y)`) participate.
    for (&v, ty) in &func.value_types {
        if matches!(ty, crate::tir::types::TirType::Bool) {
            let canon = result.resolve(v);
            result
                .global_range
                .entry(canon)
                .or_insert(IntRange::new(0, 1));
        }
    }

    // Iterate to a fixpoint, assigning a range only to results that have none.
    // Bound the iteration count defensively by the op count (the additive
    // monotonicity already guarantees termination; this is a hard ceiling).
    let max_iters = func.blocks.values().map(|b| b.ops.len()).sum::<usize>() + 1;
    for _ in 0..max_iters {
        let mut changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                // Single-result integer ops only. (Value-identity copies — plain
                // or tagged — are already threaded by `resolve` through
                // `copy_src`; skip them here so the fact lands on the canonical
                // source rather than a copy alias.)
                if op.results.len() != 1 || copy_value_source(op).is_some() {
                    continue;
                }
                let res = result.resolve(op.results[0]);
                if result.global_range.contains_key(&res) {
                    continue; // already ranged (constant / IV / earlier sweep).
                }
                let Some(range) = transfer_op_range(op, result) else {
                    continue;
                };
                if range.is_full() {
                    continue; // no information — leave un-ranged.
                }
                result.global_range.insert(res, range);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

/// True if `r` is a phi-INDEPENDENT, genuinely BOUNDED range — the licensing
/// condition for using an incoming as a phi-narrowing JOIN contributor (see
/// [`narrow_loop_header_phis`]).
///
/// "Non-FULL" alone is INSUFFICIENT. The transfer functions ([`IntRange::add`],
/// `sub`, `mul`, `neg`, `shl_const`, …) *saturate* to the i64 endpoints rather
/// than collapsing to the exact `FULL_I64` sentinel, so a phi-DEPENDENT
/// computation under the all-phis-FULL sweep can yield a range like
/// `add(FULL, [1,1]) = [i64::MIN + 1, i64::MAX]` — not `is_full()`, yet
/// effectively unbounded and entirely a function of the FULL phi's magnitude. An
/// unbounded counter `i = i + 1` with an opaque loop bound is exactly this
/// shape, and treating its near-full back-edge range as a "bound" would wrongly
/// narrow the IV (the `e2e_nonconst_bound_no_nsw_not_proven` soundness gate).
///
/// A genuine phi-independent re-bound (`x & MASK` ⇒ `[0, MASK]`, `x % c` ⇒
/// `[0, c-1]`) has bounds that are *constants drawn from the program*, strictly
/// INTERIOR to the i64 domain — never touching either extreme. Requiring the
/// range to be strictly interior (`lo > i64::MIN && hi < i64::MAX`) therefore
/// rejects every saturated/unbounded transfer result while accepting every real
/// masked/modular bound. This is sound and loses no raw-i64 promotion: a bound
/// that reaches an i64 extreme cannot fit the `2**46` inline window anyway, so it
/// could never license a raw carrier even if narrowed.
#[inline]
fn is_phi_independent_bound(r: IntRange) -> bool {
    r.lo > i64::MIN && r.hi < i64::MAX
}

/// Narrow loop-header phis (block arguments of `LoopHeader` blocks) to the JOIN
/// of their incoming-edge ranges — the targeted producer that restores the
/// raw-i64 lane for a **masked back-edge accumulator** (`s = (s << 1) & MASK`),
/// whose carried value is re-bounded to `[0, MASK]` independently of the phi.
/// Returns `true` if any phi was narrowed (so the caller re-runs the forward
/// sweep to range values derived from the narrowed phi).
///
/// ## The soundness boundary (why a masked back edge licenses narrowing and an
/// unbounded accumulator does not)
///
/// A header phi's range is the JOIN (union) of the ranges of the values flowing
/// into it on every reachable incoming edge — *provided each of those ranges is
/// independent of the phi itself*. The danger is circularity: an accumulator
/// `total = total + i` carries `total + i` on the back edge, whose range depends
/// on `total`'s range (the phi). "Narrowing" it from a range that already
/// assumed a bound on the phi would be unsound — the accumulator can exceed the
/// inline window and even i64, requiring a heap BigInt; a false inline proof is
/// a silent truncation miscompile (the worst bug class).
///
/// This pass sidesteps the circularity WITHOUT a bespoke dependency analysis, by
/// exploiting an invariant the forward sweep ([`propagate_op_ranges`]) already
/// guarantees: **it never assigns a range to a phi**, so every op-result range
/// in `global_range` at this point was computed treating *all* phis as FULL
/// (unknown). Consequently, any incoming value whose current range is a genuine
/// **bounded interior** range ([`is_phi_independent_bound`]) is phi-INDEPENDENT
/// by construction — its bound was derived without assuming anything about any
/// phi. That is exactly the licensing condition:
///
///   * Masked back edge `s_next = (s << 1) & MASK`: under the FULL-phi sweep,
///     `s << 1` is FULL (operand `s` is FULL) but the `& MASK` re-bounds it to
///     `[0, MASK]` regardless — a *constant* range derived purely from the mask.
///     `s_next` is a bounded interior range ⇒ phi-independent ⇒ a valid JOIN
///     incoming.
///   * Unbounded accumulator back edge `total + i` / `acc << 1`: under the
///     FULL-phi sweep these are FULL or saturated-to-the-i64-extreme (a FULL
///     operand poisons the transfer). Not a bounded interior range ⇒ the
///     all-incomings test fails ⇒ NOT narrowed. The phi keeps its FULL range and
///     correctly falls to the boxed BigInt carrier.
///
/// We narrow ONLY when EVERY incoming is a bounded interior range (so the JOIN is
/// itself bounded and every contributor is phi-independent); a single
/// FULL/saturated incoming makes the phi unprovable, so we refuse — fail-closed.
/// The narrowed range is the JOIN of the incomings; it is a sound
/// over-approximation of every value the phi can hold (each iteration's value
/// flows in on some edge), and it holds for the phi everywhere it is live
/// (header, body, and the loop-exit use), so it is placed as a global fact,
/// mirroring the IV-range placement.
///
/// ## Why a single narrowing round + one re-sweep is sound (no fixpoint)
///
/// Narrowing reads ONLY the FULL-phi sweep results. A *second* narrowing round
/// would read ranges the re-sweep computed AFTER the first round narrowed some
/// phi — those are no longer guaranteed phi-independent (they may incorporate
/// the just-narrowed phi's bound), so iterating narrow→sweep→narrow could feed a
/// phi-dependent range back into a phi and lose soundness. We therefore narrow
/// exactly once, from the FULL-phi baseline, then re-sweep once to propagate the
/// narrowed phi ranges to derived values. This is complete for any set of
/// *independent* masked accumulators (each proven under the same FULL-phi
/// baseline); a phi whose mask depends on another narrowed phi is a conservative
/// miss (sound), never a miscompile.
fn narrow_loop_header_phis(
    func: &TirFunction,
    loop_bodies: &HashMap<BlockId, HashSet<BlockId>>,
    result: &mut ValueRangeResult,
) -> bool {
    // Only header phis are candidates: a loop-header block argument is the
    // canonical loop-carried (phi) value. Non-header block args (e.g. a plain
    // join point) are out of scope — the masked-accumulator shape this targets
    // is loop-carried. `loop_bodies`' keys are exactly the recognized headers.
    if loop_bodies.is_empty() {
        return false;
    }

    // Collect, per header block argument, the values flowing in on every
    // reachable incoming edge (preheader entry + back edges). Dead-edge
    // insensitive (the standard SCCP phi semantics): an unreachable source block
    // delivers no value, so its fabricated args (e.g. the vestigial
    // `loop_end → header` `ConstNone`s the SSA lift keeps as loop metadata) must
    // not contribute — counting them would inject a spurious FULL incoming and
    // defeat every narrow. Shares the `executable_reachable_blocks` oracle with
    // the raw-i64-safe phi propagation (`propagate_raw_i64_safe_values`).
    let reachable = dominators::executable_reachable_blocks(func);
    let mut incomings: HashMap<(BlockId, usize), Vec<ValueId>> = HashMap::new();
    for block in func.blocks.values() {
        if !reachable.contains(&block.id) {
            continue;
        }
        let mut add = |target: BlockId, args: &[ValueId]| {
            for (index, &arg) in args.iter().enumerate() {
                incomings.entry((target, index)).or_default().push(arg);
            }
        };
        match &block.terminator {
            Terminator::Branch { target, args } => add(*target, args),
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                add(*then_block, then_args);
                add(*else_block, else_args);
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            } => {
                for (_, target, args) in cases {
                    add(*target, args);
                }
                add(*default, default_args);
            }
            Terminator::StateDispatch {
                cases,
                default,
                default_args,
            } => {
                for (_, target, args) in cases {
                    add(*target, args);
                }
                add(*default, default_args);
            }
            Terminator::Return { .. } | Terminator::Unreachable => {}
        }
    }

    // Decide narrowings against the FROZEN FULL-phi sweep state (read-only over
    // `result`), then apply them in one batch. Computing every narrowing from
    // the same pre-narrow snapshot is what makes the rule a single round (no
    // phi's narrowed range can leak into another phi's decision this round).
    let mut narrowings: Vec<(ValueId, IntRange)> = Vec::new();
    let mut headers: Vec<BlockId> = loop_bodies.keys().copied().collect();
    headers.sort_unstable_by_key(|b| b.0); // deterministic order.
    for header in headers {
        let Some(header_block) = func.blocks.get(&header) else {
            continue;
        };
        for (index, arg) in header_block.args.iter().enumerate() {
            let phi = arg.id;
            // If the phi already has a proven range (an AddRec IV ranged above),
            // that fact is authoritative — never widen/disturb it.
            if result.global_range.contains_key(&result.resolve(phi)) {
                continue;
            }
            let Some(srcs) = incomings.get(&(header, index)) else {
                continue; // no reachable incoming edges → cannot narrow.
            };
            if srcs.is_empty() {
                continue;
            }
            // JOIN the incoming ranges. Bail to "no narrow" the instant any
            // incoming is FULL (phi-dependent or simply unproven) — fail-closed.
            // A self-referential incoming (the phi feeding itself directly, with
            // no re-bounding op) resolves to the phi, whose range is absent here
            // (we skipped already-ranged phis) ⇒ FULL ⇒ bails. So a bare
            // `x = x` / rotate-without-mask phi never narrows.
            let mut joined: Option<IntRange> = None;
            let mut all_independent = true;
            for &src in srcs {
                let r = result.range_of(src); // resolves copies; FULL if unknown.
                if !is_phi_independent_bound(r) {
                    all_independent = false;
                    break;
                }
                joined = Some(match joined {
                    None => r,
                    Some(acc) => acc.join(r),
                });
            }
            if !all_independent {
                continue;
            }
            // Every incoming is a phi-independent bounded fact ⇒ the JOIN is a
            // sound, bounded bound on the phi.
            if let Some(range) = joined {
                debug_assert!(
                    is_phi_independent_bound(range),
                    "JOIN of interior bounds must itself be an interior bound"
                );
                narrowings.push((result.resolve(phi), range));
            }
        }
    }

    if narrowings.is_empty() {
        return false;
    }
    for (phi, range) in narrowings {
        // The phi had no prior range (checked above); insert the JOIN as a weak
        // global fact. Meet with any concurrently-inserted fact for the same
        // canonical value (two header args resolving to one source — rare) so we
        // never widen.
        let existing = result
            .global_range
            .get(&phi)
            .copied()
            .unwrap_or(IntRange::FULL_I64);
        result.global_range.insert(phi, existing.meet(range));
    }
    true
}

/// The transfer function for one op: the range of its result computed from the
/// (already-proven) ranges of its operands. `None` when the opcode is not a
/// modeled integer operation; `Some(FULL_I64)` when modeled but unprovable.
///
/// Every rule here is sound over the **full i64 domain including negatives**.
/// A false (too-tight) range feeds `fits_inline_int47` → `RawI64Safe` promotion,
/// so an unsound bound is a silent BigInt-truncation miscompile. When in doubt,
/// return `FULL_I64`.
fn transfer_op_range(op: &TirOp, result: &ValueRangeResult) -> Option<IntRange> {
    // Operand range / constant helpers (resolve through plain copies).
    let r = |i: usize| -> IntRange {
        op.operands
            .get(i)
            .map(|&v| result.range_of(v))
            .unwrap_or(IntRange::FULL_I64)
    };
    let c = |i: usize| -> Option<i64> {
        op.operands
            .get(i)
            .and_then(|&v| result.const_int.get(&result.resolve(v)).copied())
    };
    match opcode_value_range_transfer_rule_table(op.opcode) {
        ValueRangeTransferRule::Add if op.operands.len() == 2 => Some(r(0).add(r(1))),
        ValueRangeTransferRule::Sub if op.operands.len() == 2 => Some(r(0).sub(r(1))),
        ValueRangeTransferRule::Mul if op.operands.len() == 2 => {
            let (a, b) = (r(0), r(1));
            // A FULL operand makes the product FULL; guard to avoid i64::MIN ·
            // huge corner-product noise (still sound, just no information).
            if a.is_full() || b.is_full() {
                Some(IntRange::FULL_I64)
            } else {
                Some(a.mul(b))
            }
        }
        ValueRangeTransferRule::Neg if op.operands.len() == 1 => Some(r(0).neg()),
        ValueRangeTransferRule::BitAnd if op.operands.len() == 2 => {
            Some(r(0).bit_and(r(1), c(0), c(1)))
        }
        ValueRangeTransferRule::BitOr if op.operands.len() == 2 => {
            Some(r(0).bit_or_xor(r(1), true))
        }
        ValueRangeTransferRule::BitXor if op.operands.len() == 2 => {
            Some(r(0).bit_or_xor(r(1), false))
        }
        ValueRangeTransferRule::Mod if op.operands.len() == 2 => {
            // Constant divisor (Python sign-of-divisor semantics); else a
            // sign-uniform, non-zero divisor range.
            if let Some(cd) = c(1) {
                if cd == 0 {
                    Some(IntRange::FULL_I64) // ZeroDivisionError — no value.
                } else {
                    Some(IntRange::mod_const(cd))
                }
            } else {
                Some(IntRange::mod_range(r(1)))
            }
        }
        ValueRangeTransferRule::FloorDiv if op.operands.len() == 2 => {
            // Constant divisor (the common `i // 3` loop-IV case) takes Python
            // sign-of-divisor floor semantics; else a sign-uniform, non-zero
            // divisor range. `//` rounds toward -inf, so the dividend's whole
            // range — not just its magnitude — drives the bound.
            if let Some(cd) = c(1) {
                if cd == 0 {
                    Some(IntRange::FULL_I64) // ZeroDivisionError — no value.
                } else {
                    Some(r(0).floordiv_const(cd))
                }
            } else {
                Some(r(0).floordiv_range(r(1)))
            }
        }
        ValueRangeTransferRule::Shr if op.operands.len() == 2 => match c(1) {
            Some(s) => Some(r(0).shr_const(s)),
            None => Some(IntRange::FULL_I64),
        },
        ValueRangeTransferRule::Shl if op.operands.len() == 2 => match c(1) {
            Some(s) => Some(r(0).shl_const(s)),
            None => Some(IntRange::FULL_I64),
        },
        _ => None,
    }
}

/// Collect `ConstInt` values, container lengths, and `len(c)` symbols.
fn collect_constants_and_lengths(func: &TirFunction, result: &mut ValueRangeResult) {
    // First pass: literal constants.
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::ConstInt
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                for &r in &op.results {
                    result.const_int.insert(r, *v);
                }
            }
        }
    }
    // Constant-fold integer `Add`/`Sub`/`Mul`/`Shl`/`Shr`/bitwise of known
    // constants to a fixpoint, so derived lengths like `n + 1` (the
    // `[True] * (n + 1)` sieve shape) AND constant bit-masks like `(1 << 32) - 1`
    // resolve to numeric bounds. The mask case is load-bearing for the masked
    // back-edge accumulator (`s = (s << 1) & MASK`): `bit_and`'s constant-mask
    // rule (`a & m, m >= 0 ⇒ [0, m]` for ANY `a`) requires `MASK` to be a known
    // constant — without folding `(1 << k) - 1`, the mask stays a non-constant
    // and `bit_and` falls back to the both-non-negative rule, which fails on the
    // FULL (negative-`lo`) shift result, leaving the masked value FULL and the
    // accumulator off the raw lane. All arithmetic is CHECKED in i64 — an
    // overflow (`1 << 70` would exceed i64) drops the value (left unknown,
    // correctly forcing the boxed BigInt path), never wraps. A negative shift
    // count yields no fold (it is a runtime `ValueError`, no static value).
    let mut changed = true;
    while changed {
        changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                let const_fold_rule = opcode_value_range_const_fold_rule_table(op.opcode);
                if const_fold_rule == ValueRangeConstFoldRule::None || op.operands.len() != 2 {
                    continue;
                }
                let Some(&a) = result.const_int.get(&result.resolve(op.operands[0])) else {
                    continue;
                };
                let Some(&b) = result.const_int.get(&result.resolve(op.operands[1])) else {
                    continue;
                };
                let folded = match const_fold_rule {
                    ValueRangeConstFoldRule::Add => a.checked_add(b),
                    ValueRangeConstFoldRule::Sub => a.checked_sub(b),
                    ValueRangeConstFoldRule::Mul => a.checked_mul(b),
                    // `a << b`: only fold a non-negative, in-i64-range count whose
                    // result fits i64 (checked). A count `>= 64`, `< 0`, or an
                    // overflowing result yields no constant (boxed BigInt path).
                    ValueRangeConstFoldRule::Shl => {
                        if (0..64).contains(&b) {
                            a.checked_shl(b as u32).filter(|&v| (v >> b) == a)
                        } else {
                            None
                        }
                    }
                    // `a >> b`: arithmetic floor shift; only a non-negative,
                    // in-range count. `a >> b` never overflows i64.
                    ValueRangeConstFoldRule::Shr => {
                        if (0..64).contains(&b) {
                            Some(a >> b)
                        } else {
                            None
                        }
                    }
                    ValueRangeConstFoldRule::BitAnd => Some(a & b),
                    ValueRangeConstFoldRule::BitOr => Some(a | b),
                    ValueRangeConstFoldRule::BitXor => Some(a ^ b),
                    ValueRangeConstFoldRule::None => None,
                };
                if let Some(v) = folded {
                    for &r in &op.results {
                        if result.const_int.insert(r, v).is_none() {
                            changed = true;
                        }
                    }
                }
            }
        }
    }
    // Second pass: container lengths (depends on constants for list-repeat).
    for block in func.blocks.values() {
        for op in &block.ops {
            match opcode_value_range_container_length_rule_table(op.opcode) {
                ValueRangeContainerLengthRule::FixedLiteral => {
                    let len = op.operands.len() as i64;
                    for &r in &op.results {
                        result
                            .container_length
                            .insert(r, KnownLength::Constant(len));
                    }
                }
                ValueRangeContainerLengthRule::ListRepeat => {
                    if op.operands.len() == 2 {
                        // list-repeat: Mul(list_of_1, count) → length == count.
                        // Resolve operands through copies to reach the BuildList /
                        // const sources.
                        let (a, b) = (
                            result.resolve(op.operands[0]),
                            result.resolve(op.operands[1]),
                        );
                        let count = if result
                            .container_length
                            .get(&a)
                            .is_some_and(|l| matches!(l, KnownLength::Constant(1)))
                        {
                            Some(b)
                        } else if result
                            .container_length
                            .get(&b)
                            .is_some_and(|l| matches!(l, KnownLength::Constant(1)))
                        {
                            Some(a)
                        } else {
                            None
                        };
                        if let Some(count_val) = count {
                            for &r in &op.results {
                                if let Some(&c) = result.const_int.get(&count_val) {
                                    result.container_length.insert(r, KnownLength::Constant(c));
                                } else {
                                    result
                                        .container_length
                                        .insert(r, KnownLength::SameAs(count_val));
                                }
                            }
                        }
                    }
                }
                ValueRangeContainerLengthRule::LenCall => {
                    let name = op
                        .attrs
                        .get("name")
                        .and_then(|v| match v {
                            AttrValue::Str(s) => Some(s.as_str()),
                            _ => None,
                        })
                        .unwrap_or("");
                    if name == "len" && op.operands.len() == 1 {
                        let container = result.resolve(op.operands[0]);
                        for &r in &op.results {
                            result.len_of.insert(r, container);
                        }
                    }
                }
                ValueRangeContainerLengthRule::None => {}
            }
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
fn back_edge_update_value(
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
fn narrow_from_header_guards(
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
        let bound_const = result.const_int.get(&bound).copied();
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
    let existing = result
        .block_range
        .get(&(bid, var))
        .copied()
        .or_else(|| result.global_range.get(&var).copied())
        .unwrap_or(IntRange::FULL_I64);
    result.block_range.insert((bid, var), existing.meet(range));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, TirBlock};
    use crate::tir::passes::scev::compute_scev;

    #[test]
    fn proves_in_bounds_const_index() {
        // Direct query on a hand-built result: container of length 3, index 2.
        let bid = BlockId(0);
        let lst = ValueId(100);
        let mut res = ValueRangeResult::default();
        res.container_length.insert(lst, KnownLength::Constant(3));

        let idx = ValueId(101);
        res.global_range.insert(idx, IntRange::point(2));
        assert!(res.proves_index_in_bounds(bid, lst, idx));

        // index 3 into len-3 container → unsafe (3 is out of bounds).
        let idx3 = ValueId(102);
        res.global_range.insert(idx3, IntRange::point(3));
        assert!(!res.proves_index_in_bounds(bid, lst, idx3));

        // negative index → unsafe.
        let idxn = ValueId(103);
        res.global_range.insert(idxn, IntRange::point(-1));
        assert!(!res.proves_index_in_bounds(bid, lst, idxn));

        // unknown range → unsafe.
        let idxu = ValueId(104);
        assert!(!res.proves_index_in_bounds(bid, lst, idxu));

        // unbounded-above range → unsafe even though lo >= 0.
        let idxh = ValueId(105);
        res.global_range.insert(idxh, IntRange::new(0, i64::MAX));
        assert!(!res.proves_index_in_bounds(bid, lst, idxh));
    }

    #[test]
    fn symbolic_lt_len_proof() {
        // `while i < len(lst): lst[i]` — i guarded `< len_val`, len_val=len(lst).
        let bid = BlockId(1);
        let lst = ValueId(10);
        let i = ValueId(11);
        let len_val = ValueId(12);
        let mut res = ValueRangeResult::default();
        res.len_of.insert(len_val, lst);
        // i is provably >= 0 (an IV from 0).
        res.global_range.insert(i, IntRange::new(0, i64::MAX));
        res.record_symbolic_lt(bid, i, len_val);
        assert!(res.proves_index_lt_len_symbolically(bid, lst, i));
        // wrong container → not proven.
        let other = ValueId(99);
        assert!(!res.proves_index_lt_len_symbolically(bid, other, i));
    }

    #[test]
    fn unknown_everything_proves_nothing() {
        let res = ValueRangeResult::default();
        assert!(!res.proves_index_in_bounds(BlockId(0), ValueId(0), ValueId(1)));
        assert!(!res.fits_inline_int47(ValueId(0)));
        let _ = TirBlock {
            id: BlockId(0),
            args: vec![],
            ops: vec![],
            terminator: Terminator::Return { values: vec![] },
        };
    }

    // -- end-to-end through compute_value_range + compute_scev ---------------
    // (BlockId/Terminator/LoopRole/OpCode/AttrValue/ValueId come via super::*;
    // TirBlock via the module-level test import.)

    use crate::tir::blocks::TirBlock as Blk;
    use crate::tir::ops::{AttrDict, Dialect, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::TirValue;

    fn op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }
    fn op_nsw(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        let mut o = op(opcode, operands, results);
        o.attrs
            .insert("no_signed_wrap".into(), AttrValue::Bool(true));
        o
    }
    fn cint(result: ValueId, value: i64) -> TirOp {
        let mut o = op(OpCode::ConstInt, vec![], vec![result]);
        o.attrs.insert("value".into(), AttrValue::Int(value));
        o
    }

    fn mark_i64_values(func: &mut TirFunction, values: impl IntoIterator<Item = ValueId>) {
        for value in values {
            func.value_types.insert(value, TirType::I64);
        }
    }

    fn mark_bool_values(func: &mut TirFunction, values: impl IntoIterator<Item = ValueId>) {
        for value in values {
            func.value_types.insert(value, TirType::Bool);
        }
    }

    /// `for i in range(stop): a[i]` where `a = [0]*list_len` — built in the
    /// canonical post-range_devirt shape and run through the real
    /// compute_scev + compute_value_range pipeline.
    fn range_loop_vr(list_len: i64, stop: i64) -> (TirFunction, BlockId, ValueId, ValueId) {
        let mut func = TirFunction::new("rl".into(), vec![], TirType::None);
        let one = func.fresh_value();
        let elem = func.fresh_value();
        let list1 = func.fresh_value();
        let lenv = func.fresh_value();
        let a = func.fresh_value();
        let start = func.fresh_value();
        let stop_v = func.fresh_value();
        let step = func.fresh_value();
        let iv = func.fresh_value();
        let cond = func.fresh_value();
        let next = func.fresh_value();
        let r = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                cint(one, 1),
                cint(elem, 0),
                op(OpCode::BuildList, vec![elem], vec![list1]),
                cint(lenv, list_len),
                op(OpCode::Mul, vec![list1, lenv], vec![a]),
                cint(start, 0),
                cint(stop_v, stop),
                cint(step, 1),
            ];
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start],
            };
        }
        func.blocks.insert(
            header,
            Blk {
                id: header,
                args: vec![TirValue {
                    id: iv,
                    ty: TirType::I64,
                }],
                ops: vec![op(OpCode::Lt, vec![iv, stop_v], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            body,
            Blk {
                id: body,
                args: vec![],
                ops: vec![
                    op(OpCode::Index, vec![a, iv], vec![r]),
                    op_nsw(OpCode::Add, vec![iv, step], vec![next]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next],
                },
            },
        );
        func.blocks.insert(
            exit,
            Blk {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit, LoopRole::LoopEnd);
        (func, body, a, iv)
    }

    #[test]
    fn e2e_range_loop_iv_in_bounds() {
        // a has length 10, for i in range(10): i in [0,9] < 10 → in bounds.
        let (func, body, a, iv) = range_loop_vr(10, 10);
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        // The IV range over the body is [0, 9].
        assert_eq!(vr.range_at(body, iv), IntRange::new(0, 9));
        assert!(vr.proves_index_in_bounds(body, a, iv));
    }

    #[test]
    fn e2e_range_loop_container_too_small_not_proven() {
        // a has length 3, for i in range(10): i can reach 9 > 2 → NOT in bounds.
        let (func, body, a, iv) = range_loop_vr(3, 10);
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert!(
            !vr.proves_index_in_bounds(body, a, iv),
            "container shorter than the IV's max must NOT be provable in-bounds"
        );
    }

    #[test]
    fn e2e_counted_loop_const_bound_proven_without_nsw() {
        // The frontend's counted-loop shape lowers `for i in range(10)` to an
        // arithmetic loop whose `Add(i, 1)` is NOT nsw-tagged (SCEV refuses the
        // AddRec). The counted-loop recognizer proves start=0/step=1/trip=10 from
        // the CONSTANT guard bound, so the IV range [0,9] is recovered soundly —
        // the producer that unblocks SROA/BCE on the dominant counted-loop shape.
        let (mut func, body, a, iv) = range_loop_vr(10, 10);
        for op in func.blocks.get_mut(&body).unwrap().ops.iter_mut() {
            if op.opcode == OpCode::Add {
                op.attrs.remove("no_signed_wrap");
            }
        }
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        // SCEV gives no AddRec, but the counted-loop recognizer still proves it.
        assert_eq!(
            vr.range_of(iv),
            IntRange::new(0, 9),
            "counted-loop recognizer must recover the IV range from the const bound"
        );
        assert!(
            vr.proves_index_in_bounds(body, a, iv),
            "a constant-bounded counted loop is provably in-bounds even without nsw"
        );
    }

    #[test]
    fn counted_loop_fallback_uses_loopforest_without_loop_roles() {
        let (mut func, body, a, iv) = range_loop_vr(10, 10);
        func.loop_roles.clear();
        for op in func.blocks.get_mut(&body).unwrap().ops.iter_mut() {
            if op.opcode == OpCode::Add {
                op.attrs.remove("no_signed_wrap");
            }
        }

        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);

        assert!(
            !scev.is_induction_var(iv),
            "without nsw, SCEV must not be the source of this range"
        );
        assert_eq!(vr.range_of(iv), IntRange::new(0, 9));
        assert!(vr.proves_index_in_bounds(body, a, iv));
    }

    #[test]
    fn e2e_nonconst_bound_no_nsw_not_proven() {
        // The genuinely-unprovable case: a NON-CONSTANT stop bound AND no nsw.
        // The counted-loop recognizer needs a ConstInt stop (it gets none here),
        // and SCEV needs nsw (stripped) — so NEITHER prover fires and the IV has
        // no range. BCE must NOT fire (fail-closed).
        let (mut func, body, a, iv) = range_loop_vr(10, 10);
        // Make the stop bound a non-constant: replace the `Lt(iv, stop_v)` RHS
        // with a fresh value that has no ConstInt def (an opaque parameter-like
        // value). Find the Lt op (in the header) and the stop ConstInt, and drop
        // the constant by re-pointing Lt's RHS at a never-defined value.
        let opaque = func.fresh_value();
        for block in func.blocks.values_mut() {
            for op in block.ops.iter_mut() {
                if op.opcode == OpCode::Lt && op.operands.len() == 2 {
                    op.operands[1] = opaque; // RHS now has no constant/def → opaque.
                }
            }
        }
        // Strip nsw so SCEV cannot form the AddRec either.
        for op in func.blocks.get_mut(&body).unwrap().ops.iter_mut() {
            if op.opcode == OpCode::Add {
                op.attrs.remove("no_signed_wrap");
            }
        }
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert!(
            vr.range_of(iv).is_full(),
            "IV with neither a const bound nor nsw must have NO proven range"
        );
        assert!(
            !vr.proves_index_in_bounds(body, a, iv),
            "a possibly-wrapping IV must not yield a bounds proof"
        );
    }

    /// Build `for i in range(stop): ...` with extra derived ops in the body, the
    /// `p.y = i + 1`-style values the forward sweep must range. Returns the func
    /// plus the derived-value ids.
    fn range_loop_with_derived(
        stop: i64,
    ) -> (TirFunction, ValueId, ValueId, ValueId, ValueId, ValueId) {
        let (mut func, body, _a, iv) = range_loop_vr(64, stop);
        let one = func.fresh_value();
        let mask = func.fresh_value();
        let m4 = func.fresh_value();
        let sh = func.fresh_value();
        let i_plus_1 = func.fresh_value();
        let i_and_15 = func.fresh_value();
        let i_mod_4 = func.fresh_value();
        let i_shl_30 = func.fresh_value();
        let block = func.blocks.get_mut(&body).unwrap();
        // Prepend the constants for the derived ops, then the derived ops.
        let mut new_ops = vec![
            cint(one, 1),
            cint(mask, 15),
            cint(m4, 4),
            cint(sh, 30),
            op(OpCode::Add, vec![iv, one], vec![i_plus_1]),
            op(OpCode::BitAnd, vec![iv, mask], vec![i_and_15]),
            op(OpCode::Mod, vec![iv, m4], vec![i_mod_4]),
            op(OpCode::Shl, vec![iv, sh], vec![i_shl_30]),
        ];
        new_ops.append(&mut block.ops);
        block.ops = new_ops;
        (func, iv, i_plus_1, i_and_15, i_mod_4, i_shl_30)
    }

    #[test]
    fn e2e_derived_values_get_proven_ranges() {
        // for i in range(10): i in [0,9]. The forward sweep must prove the
        // derived store values that today block SROA's hot-loop promotion.
        let (func, iv, i_plus_1, i_and_15, i_mod_4, i_shl_30) = range_loop_with_derived(10);
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert_eq!(vr.range_of(iv), IntRange::new(0, 9), "IV body range");
        // i + 1 ∈ [1, 10] — the `p.y = i + 1` shape.
        assert_eq!(vr.range_of(i_plus_1), IntRange::new(1, 10));
        assert!(vr.fits_inline_int47(i_plus_1));
        // i & 15 ∈ [0, 15] (mask bound — holds even if i were unknown).
        assert_eq!(vr.range_of(i_and_15), IntRange::new(0, 15));
        assert!(vr.fits_inline_int47(i_and_15));
        // i % 4 ∈ [0, 3].
        assert_eq!(vr.range_of(i_mod_4), IntRange::new(0, 3));
        assert!(vr.fits_inline_int47(i_mod_4));
        // i << 30 ∈ [0, 9 << 30] = [0, 9663676416] — still well within 2^46.
        assert_eq!(vr.range_of(i_shl_30), IntRange::new(0, 9i64 << 30));
        assert!(vr.fits_inline_int47(i_shl_30));
    }

    #[test]
    fn e2e_shl_past_inline_window_not_proven_inline() {
        // for i in range(10): i << 45 reaches 9 << 45 ≈ 3.2e14 > 2^46-1 ⇒ the
        // range is proven but does NOT fit the inline window (must stay boxed).
        let (mut func, _iv, _p1, _a15, _m4, _shl30) = range_loop_with_derived(10);
        // Find the Shl op and bump its shift constant to 45 (overflow window).
        let mut sh_id = None;
        for block in func.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::Shl {
                    sh_id = Some(op.operands[1]);
                }
            }
        }
        let sh_id = sh_id.unwrap();
        for block in func.blocks.values_mut() {
            for op in block.ops.iter_mut() {
                if op.opcode == OpCode::ConstInt && op.results.first() == Some(&sh_id) {
                    op.attrs.insert("value".into(), AttrValue::Int(45));
                }
            }
        }
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        // The Shl result is the value whose def op is Shl.
        let mut shl_res = None;
        for block in func.blocks.values() {
            for op in &block.ops {
                if op.opcode == OpCode::Shl {
                    shl_res = Some(op.results[0]);
                }
            }
        }
        let shl_res = shl_res.unwrap();
        // Range is proven ([0, 9<<45]) but does NOT fit the inline window.
        assert_eq!(vr.range_of(shl_res), IntRange::new(0, 9i64 << 45));
        assert!(
            !vr.fits_inline_int47(shl_res),
            "9<<45 exceeds 2^46-1 — must NOT be proven inline"
        );
    }

    /// `for i in range(stop): q = i // divisor`, with `q` derived in the loop
    /// body. Returns the func, the IV, and the floordiv result value.
    fn range_loop_with_floordiv(stop: i64, divisor: i64) -> (TirFunction, ValueId, ValueId) {
        let (mut func, body, _a, iv) = range_loop_vr(64, stop);
        let d = func.fresh_value();
        let q = func.fresh_value();
        let block = func.blocks.get_mut(&body).unwrap();
        let mut new_ops = vec![cint(d, divisor), op(OpCode::FloorDiv, vec![iv, d], vec![q])];
        new_ops.append(&mut block.ops);
        block.ops = new_ops;
        (func, iv, q)
    }

    #[test]
    fn e2e_floordiv_const_proven_inline() {
        // (a) for i in range(1000): i // 3 ∈ [0, 333] ⊂ inline-int47, so the
        // result keeps the raw-i64 lane instead of boxing to MaybeBigInt — the
        // perf unlock for the numeric-loop class.
        let (func, iv, q) = range_loop_with_floordiv(1000, 3);
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert_eq!(vr.range_of(iv), IntRange::new(0, 999), "IV body range");
        assert_eq!(vr.range_of(q), IntRange::new(0, 333), "999 // 3 == 333");
        assert!(
            vr.fits_inline_int47(q),
            "i // 3 over [0, 999] must be proven inline (raw lane fires)"
        );
    }

    #[test]
    fn e2e_floordiv_negative_dividend_floor_exact() {
        // (b) for i in range(10): q = (-i) // 3. The IV is [0, 9] ⇒ -i ∈ [-9, 0],
        // and Python floor division rounds toward -inf, so (-i) // 3 ∈ [-3, 0]
        // (a truncating divide would mis-bound the low end). Exact negative-
        // dividend rounding through the real transfer keeps the bound sound.
        let (mut func, body, _a, iv) = range_loop_vr(64, 10);
        let neg = func.fresh_value();
        let d = func.fresh_value();
        let q = func.fresh_value();
        let block = func.blocks.get_mut(&body).unwrap();
        let mut new_ops = vec![
            cint(d, 3),
            op(OpCode::Neg, vec![iv], vec![neg]),
            op(OpCode::FloorDiv, vec![neg, d], vec![q]),
        ];
        new_ops.append(&mut block.ops);
        block.ops = new_ops;
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert_eq!(vr.range_of(neg), IntRange::new(-9, 0), "-i over i ∈ [0, 9]");
        assert_eq!(
            vr.range_of(q),
            IntRange::new(-3, 0),
            "(-i) // 3 floors toward -inf to [-3, 0]"
        );
        assert!(vr.fits_inline_int47(q));
    }

    #[test]
    fn e2e_floordiv_divisor_spanning_zero_not_proven() {
        // (c) for i in range(10): q = i // k, where k is an opaque (unranged)
        // value. Its range is FULL (spans 0), so the divisor is not provably
        // non-zero/sign-uniform ⇒ NO range proof for q. Fail-closed: a possible
        // ZeroDivisionError or sign flip must never yield a tight bound (a false
        // bound here is the inline-int47 truncation P0).
        let (mut func, body, _a, iv) = range_loop_vr(64, 10);
        let k = func.fresh_value(); // opaque: never given a ConstInt def.
        let q = func.fresh_value();
        let block = func.blocks.get_mut(&body).unwrap();
        let mut new_ops = vec![op(OpCode::FloorDiv, vec![iv, k], vec![q])];
        new_ops.append(&mut block.ops);
        block.ops = new_ops;
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert!(
            vr.range_of(q).is_full(),
            "opaque (zero-spanning) divisor ⇒ no proof"
        );
        assert!(
            !vr.fits_inline_int47(q),
            "unproven floordiv result must stay boxed"
        );
    }

    /// The shift-overflow contract's count-validity gate (task #34): a `Shl`/
    /// `Shr` result whose machine shift COUNT is proven outside `[0, 63]` must
    /// NOT be a raw-i64-safe carrier, *even when its result range fits the
    /// inline window*. `0 << 70` ranges to `[0, 0]` (fits inline) yet a raw
    /// machine `shl` by 70 is LLVM poison / a wasm wrong-value mask-mod-64, so
    /// the seed (`raw_i64_safe_values_for` — the single source of truth the
    /// LLVM/WASM shift lanes consult) must exclude it, routing the shift to the
    /// BigInt-/exception-correct boxed runtime. The proven-`[0, 63]` count case
    /// (`5 << 3`) stays raw.
    #[test]
    fn shl_count_outside_0_63_is_not_raw_i64_safe() {
        use crate::representation_facts::raw_i64_safe_values_for;
        let mut func = TirFunction::new("shl_count_gate".into(), vec![], TirType::None);
        let zero = func.fresh_value();
        let big_count = func.fresh_value();
        let bad_res = func.fresh_value(); // 0 << 70  (result fits inline, count > 63)
        let five = func.fresh_value();
        let small_count = func.fresh_value();
        let good_res = func.fresh_value(); // 5 << 3   (result fits inline, count in [0,63])
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                cint(zero, 0),
                cint(big_count, 70),
                op(OpCode::Shl, vec![zero, big_count], vec![bad_res]),
                cint(five, 5),
                cint(small_count, 3),
                op(OpCode::Shl, vec![five, small_count], vec![good_res]),
            ];
            entry.terminator = Terminator::Return { values: vec![] };
        }
        mark_i64_values(
            &mut func,
            [zero, big_count, bad_res, five, small_count, good_res],
        );
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        // Both results are range-proven inside the inline window...
        assert_eq!(vr.range_of(bad_res), IntRange::point(0), "0<<70 == 0");
        assert!(vr.fits_inline_int47(bad_res));
        assert_eq!(vr.range_of(good_res), IntRange::point(40), "5<<3 == 40");
        assert!(vr.fits_inline_int47(good_res));
        // ...but only the in-range-count shift may carry a raw i64.
        let raw = raw_i64_safe_values_for(&func, &vr);
        assert!(
            !raw.contains(&bad_res),
            "0<<70: machine count 70 is out of [0,63] — must NOT be raw-i64-safe"
        );
        assert!(
            raw.contains(&good_res),
            "5<<3: count 3 is in [0,63] and result fits inline — stays raw"
        );
    }

    #[test]
    fn e2e_unbounded_accumulator_stays_unranged() {
        // The mandatory bigint_accumulator soundness gate: an accumulator phi
        // `total = total + i` whose SCEV is NOT a proven AddRec must keep its
        // FULL (absent) range — the forward sweep must NEVER prove it inline.
        let mut func = TirFunction::new("acc".into(), vec![], TirType::None);
        let start_i = func.fresh_value();
        let start_t = func.fresh_value();
        let stop_v = func.fresh_value();
        let step = func.fresh_value();
        let iv = func.fresh_value();
        let total = func.fresh_value();
        let cond = func.fresh_value();
        let next_i = func.fresh_value();
        let next_t = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                cint(start_i, 0),
                cint(start_t, 0),
                cint(stop_v, 1000000),
                cint(step, 1),
            ];
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start_i, start_t],
            };
        }
        func.blocks.insert(
            header,
            Blk {
                id: header,
                args: vec![
                    TirValue {
                        id: iv,
                        ty: TirType::I64,
                    },
                    TirValue {
                        id: total,
                        ty: TirType::I64,
                    },
                ],
                ops: vec![op(OpCode::Lt, vec![iv, stop_v], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            body,
            Blk {
                id: body,
                args: vec![],
                // total = total + i  (an accumulator — NOT an affine recurrence).
                ops: vec![
                    op(OpCode::Add, vec![total, iv], vec![next_t]),
                    op_nsw(OpCode::Add, vec![iv, step], vec![next_i]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_i, next_t],
                },
            },
        );
        func.blocks.insert(
            exit,
            Blk {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit, LoopRole::LoopEnd);

        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        // The IV is a proper AddRec and IS ranged.
        assert!(vr.fits_inline_int47(iv) || vr.range_of(iv).hi <= 999_999);
        // The accumulator phi and its update MUST stay un-proven (FULL).
        assert!(
            !vr.fits_inline_int47(total),
            "unbounded accumulator phi must never be proven inline"
        );
        assert!(
            !vr.fits_inline_int47(next_t),
            "accumulator update (total + i) must never be proven inline — it can \
             exceed the inline window and even i64, requiring a boxed BigInt"
        );
        assert!(
            vr.range_of(next_t).is_full(),
            "total + i range must be FULL"
        );
    }

    /// Build a single-latch loop whose header carries ONE phi `s` (start `s0`),
    /// with a body that computes `s_next = (s << shift) & mask` (when `mask` is
    /// `Some`) or `s_next = s << shift` (when `mask` is `None`), branching back
    /// with `s_next`. The loop is gated by a constant counter `for _ in
    /// range(trip)` so it is a recognized counted loop (header role set). Returns
    /// `(func, s_phi, s_next, shl_result)`.
    ///
    /// This is the masked-shift-accumulator shape (#43): with `mask = Some`, the
    /// back-edge value is re-bounded to `[0, mask]` independently of the phi, so
    /// the phi must narrow; with `mask = None`, the back-edge `s << 1` is FULL
    /// (operand `s` is FULL), so the phi must NOT narrow (adversarial / bigint).
    fn masked_shift_loop(
        s0: i64,
        shift: i64,
        mask: Option<i64>,
        trip: i64,
    ) -> (TirFunction, ValueId, ValueId, ValueId) {
        let mut func = TirFunction::new("msl".into(), vec![], TirType::None);
        // Counter machinery (drives a constant trip count so the header is a
        // recognized loop with a constant guard) + the accumulator.
        let start_i = func.fresh_value();
        let stop_v = func.fresh_value();
        let step = func.fresh_value();
        let s_start = func.fresh_value();
        let shift_c = func.fresh_value();
        let mask_c = func.fresh_value();
        let iv = func.fresh_value();
        let s_phi = func.fresh_value();
        let cond = func.fresh_value();
        let shl_res = func.fresh_value();
        let s_next = func.fresh_value();
        let next_i = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            let mut ops = vec![
                cint(start_i, 0),
                cint(stop_v, trip),
                cint(step, 1),
                cint(s_start, s0),
                cint(shift_c, shift),
            ];
            if let Some(m) = mask {
                ops.push(cint(mask_c, m));
            }
            entry.ops = ops;
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start_i, s_start],
            };
        }
        func.blocks.insert(
            header,
            Blk {
                id: header,
                args: vec![
                    TirValue {
                        id: iv,
                        ty: TirType::I64,
                    },
                    TirValue {
                        id: s_phi,
                        ty: TirType::I64,
                    },
                ],
                ops: vec![op(OpCode::Lt, vec![iv, stop_v], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        let mut body_ops = vec![op(OpCode::Shl, vec![s_phi, shift_c], vec![shl_res])];
        // The carried value: masked (re-bounds to [0, mask]) or bare (FULL).
        if mask.is_some() {
            body_ops.push(op(OpCode::BitAnd, vec![shl_res, mask_c], vec![s_next]));
        } else {
            // No mask: the carried value IS the shift result. Use a plain copy so
            // the back-edge arg is a distinct id (mirrors `s = s << 1`).
            body_ops.push(op(OpCode::Copy, vec![shl_res], vec![s_next]));
        }
        body_ops.push(op_nsw(OpCode::Add, vec![iv, step], vec![next_i]));
        func.blocks.insert(
            body,
            Blk {
                id: body,
                args: vec![],
                ops: body_ops,
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_i, s_next],
                },
            },
        );
        func.blocks.insert(
            exit,
            Blk {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit, LoopRole::LoopEnd);
        mark_i64_values(
            &mut func,
            [
                start_i, stop_v, step, s_start, shift_c, mask_c, iv, s_phi, shl_res, s_next, next_i,
            ],
        );
        mark_bool_values(&mut func, [cond]);
        (func, s_phi, s_next, shl_res)
    }

    #[test]
    fn masked_back_edge_phi_narrows_and_is_raw_safe() {
        // s = (s << 1) & MASK, MASK = 2**32 - 1. The masked back-edge value is
        // [0, MASK] INDEPENDENT of the phi, so the phi must narrow to [0, MASK].
        use crate::representation_facts::raw_i64_safe_values_for;
        let mask = (1i64 << 32) - 1;
        let (func, s_phi, s_next, shl_res) = masked_shift_loop(1, 1, Some(mask), 64);
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);

        // The back-edge masked value is bounded by the mask (phi-independent).
        assert_eq!(
            vr.range_of(s_next),
            IntRange::new(0, mask),
            "masked back-edge value must be [0, MASK] under the FULL-phi sweep"
        );
        // The header phi is narrowed to the JOIN of {start=[1,1], [0, MASK]}.
        assert_eq!(
            vr.range_of(s_phi),
            IntRange::new(0, mask),
            "masked-accumulator phi must narrow to the JOIN of its incomings"
        );
        assert!(
            vr.fits_inline_int47(s_phi),
            "[0, 2**32-1] fits the 2**46 inline window"
        );
        // The re-sweep ranges the shift result `s << 1` to [0, MASK<<1], which
        // fits the inline window — the value the raw-i64 shift seed needs.
        assert_eq!(vr.range_of(shl_res), IntRange::new(0, mask << 1));
        assert!(vr.fits_inline_int47(shl_res));

        // End-to-end: the shift result IS now a raw-i64-safe carrier (count 1 in
        // [0,63] AND result fits inline) — the boxed `molt_lshift` lane is gone.
        let raw = raw_i64_safe_values_for(&func, &vr);
        assert!(
            raw.contains(&shl_res),
            "the masked-accumulator shift must be raw-i64-safe post-narrowing"
        );
        assert!(
            raw.contains(&s_phi),
            "the narrowed phi must propagate to a raw-i64 carrier (all incomings raw)"
        );
    }

    #[test]
    fn non_masked_back_edge_phi_does_not_narrow() {
        // ADVERSARIAL (the soundness gate): s = s << 1 with NO mask. The
        // back-edge value `s << 1` has FULL range (operand `s` is FULL under the
        // FULL-phi sweep), so the phi must NOT narrow — it can grow into a heap
        // BigInt (`1 << 70` overflows i64), and a false inline proof would be a
        // silent truncation miscompile.
        use crate::representation_facts::raw_i64_safe_values_for;
        let (func, s_phi, s_next, shl_res) = masked_shift_loop(1, 1, None, 70);
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);

        assert!(
            vr.range_of(s_next).is_full(),
            "unmasked `s << 1` back-edge must stay FULL (phi-dependent)"
        );
        assert!(
            vr.range_of(s_phi).is_full(),
            "unmasked doubling phi must NOT narrow — it grows past i64"
        );
        assert!(
            !vr.fits_inline_int47(s_phi),
            "unbounded doubling accumulator must never be proven inline"
        );
        assert!(
            !vr.fits_inline_int47(shl_res),
            "the unproven shift result must never be proven inline"
        );
        // End-to-end: NOT raw-i64-safe — the boxed BigInt-correct lane is kept.
        let raw = raw_i64_safe_values_for(&func, &vr);
        assert!(
            !raw.contains(&shl_res),
            "an unbounded doubling shift must NOT be raw-i64-safe (would truncate)"
        );
        assert!(
            !raw.contains(&s_phi),
            "the unbounded doubling phi must NOT be raw-i64-safe"
        );
    }

    #[test]
    fn masked_back_edge_narrows_with_wider_shift() {
        // s = (s << 4) & (2**28 - 1): a wider per-step shift, still bounded by
        // the mask. The phi narrows to [0, mask]; the shift result [0, mask<<4]
        // must still fit the inline window (mask<<4 = 2**32-16 < 2**46).
        let mask = (1i64 << 28) - 1;
        let (func, s_phi, s_next, shl_res) = masked_shift_loop(3, 4, Some(mask), 20);
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert_eq!(vr.range_of(s_next), IntRange::new(0, mask));
        assert_eq!(vr.range_of(s_phi), IntRange::new(0, mask));
        assert_eq!(vr.range_of(shl_res), IntRange::new(0, mask << 4));
        assert!(vr.fits_inline_int47(shl_res));
    }

    #[test]
    fn masked_back_edge_does_not_narrow_when_mask_overflows_window() {
        // s = (s << 1) & (2**48 - 1): the mask itself exceeds the 2**46 inline
        // window, so the phi narrows to [0, 2**48-1] (a SOUND fact) but it must
        // NOT be proven inline — the value genuinely can exceed the window.
        let mask = (1i64 << 48) - 1;
        let (func, s_phi, _s_next, shl_res) = masked_shift_loop(1, 1, Some(mask), 100);
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert_eq!(
            vr.range_of(s_phi),
            IntRange::new(0, mask),
            "phi narrows to the (sound) masked bound even when it exceeds the window"
        );
        assert!(
            !vr.fits_inline_int47(s_phi),
            "a [0, 2**48-1] phi must NOT be proven inline (exceeds 2**46)"
        );
        assert!(
            !vr.fits_inline_int47(shl_res),
            "the shift result of an out-of-window masked phi must not be inline"
        );
    }

    #[test]
    fn masked_back_edge_narrows_with_derived_mask_and_vestigial_loopend() {
        // REAL-COMPILE FIDELITY: the frontend lowering of
        //   MASK = (1 << 32) - 1; s = 1
        //   for _ in range(N): s = (s << 1) & MASK
        // differs from `masked_shift_loop` in two structural ways that BOTH must
        // be tolerated for the narrowing to fire end-to-end (observed in the
        // bench_masked_shift_accumulator TIR dump):
        //
        //   (1) MASK is a DERIVED constant `(1 << 32) - 1`, not a literal
        //       `ConstInt`. Its `[0, MASK]` re-bound only materializes once
        //       `collect_constants_and_lengths` folds `Shl`/`Sub` of constants so
        //       `bit_and`'s constant-mask rule sees a known non-negative mask.
        //   (2) The SSA lift keeps a VESTIGIAL `loop_end -> header` back edge whose
        //       args are fabricated `ConstNone`s. That block is UNREACHABLE (no
        //       predecessor) and so must be excluded by the
        //       `executable_reachable_blocks` oracle — otherwise its FULL ConstNone
        //       incoming poisons the phi JOIN and defeats the narrow.
        //
        // This is the adversarial mirror of the unit `masked_shift_loop`: same
        // licensing structure, but built in the shape the compiler actually emits.
        let mut func = TirFunction::new("dm".into(), vec![], TirType::None);
        // Mask materials: one=1, k=32, then mask = (one << k) - 1 (DERIVED).
        let one_c = func.fresh_value();
        let k_c = func.fresh_value();
        let mask_shl = func.fresh_value();
        let mask = func.fresh_value();
        // Counter + accumulator seeds.
        let start_i = func.fresh_value();
        let stop_v = func.fresh_value();
        let step = func.fresh_value();
        let s_start = func.fresh_value();
        let shift_c = func.fresh_value();
        // Header phis.
        let iv = func.fresh_value();
        let s_phi = func.fresh_value();
        let cond = func.fresh_value();
        // Body values.
        let shl_res = func.fresh_value();
        let s_next = func.fresh_value();
        let next_i = func.fresh_value();
        // Vestigial loop-end ConstNone.
        let dead_none = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();
        let dead_end = func.fresh_block();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                cint(one_c, 1),
                cint(k_c, 32),
                // mask = (1 << 32) - 1 — derived, must be const-folded.
                op(OpCode::Shl, vec![one_c, k_c], vec![mask_shl]),
                cint(start_i, 0),
                op(OpCode::Sub, vec![mask_shl, one_c], vec![mask]),
                cint(stop_v, 64),
                cint(step, 1),
                cint(s_start, 1),
                cint(shift_c, 1),
            ];
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start_i, s_start],
            };
        }
        func.blocks.insert(
            header,
            Blk {
                id: header,
                args: vec![
                    TirValue {
                        id: iv,
                        ty: TirType::I64,
                    },
                    TirValue {
                        id: s_phi,
                        ty: TirType::I64,
                    },
                ],
                ops: vec![op(OpCode::Lt, vec![iv, stop_v], vec![cond])],
                terminator: Terminator::CondBranch {
                    cond,
                    then_block: body,
                    then_args: vec![],
                    else_block: exit,
                    else_args: vec![],
                },
            },
        );
        func.loop_roles.insert(header, LoopRole::LoopHeader);
        func.blocks.insert(
            body,
            Blk {
                id: body,
                args: vec![],
                ops: vec![
                    op(OpCode::Shl, vec![s_phi, shift_c], vec![shl_res]),
                    op(OpCode::BitAnd, vec![shl_res, mask], vec![s_next]),
                    op_nsw(OpCode::Add, vec![iv, step], vec![next_i]),
                ],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next_i, s_next],
                },
            },
        );
        func.blocks.insert(
            exit,
            Blk {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        // The vestigial, UNREACHABLE loop-end: branches back to the header with
        // fabricated ConstNone args. No block branches INTO it.
        func.blocks.insert(
            dead_end,
            Blk {
                id: dead_end,
                args: vec![],
                ops: vec![op(OpCode::ConstNone, vec![], vec![dead_none])],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![dead_none, dead_none],
                },
            },
        );
        func.loop_roles.insert(dead_end, LoopRole::LoopEnd);
        mark_i64_values(
            &mut func,
            [
                one_c, k_c, mask_shl, mask, start_i, stop_v, step, s_start, shift_c, iv, s_phi,
                shl_res, s_next, next_i,
            ],
        );
        mark_bool_values(&mut func, [cond]);

        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);

        let mask_val = (1i64 << 32) - 1;
        // The derived mask folded to a constant.
        assert_eq!(
            vr.range_of(mask),
            IntRange::point(mask_val),
            "derived mask (1 << 32) - 1 must const-fold to a point range"
        );
        // The masked back-edge value is [0, MASK] under the FULL-phi sweep,
        // independent of the (unreachable) ConstNone edge.
        assert_eq!(
            vr.range_of(s_next),
            IntRange::new(0, mask_val),
            "masked back-edge `(s << 1) & MASK` must be [0, MASK]"
        );
        // The phi narrows to the JOIN despite the vestigial ConstNone back edge.
        assert_eq!(
            vr.range_of(s_phi),
            IntRange::new(0, mask_val),
            "phi must narrow to [0, MASK] — the unreachable ConstNone edge must NOT \
             poison the JOIN"
        );
        assert!(
            vr.fits_inline_int47(s_phi),
            "[0, 2**32-1] fits the inline window"
        );
        // The shift result feeds the raw-i64 seed.
        assert_eq!(vr.range_of(shl_res), IntRange::new(0, mask_val << 1));
        assert!(vr.fits_inline_int47(shl_res));
        use crate::representation_facts::raw_i64_safe_values_for;
        let raw = raw_i64_safe_values_for(&func, &vr);
        assert!(
            raw.contains(&shl_res) && raw.contains(&s_phi),
            "the derived-mask masked accumulator must reach the raw-i64 lane \
             end-to-end (shift result + phi both raw)"
        );
    }
}
