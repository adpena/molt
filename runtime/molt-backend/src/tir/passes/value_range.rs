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

use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

use super::scev::{compute_scev, find_loop_guard, ScevExpr, ScevResult, TripCount};

// ---------------------------------------------------------------------------
// Integer interval
// ---------------------------------------------------------------------------

/// Signed inline-int window low bound: `-2^46`.
pub const INLINE_INT47_LO: i64 = -(1i64 << 46);
/// Signed inline-int window high bound: `2^46 - 1`.
pub const INLINE_INT47_HI: i64 = (1i64 << 46) - 1;

/// A closed integer interval `[lo, hi]` over the i64 domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntRange {
    pub lo: i64,
    pub hi: i64,
}

impl IntRange {
    /// The full i64 range (top of the lattice — "anything").
    pub const FULL_I64: IntRange = IntRange {
        lo: i64::MIN,
        hi: i64::MAX,
    };

    /// A single point `[v, v]`.
    pub fn point(v: i64) -> IntRange {
        IntRange { lo: v, hi: v }
    }

    /// A range `[lo, hi]`, normalized: an inverted/empty input saturates to
    /// `FULL_I64` (we model emptiness conservatively as "unknown", never as a
    /// proof of anything).
    pub fn new(lo: i64, hi: i64) -> IntRange {
        if lo > hi {
            IntRange::FULL_I64
        } else {
            IntRange { lo, hi }
        }
    }

    /// Lattice **join** (union over-approximation): the smallest interval
    /// containing both. Used to merge facts from multiple sources.
    pub fn join(self, other: IntRange) -> IntRange {
        IntRange {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// Lattice **meet** (intersection): the tightest interval implied by both
    /// facts. Used when a guard narrows an already-known range. Returns
    /// `FULL_I64` if the intersection is empty (modeled as "unknown" — never a
    /// false proof).
    pub fn meet(self, other: IntRange) -> IntRange {
        let lo = self.lo.max(other.lo);
        let hi = self.hi.min(other.hi);
        IntRange::new(lo, hi)
    }

    /// Saturating `self + other` in i128, clamped to the i64 domain.
    pub fn add(self, other: IntRange) -> IntRange {
        let lo = (self.lo as i128) + (other.lo as i128);
        let hi = (self.hi as i128) + (other.hi as i128);
        IntRange::from_i128(lo, hi)
    }

    fn from_i128(lo: i128, hi: i128) -> IntRange {
        let clamp = |x: i128| -> i64 {
            if x < i64::MIN as i128 {
                i64::MIN
            } else if x > i64::MAX as i128 {
                i64::MAX
            } else {
                x as i64
            }
        };
        IntRange::new(clamp(lo), clamp(hi))
    }

    /// True if the whole interval is `>= 0`.
    pub fn is_non_negative(self) -> bool {
        self.lo >= 0
    }

    /// True if the whole interval lies within the signed 47-bit inline window.
    pub fn fits_inline_int47(self) -> bool {
        self.lo >= INLINE_INT47_LO && self.hi <= INLINE_INT47_HI
    }
}

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
        self.global_range.get(&v).copied().unwrap_or(IntRange::FULL_I64)
    }

    /// The proven loop-invariant / constant range of `v` (ignoring per-block
    /// guard narrowing). `FULL_I64` if unknown.
    pub fn range_of(&self, v: ValueId) -> IntRange {
        let v = self.resolve(v);
        self.global_range.get(&v).copied().unwrap_or(IntRange::FULL_I64)
    }

    /// CONSERVATIVELY prove `0 <= index < len(container)` for an `Index` /
    /// `StoreIndex` at block `bid`. Returns `true` only when both bounds are
    /// provable; any uncertainty returns `false` (the bounds check stays).
    ///
    /// This is the BCE memory-safety query. A false positive is a silent
    /// out-of-bounds access, so every path that does not *prove* safety must
    /// fall through to `false`.
    pub fn proves_index_in_bounds(
        &self,
        bid: BlockId,
        container: ValueId,
        index: ValueId,
    ) -> bool {
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

    /// CONSERVATIVELY prove `v`'s entire proven range fits the signed 47-bit
    /// inline window. Unknown range ⇒ `false`.
    pub fn fits_inline_int47(&self, v: ValueId) -> bool {
        match self.global_range.get(&self.resolve(v)) {
            Some(r) => r.fits_inline_int47(),
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
        let scev = compute_scev(func);
        compute_value_range(func, &scev)
    }
}

// ---------------------------------------------------------------------------
// Computation
// ---------------------------------------------------------------------------

/// Compute value-range facts from the function + its scalar-evolution facts.
pub fn compute_value_range(func: &TirFunction, scev: &ScevResult) -> ValueRangeResult {
    let mut result = ValueRangeResult::default();

    // ---- transparent-copy map (built first; every fact resolves through it) --
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.is_plain_value_copy() {
                result.copy_src.insert(op.results[0], op.operands[0]);
            }
        }
    }

    // ---- constants + container lengths --------------------------------------
    collect_constants_and_lengths(func, &mut result);

    // ---- loop bodies (for IV-range placement) -------------------------------
    let loop_bodies = build_loop_bodies(func);

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
            let ScevExpr::AddRec { start, step, loop_header } = scev.scev_of(iv) else {
                continue;
            };
            if loop_header != header {
                continue;
            }
            let (Some(s0), Some(k)) = (start.as_constant(), step.as_constant()) else {
                continue;
            };
            // Compute the IV's range over the body from start, step, trip count.
            let iv_range = match iv_range_from_recurrence(s0, k, &scev.trip_count(header)) {
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
        }
    }

    // ---- edge-sensitive guard narrowing -------------------------------------
    // For a header `CondBranch(cond -> then=body, else=exit)` where
    // `cond = Lt(i, n)` / `Le(i, n)`, the body sees `i < n` / `i <= n`.
    narrow_from_header_guards(func, &loop_bodies, &mut result);

    result
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
    // Constant-fold integer `Add`/`Sub`/`Mul` of known constants to a fixpoint,
    // so derived lengths like `n + 1` (the `[True] * (n + 1)` sieve shape) and
    // `len = SameAs(n_plus_1)` resolve to numeric bounds. All arithmetic is
    // checked — an overflow drops the value (left unknown), never wraps.
    let mut changed = true;
    while changed {
        changed = false;
        for block in func.blocks.values() {
            for op in &block.ops {
                if !matches!(op.opcode, OpCode::Add | OpCode::Sub | OpCode::Mul)
                    || op.operands.len() != 2
                {
                    continue;
                }
                let Some(&a) = result.const_int.get(&result.resolve(op.operands[0])) else {
                    continue;
                };
                let Some(&b) = result.const_int.get(&result.resolve(op.operands[1])) else {
                    continue;
                };
                let folded = match op.opcode {
                    OpCode::Add => a.checked_add(b),
                    OpCode::Sub => a.checked_sub(b),
                    OpCode::Mul => a.checked_mul(b),
                    _ => None,
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
            match op.opcode {
                OpCode::BuildList => {
                    let len = op.operands.len() as i64;
                    for &r in &op.results {
                        result
                            .container_length
                            .insert(r, KnownLength::Constant(len));
                    }
                }
                OpCode::BuildTuple => {
                    let len = op.operands.len() as i64;
                    for &r in &op.results {
                        result
                            .container_length
                            .insert(r, KnownLength::Constant(len));
                    }
                }
                OpCode::Mul if op.operands.len() == 2 => {
                    // list-repeat: Mul(list_of_1, count) → length == count.
                    // Resolve operands through copies to reach the BuildList /
                    // const sources.
                    let (a, b) = (result.resolve(op.operands[0]), result.resolve(op.operands[1]));
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
                OpCode::CallBuiltin => {
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
                _ => {}
            }
        }
    }
}

/// Build header → natural-loop body set (the S1 loop forest definition).
fn build_loop_bodies(func: &TirFunction) -> HashMap<BlockId, HashSet<BlockId>> {
    let mut headers: Vec<BlockId> = func
        .loop_roles
        .iter()
        .filter_map(|(bid, role)| {
            if matches!(role, LoopRole::LoopHeader) {
                Some(*bid)
            } else {
                None
            }
        })
        .collect();
    headers.sort_unstable_by_key(|b| b.0);
    let mut bodies = HashMap::new();
    if !headers.is_empty() {
        let pred_map = dominators::build_pred_map(func);
        let idoms = dominators::compute_idoms(func, &pred_map);
        for h in headers {
            bodies.insert(h, dominators::collect_loop_blocks(func, &pred_map, &idoms, h));
        }
    }
    bodies
}

/// The range an induction variable `{s0, +, k}` takes over a loop body.
///
/// The recurrence alone gives a SOUND one-sided monotone bound that holds in
/// the body regardless of trip count:
///   * step `k > 0`: the IV only increases, so `iv >= s0` (range `[s0, MAX]`).
///   * step `k < 0`: the IV only decreases, so `iv <= s0` (range `[MIN, s0]`).
/// A *constant* trip count `t` refines the open side to the IV's last value:
///   * `k > 0`: `[s0, s0 + (t-1)*k]`.
///   * `k < 0`: `[s0 + (t-1)*k, s0]`.
/// The remaining open side (for symbolic/unknown trips) is supplied by the
/// header guard's edge-narrowing (`Lt`/`Le`). All arithmetic is i128,
/// saturating to i64 — never wrapping.
///
/// Returns `None` only for `k == 0` handled inline below as a point.
fn iv_range_from_recurrence(s0: i64, k: i64, trip: &TripCount) -> Option<IntRange> {
    if k == 0 {
        return Some(IntRange::point(s0));
    }
    // Monotone one-sided bound from the recurrence (always sound in the body).
    let mono = if k > 0 {
        IntRange::new(s0, i64::MAX)
    } else {
        IntRange::new(i64::MIN, s0)
    };

    if let TripCount::Constant(t) = trip {
        if *t <= 0 {
            // Loop never executes; the (dead) body IV is bounded by s0.
            return Some(IntRange::point(s0));
        }
        let last = (s0 as i128) + ((*t as i128) - 1) * (k as i128);
        let (lo, hi) = if k > 0 {
            (s0 as i128, last)
        } else {
            (last, s0 as i128)
        };
        // meet the closed-form with the monotone bound (the closed form is
        // tighter or equal; meet guards against any saturation surprise).
        return Some(IntRange::from_i128(lo, hi).meet(mono));
    }

    // Symbolic / unknown trip: rely on the monotone bound; guard narrowing adds
    // the other side. (`index >= 0` is provable directly from `s0 >= 0`.)
    Some(mono)
}

/// Narrow body ranges from the loop's exit-test guard `Lt(i, n)` / `Le(i, n)`
/// and record symbolic `i < len(c)` facts for the symbolic bound proof.
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
        if !(then_in && !else_in) {
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
        for &b in body {
            match opcode {
                OpCode::Lt => {
                    if let Some(n) = bound_const {
                        let narrow = IntRange::new(i64::MIN, n.saturating_sub(1));
                        narrow_block(result, b, var, narrow);
                    }
                    // Symbolic `var < bound` regardless of constancy.
                    result.record_symbolic_lt(b, var, bound);
                }
                OpCode::Le => {
                    if let Some(n) = bound_const {
                        let narrow = IntRange::new(i64::MIN, n);
                        narrow_block(result, b, var, narrow);
                    }
                    // Le(var, n) ⇒ var < n+1; the symbolic-len path is Lt-only
                    // (the numeric path covers the constant n+1 length case).
                }
                _ => {}
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
    use crate::tir::blocks::TirBlock;

    #[test]
    fn int_range_join_meet() {
        let a = IntRange::new(0, 10);
        let b = IntRange::new(5, 20);
        assert_eq!(a.join(b), IntRange::new(0, 20));
        assert_eq!(a.meet(b), IntRange::new(5, 10));
        // Disjoint meet → unknown (FULL), never a false tight range.
        assert_eq!(IntRange::new(0, 1).meet(IntRange::new(5, 6)), IntRange::FULL_I64);
    }

    #[test]
    fn int_range_add_saturates() {
        let big = IntRange::point(i64::MAX);
        let r = big.add(IntRange::point(1));
        assert_eq!(r.hi, i64::MAX, "overflow must saturate, not wrap");
    }

    #[test]
    fn fits_inline_int47_boundary() {
        assert!(IntRange::new(INLINE_INT47_LO, INLINE_INT47_HI).fits_inline_int47());
        assert!(!IntRange::new(0, INLINE_INT47_HI + 1).fits_inline_int47());
        assert!(!IntRange::new(INLINE_INT47_LO - 1, 0).fits_inline_int47());
        assert!(!IntRange::FULL_I64.fits_inline_int47());
    }

    #[test]
    fn iv_range_positive_step() {
        // for i in range(10): i in [0, 9].
        let r = iv_range_from_recurrence(0, 1, &TripCount::Constant(10)).unwrap();
        assert_eq!(r, IntRange::new(0, 9));
    }

    #[test]
    fn iv_range_step_two() {
        // for i in range(0, 10, 2): values 0,2,4,6,8 → [0, 8], trip 5.
        let r = iv_range_from_recurrence(0, 2, &TripCount::Constant(5)).unwrap();
        assert_eq!(r, IntRange::new(0, 8));
    }

    #[test]
    fn iv_range_negative_step() {
        // for i in range(10, 0, -1): values 10,9,...,1 → [1, 10], trip 10.
        let r = iv_range_from_recurrence(10, -1, &TripCount::Constant(10)).unwrap();
        assert_eq!(r, IntRange::new(1, 10));
    }

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
        o.attrs.insert("no_signed_wrap".into(), AttrValue::Bool(true));
        o
    }
    fn cint(result: ValueId, value: i64) -> TirOp {
        let mut o = op(OpCode::ConstInt, vec![], vec![result]);
        o.attrs.insert("value".into(), AttrValue::Int(value));
        o
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
                args: vec![TirValue { id: iv, ty: TirType::I64 }],
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
    fn e2e_wrapping_increment_not_in_bounds() {
        // Same shape but WITHOUT no_signed_wrap on the increment: SCEV refuses
        // the AddRec, so the IV has no proven range and BCE must NOT fire.
        let (mut func, body, a, iv) = range_loop_vr(10, 10);
        // Strip nsw from the body's Add.
        for op in func.blocks.get_mut(&body).unwrap().ops.iter_mut() {
            if op.opcode == OpCode::Add {
                op.attrs.remove("no_signed_wrap");
            }
        }
        let scev = compute_scev(&func);
        let vr = compute_value_range(&func, &scev);
        assert!(
            !vr.proves_index_in_bounds(body, a, iv),
            "a possibly-wrapping IV must not yield a bounds proof"
        );
    }
}
