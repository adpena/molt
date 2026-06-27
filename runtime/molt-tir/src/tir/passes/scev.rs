//! Scalar Evolution (SCEV) analysis for TIR — Tier-0 substrate **S6**.
//!
//! A closed-form recurrence representation for SSA values, modeled on LLVM's
//! `ScalarEvolution`. For each value the analysis computes a [`ScevExpr`]
//! describing how the value evolves; for each loop it computes a [`TripCount`].
//!
//! This is the foundation under general bounds-check elimination, induction-
//! variable strength reduction, dynamic-trip unrolling, and the
//! `MaybeBigInt → RawI64Safe` representation promotion. The immediate consumer
//! in this arc is [`super::super::passes::value_range`], which turns affine
//! recurrences and trip counts into integer ranges.
//!
//! ## What an `AddRec` is
//!
//! `AddRec { start, step, loop_header }` denotes the affine recurrence
//! `{start, +, step}` over `loop_header`: the value equals `start` on the first
//! iteration and increases by `step` each subsequent iteration. It is the SCEV
//! form of a canonical induction variable.
//!
//! ## IV detection from the post-`range_devirt` shape
//!
//! After `range_devirt` lowers `for i in range(...)` (and after `iter_devirt`
//! produces the equivalent `while i < len: ...` shape), a canonical integer
//! induction variable manifests as:
//!
//!   * a **loop-header block argument** `iv` (the SSA "phi"), whose incoming
//!     values are a loop-invariant `start` (from the preheader edge) and a
//!     `next` (from each back-edge);
//!   * `next = Add(iv, step)` computed on the back-edge block, where `step` is
//!     loop-invariant.
//!
//! When the back-edge increment carries the `no_signed_wrap` attribute (set by
//! `range_devirt` for unit steps) — or is otherwise proven not to wrap — the
//! recurrence is a sound `AddRec`. Without that proof we must NOT construct an
//! `AddRec`: a wrapping increment is not affine over the integers, and a
//! consumer that assumed monotonicity would miscompile (the loop-IV OOM
//! hazard). See [`SCEV soundness`](#soundness).
//!
//! ## <a name="soundness"></a>Soundness rules (each one prevents a miscompile)
//!
//!   1. **No-wrap requirement for `AddRec`.** A back-edge `Add(iv, step)` only
//!      forms an `AddRec` when it carries `no_signed_wrap`. Otherwise the value
//!      is `Unknown`.
//!   2. **Loop-invariant `step`.** The step must be loop-invariant (defined
//!      outside the loop or a constant). A step that itself varies per-iteration
//!      makes the recurrence non-affine.
//!   3. **Degree-2 recurrence → `Unknown`.** If the step is itself an `AddRec`
//!      (the `total += i` accumulator pattern, whose closed form is quadratic),
//!      we refuse to model it: returning `Unknown` keeps any downstream
//!      range/representation consumer conservative. Promoting such an
//!      accumulator to a bounded raw-i64 carrier is the loop-IV OOM hazard
//!      (`project_loop_iv_osc_15_baton`).
//!   4. **Single back-edge value.** The IV header-arg must receive exactly one
//!      `start` (from non-back-edge predecessors, and they must agree) and the
//!      same `next` from every back-edge. Divergent incoming values → `Unknown`.

use std::collections::{HashMap, HashSet};

use crate::tir::analysis::{Analysis, AnalysisId};
use crate::tir::blocks::{BlockId, LoopRole, Terminator};
use crate::tir::dominators;
use crate::tir::function::TirFunction;
use crate::tir::numeric_facts::{ScevExpr, TripCount, ordered_comparison_trip_count};
use crate::tir::op_kinds_generated::{
    CountedLoopComparisonRole, ScevExprRule, opcode_scev_expr_rule_table,
};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::values::ValueId;

// ---------------------------------------------------------------------------
// SCEV result
// ---------------------------------------------------------------------------

/// Per-function scalar-evolution facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScevResult {
    /// SCEV expression for each value the analysis could classify. A value
    /// absent from the map is treated as `Unknown` by `scev_of`.
    exprs: HashMap<ValueId, ScevExpr>,
    /// Trip count per loop header.
    trip_counts: HashMap<BlockId, TripCount>,
    /// Loop headers, ascending — mirrors the loop forest used to build this.
    headers: Vec<BlockId>,
}

impl ScevResult {
    /// The SCEV expression for `v` (`Unknown` if unclassified).
    pub fn scev_of(&self, v: ValueId) -> ScevExpr {
        self.exprs.get(&v).cloned().unwrap_or(ScevExpr::Unknown)
    }

    /// The trip count of the loop whose header is `header` (`Unknown` if none).
    pub fn trip_count(&self, header: BlockId) -> TripCount {
        self.trip_counts
            .get(&header)
            .cloned()
            .unwrap_or(TripCount::Unknown)
    }

    /// All loop headers (ascending), for iteration by consumers.
    pub fn headers(&self) -> &[BlockId] {
        &self.headers
    }

    /// True if `v` is the canonical induction variable of some loop (its SCEV
    /// is an `AddRec`).
    pub fn is_induction_var(&self, v: ValueId) -> bool {
        self.exprs.get(&v).map(|e| e.is_add_rec()).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Analysis registration (S1 AnalysisManager)
// ---------------------------------------------------------------------------

/// Scalar-evolution analysis marker. Cached by the [`AnalysisManager`].
///
/// CFG-sensitive (loop structure and back-edges define the recurrences) and
/// ops-sensitive (the increment ops and constants feed the recurrence shape).
pub struct ScalarEvolution;

impl Analysis for ScalarEvolution {
    type Result = ScevResult;
    const ID: AnalysisId = AnalysisId::ScalarEvolution;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        compute_scev(func)
    }
}

// ---------------------------------------------------------------------------
// Computation
// ---------------------------------------------------------------------------

/// Loop context: headers + the set of blocks belonging to each natural loop.
/// Computed exactly as `analysis::LoopForest` does (the S1 loop forest) so SCEV
/// shares the one sound definition of a loop body.
struct LoopContext {
    headers: Vec<BlockId>,
    bodies: HashMap<BlockId, HashSet<BlockId>>,
}

/// Build the loop context the same way the S1 `LoopForest` analysis does.
fn build_loop_context(func: &TirFunction) -> LoopContext {
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

    let mut bodies: HashMap<BlockId, HashSet<BlockId>> = HashMap::new();
    if !headers.is_empty() {
        let pred_map = dominators::build_pred_map(func);
        let idoms = dominators::compute_idoms(func, &pred_map);
        for &h in &headers {
            bodies.insert(
                h,
                dominators::collect_loop_blocks(func, &pred_map, &idoms, h),
            );
        }
    }

    LoopContext { headers, bodies }
}

/// Index of where every value is defined and the op (if any) that defines it.
struct DefIndex {
    /// value → defining block.
    def_block: HashMap<ValueId, BlockId>,
    /// value → (opcode, operands, no_signed_wrap) for op-defined values.
    def_op: HashMap<ValueId, (OpCode, Vec<ValueId>, bool)>,
    /// value → constant integer (ConstInt).
    const_int: HashMap<ValueId, i64>,
    /// header → its block-argument value ids (in order).
    header_args: HashMap<BlockId, Vec<ValueId>>,
    /// Transparent-copy resolution: value → the canonical source value reached
    /// by following plain SSA copies (`is_plain_value_copy`). Lowering inserts
    /// many such copies between the IV phi, the guard comparison and the
    /// back-edge increment; resolving through them is what lets the recurrence
    /// recognizer see the canonical induction-variable shape. A plain copy is
    /// *semantically* the identity, so this resolution introduces no
    /// imprecision (it only removes copy noise).
    copy_src: HashMap<ValueId, ValueId>,
}

impl DefIndex {
    /// Follow plain-copy edges to the canonical source of `v`.
    fn resolve(&self, mut v: ValueId) -> ValueId {
        // The copy graph is a DAG (SSA), but guard against pathological cycles
        // with a bounded walk.
        for _ in 0..64 {
            match self.copy_src.get(&v) {
                Some(&src) if src != v => v = src,
                _ => break,
            }
        }
        v
    }
}

fn build_def_index(func: &TirFunction) -> DefIndex {
    let mut def_block = HashMap::new();
    let mut def_op = HashMap::new();
    let mut const_int = HashMap::new();
    let mut header_args: HashMap<BlockId, Vec<ValueId>> = HashMap::new();
    let mut copy_src: HashMap<ValueId, ValueId> = HashMap::new();

    for (&bid, block) in &func.blocks {
        for arg in &block.args {
            def_block.insert(arg.id, bid);
        }
        if matches!(func.loop_roles.get(&bid), Some(LoopRole::LoopHeader)) {
            header_args.insert(bid, block.args.iter().map(|a| a.id).collect());
        }
        for op in &block.ops {
            let nsw = matches!(op.attrs.get("no_signed_wrap"), Some(AttrValue::Bool(true)));
            if op.opcode == OpCode::ConstInt
                && let Some(AttrValue::Int(v)) = op.attrs.get("value")
            {
                for &r in &op.results {
                    const_int.insert(r, *v);
                }
            }
            // Record a transparent-copy edge result → source for a plain value
            // copy (single operand, single result, no semantic attrs).
            if op.is_plain_value_copy() {
                copy_src.insert(op.results[0], op.operands[0]);
            }
            for &r in &op.results {
                def_block.insert(r, bid);
                def_op.insert(r, (op.opcode, op.operands.clone(), nsw));
            }
        }
    }
    // Function parameters are defined at entry.
    for i in 0..func.param_types.len() {
        def_block
            .entry(ValueId(i as u32))
            .or_insert(func.entry_block);
    }

    DefIndex {
        def_block,
        def_op,
        const_int,
        header_args,
        copy_src,
    }
}

/// Incoming edges to a header that pass arguments: for each predecessor block,
/// the argument vector it forwards to `header` and whether it is a back-edge
/// (i.e. the predecessor is inside the loop body).
struct HeaderIncoming {
    /// (predecessor, args, is_back_edge)
    edges: Vec<(BlockId, Vec<ValueId>, bool)>,
}

fn collect_header_incoming(
    func: &TirFunction,
    header: BlockId,
    body: &HashSet<BlockId>,
) -> HeaderIncoming {
    let mut edges = Vec::new();
    for (&bid, block) in &func.blocks {
        let is_back = body.contains(&bid);
        match &block.terminator {
            Terminator::Branch { target, args } if *target == header => {
                edges.push((bid, args.clone(), is_back));
            }
            Terminator::CondBranch {
                then_block,
                then_args,
                else_block,
                else_args,
                ..
            } => {
                if *then_block == header {
                    edges.push((bid, then_args.clone(), is_back));
                }
                if *else_block == header {
                    edges.push((bid, else_args.clone(), is_back));
                }
            }
            Terminator::Switch {
                cases,
                default,
                default_args,
                ..
            } => {
                for (_, tgt, args) in cases {
                    if *tgt == header {
                        edges.push((bid, args.clone(), is_back));
                    }
                }
                if *default == header {
                    edges.push((bid, default_args.clone(), is_back));
                }
            }
            _ => {}
        }
    }
    HeaderIncoming { edges }
}

/// SCEV builder. Owns the def index + loop context and memoizes results to
/// terminate on the recurrence cycle (a header arg's SCEV is computed while
/// computing the back-edge value's SCEV).
struct ScevBuilder<'a> {
    func: &'a TirFunction,
    loops: &'a LoopContext,
    defs: &'a DefIndex,
    /// header → its IV header-arg value (the recurrence "phi"), once recognized.
    iv_of_header: HashMap<BlockId, ValueId>,
    /// Memoized SCEV per value.
    memo: HashMap<ValueId, ScevExpr>,
    /// Values currently on the recursion stack (cycle guard).
    in_progress: HashSet<ValueId>,
}

impl<'a> ScevBuilder<'a> {
    fn new(func: &'a TirFunction, loops: &'a LoopContext, defs: &'a DefIndex) -> Self {
        Self {
            func,
            loops,
            defs,
            iv_of_header: HashMap::new(),
            memo: HashMap::new(),
            in_progress: HashSet::new(),
        }
    }

    /// True if `v` is defined outside the loop headed by `header` (or is a
    /// constant), i.e. loop-invariant with respect to that loop.
    fn is_invariant_in(&self, v: ValueId, header: BlockId) -> bool {
        if self.defs.const_int.contains_key(&v) {
            return true;
        }
        match self.defs.def_block.get(&v) {
            Some(&db) => !self
                .loops
                .bodies
                .get(&header)
                .map(|body| body.contains(&db))
                .unwrap_or(false),
            // No known def site (parameter handled above via def_block) →
            // treat as defined at entry, hence invariant.
            None => true,
        }
    }

    /// Compute the SCEV expression for value `v`.
    fn scev(&mut self, v: ValueId) -> ScevExpr {
        if let Some(e) = self.memo.get(&v) {
            return e.clone();
        }
        if self.in_progress.contains(&v) {
            // On the recurrence cycle without a resolved recurrence yet —
            // conservative.
            return ScevExpr::Unknown;
        }
        self.in_progress.insert(v);
        let result = self.compute_scev_of(v);
        self.in_progress.remove(&v);
        self.memo.insert(v, result.clone());
        result
    }

    fn compute_scev_of(&mut self, v: ValueId) -> ScevExpr {
        if let Some(&c) = self.defs.const_int.get(&v) {
            return ScevExpr::Constant(c);
        }

        // A header block-arg may be an induction-variable phi.
        if let Some(&header) = self.header_of_arg(v) {
            return self.scev_of_header_arg(v, header);
        }

        // Op-defined value: recognize affine combinations of invariants.
        if let Some((opcode, operands, _nsw)) = self.defs.def_op.get(&v).cloned() {
            return self.scev_of_op(v, opcode, &operands);
        }

        // Parameter or otherwise opaque definition → an invariant symbol.
        ScevExpr::Invariant(v)
    }

    /// If `v` is a header block-argument, return that header.
    fn header_of_arg(&self, v: ValueId) -> Option<&BlockId> {
        // header_args is small (one entry per loop header); linear scan is fine.
        self.defs
            .header_args
            .iter()
            .find_map(|(h, args)| if args.contains(&v) { Some(h) } else { None })
    }

    /// Recognize a header block-argument as an induction-variable recurrence,
    /// or classify it as invariant / unknown.
    fn scev_of_header_arg(&mut self, iv: ValueId, header: BlockId) -> ScevExpr {
        if let Some(&known) = self.iv_of_header.get(&header)
            && known == iv
        {
            // Already recognized as this loop's IV (cycle re-entry): return the
            // recurrence shape placeholder. We model the start/step lazily, so
            // re-entry just yields Unknown for the nested computation; the outer
            // call assembles the AddRec. To avoid a partial AddRec here, signal
            // self-reference as Unknown (the caller building the AddRec uses the
            // start/step directly, not this recursive value).
            return ScevExpr::Unknown;
        }

        let body = match self.loops.bodies.get(&header) {
            Some(b) => b.clone(),
            None => return ScevExpr::Invariant(iv),
        };
        let incoming = collect_header_incoming(self.func, header, &body);

        // Partition into entry (non-back-edge) and back-edge args for this iv.
        let arg_index = match self
            .defs
            .header_args
            .get(&header)
            .and_then(|args| args.iter().position(|&a| a == iv))
        {
            Some(i) => i,
            None => return ScevExpr::Invariant(iv),
        };

        let mut start_vals: Vec<ValueId> = Vec::new();
        let mut next_vals: Vec<ValueId> = Vec::new();
        for (_pred, args, is_back) in &incoming.edges {
            let Some(&val) = args.get(arg_index) else {
                // A predecessor that does not pass this arg → malformed for our
                // purposes; refuse to model.
                return ScevExpr::Unknown;
            };
            // Resolve through plain copies so the back-edge/start values name
            // their canonical sources (lowering wraps both in copies).
            let val = self.defs.resolve(val);
            if *is_back {
                next_vals.push(val);
            } else {
                start_vals.push(val);
            }
        }

        // Exactly one distinct start and one distinct back-edge value.
        if start_vals.is_empty() || next_vals.is_empty() {
            return ScevExpr::Invariant(iv);
        }
        let start_val = start_vals[0];
        if start_vals.iter().any(|&s| s != start_val) {
            return ScevExpr::Unknown;
        }
        let next_val = next_vals[0];
        if next_vals.iter().any(|&n| n != next_val) {
            return ScevExpr::Unknown;
        }

        // The back-edge value must be `Add(iv, step)` with `no_signed_wrap`,
        // and `step` loop-invariant. (Subtraction is normalized to Add of a
        // negative const upstream; we additionally accept Add(step, iv).)
        let (opcode, operands, nsw) = match self.defs.def_op.get(&next_val).cloned() {
            Some(t) => t,
            None => return ScevExpr::Invariant(iv),
        };
        if opcode != OpCode::Add || operands.len() != 2 {
            return ScevExpr::Unknown;
        }
        // Soundness rule 1: no AddRec without a non-wrap proof.
        if !nsw {
            return ScevExpr::Unknown;
        }
        // Resolve operands through copies; the Add increments the IV phi via a
        // copy of it (`Add(Copy(iv), step)`).
        let (a, b) = (
            self.defs.resolve(operands[0]),
            self.defs.resolve(operands[1]),
        );
        let iv_resolved = self.defs.resolve(iv);
        let step_val = if a == iv_resolved {
            b
        } else if b == iv_resolved {
            a
        } else {
            // Not a self-increment of this iv.
            return ScevExpr::Unknown;
        };
        // Soundness rule 2: step must be loop-invariant.
        if !self.is_invariant_in(step_val, header) {
            return ScevExpr::Unknown;
        }

        // Mark this header's IV so the recursive `scev(start_val)` /
        // `scev(step_val)` calls (which can transitively reference the header
        // through invariants) terminate cleanly.
        self.iv_of_header.insert(header, iv);

        let start_scev = self.scev_invariant_expr(start_val, header);
        let step_scev = self.scev_invariant_expr(step_val, header);

        // Soundness rule 3: degree-2 recurrence. If the step is itself a
        // recurrence (an AddRec), the closed form is quadratic — refuse.
        if matches!(step_scev, ScevExpr::AddRec { .. }) {
            return ScevExpr::Unknown;
        }
        // The start, if a recurrence of an *outer* loop, is fine (a nested IV);
        // but a start that is an AddRec of THIS loop is impossible (it is
        // defined outside). So only the step's degree gates here.

        ScevExpr::AddRec {
            start: Box::new(start_scev),
            step: Box::new(step_scev),
            loop_header: header,
        }
    }

    /// Compute the SCEV of a value known to be loop-invariant w.r.t. `header`
    /// (start or step of an AddRec). Constants stay constants; everything else
    /// is an `Invariant` symbol unless it is itself an outer-loop recurrence.
    fn scev_invariant_expr(&mut self, v: ValueId, _header: BlockId) -> ScevExpr {
        if let Some(&c) = self.defs.const_int.get(&v) {
            return ScevExpr::Constant(c);
        }
        // It may be an induction variable of an enclosing loop.
        if let Some(&outer_header) = self.header_of_arg(v) {
            let e = self.scev_of_header_arg(v, outer_header);
            // Avoid returning a within-progress Unknown as Invariant noise.
            if !matches!(e, ScevExpr::Unknown) {
                return e;
            }
        }
        ScevExpr::Invariant(v)
    }

    /// SCEV of an op-defined value (affine combinations of invariants only).
    fn scev_of_op(&mut self, _v: ValueId, opcode: OpCode, operands: &[ValueId]) -> ScevExpr {
        match opcode_scev_expr_rule_table(opcode) {
            ScevExprRule::Add if operands.len() == 2 => {
                let a = self.scev(operands[0]);
                let b = self.scev(operands[1]);
                fold_add(a, b)
            }
            ScevExprRule::Sub if operands.len() == 2 => {
                let a = self.scev(operands[0]);
                let b = self.scev(operands[1]);
                fold_sub(a, b)
            }
            ScevExprRule::Mul if operands.len() == 2 => {
                let a = self.scev(operands[0]);
                let b = self.scev(operands[1]);
                fold_mul(a, b)
            }
            _ => ScevExpr::Unknown,
        }
    }
}

/// Constant-fold / build an `Add` SCEV.
fn fold_add(a: ScevExpr, b: ScevExpr) -> ScevExpr {
    match (&a, &b) {
        (ScevExpr::Constant(x), ScevExpr::Constant(y)) => match x.checked_add(*y) {
            Some(s) => ScevExpr::Constant(s),
            None => ScevExpr::Unknown,
        },
        (ScevExpr::Unknown, _) | (_, ScevExpr::Unknown) => ScevExpr::Unknown,
        _ => ScevExpr::Add(Box::new(a), Box::new(b)),
    }
}

/// Constant-fold / build a `Sub` SCEV (represented as `Add(a, -b)` / direct
/// constant fold; non-constant subtraction is `Unknown` since the lattice has
/// no `Sub` node and `Add` of a negated symbol is not expressible safely).
fn fold_sub(a: ScevExpr, b: ScevExpr) -> ScevExpr {
    match (&a, &b) {
        (ScevExpr::Constant(x), ScevExpr::Constant(y)) => match x.checked_sub(*y) {
            Some(s) => ScevExpr::Constant(s),
            None => ScevExpr::Unknown,
        },
        _ => ScevExpr::Unknown,
    }
}

/// Constant-fold / build a `Mul` SCEV.
fn fold_mul(a: ScevExpr, b: ScevExpr) -> ScevExpr {
    match (&a, &b) {
        (ScevExpr::Constant(x), ScevExpr::Constant(y)) => match x.checked_mul(*y) {
            Some(s) => ScevExpr::Constant(s),
            None => ScevExpr::Unknown,
        },
        (ScevExpr::Unknown, _) | (_, ScevExpr::Unknown) => ScevExpr::Unknown,
        _ => ScevExpr::Mul(Box::new(a), Box::new(b)),
    }
}

/// Public entry: compute the scalar-evolution facts for `func`. Shared by the
/// [`ScalarEvolution`] analysis and the value-range analysis (single source of
/// truth — no duplicate recurrence recognizer).
pub fn compute_scev(func: &TirFunction) -> ScevResult {
    let loops = build_loop_context(func);
    if loops.headers.is_empty() {
        // No loops → no recurrences and no trip counts. Still classify
        // constants/invariants so value-range can use them, but that is cheap
        // and done lazily by value-range itself; here we return empty.
        return ScevResult::default();
    }
    let defs = build_def_index(func);
    let mut builder = ScevBuilder::new(func, &loops, &defs);

    // Compute SCEV for the IV header-arg of each loop (the primary recurrences),
    // plus the back-edge increments and any affine derivations the consumer may
    // query. We classify every value reachable as a header arg or op result.
    let mut exprs: HashMap<ValueId, ScevExpr> = HashMap::new();

    // Header args first (recognizes the IVs and populates iv_of_header).
    for &header in &loops.headers {
        if let Some(args) = defs.header_args.get(&header) {
            for &a in args {
                let e = builder.scev(a);
                if !matches!(e, ScevExpr::Unknown) {
                    exprs.insert(a, e);
                }
            }
        }
    }

    // Then every op-defined value, so derived IVs (e.g. `j = i + c`) and
    // affine index expressions get a recurrence too.
    for block in func.blocks.values() {
        for op in &block.ops {
            for &r in &op.results {
                if exprs.contains_key(&r) {
                    continue;
                }
                let e = builder.scev(r);
                if !matches!(e, ScevExpr::Unknown) {
                    exprs.insert(r, e);
                }
            }
        }
    }

    // Trip counts: for a loop whose header tests `Lt(iv, stop)` (positive unit
    // step) or `Gt(iv, stop)` (negative unit step), the trip count is derivable
    // from start, step and stop. We compute a constant trip count when start,
    // step and stop are all constants; otherwise a symbolic bound when sound.
    let mut trip_counts: HashMap<BlockId, TripCount> = HashMap::new();
    let iv_of_header = builder.iv_of_header.clone();
    for &header in &loops.headers {
        let tc = compute_trip_count(func, &defs, &iv_of_header, &mut builder, header);
        trip_counts.insert(header, tc);
    }

    ScevResult {
        exprs,
        trip_counts,
        headers: loops.headers,
    }
}

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
fn compute_trip_count(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{LoopRole, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::numeric_facts::{ScevExpr, TripCount};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::{TirValue, ValueId};

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

    fn const_int(result: ValueId, value: i64) -> TirOp {
        let mut o = op(OpCode::ConstInt, vec![], vec![result]);
        o.attrs.insert("value".into(), AttrValue::Int(value));
        o
    }

    /// Build the canonical post-range_devirt shape for `for i in range(stop)`:
    ///
    /// ```text
    /// entry:  start = const 0; stop = const STOP; br header(start)
    /// header(iv): cond = Lt(iv, stop); condbr cond -> body, exit
    /// body:   next = Add(iv, step=1) [nsw]; br header(next)
    /// exit:   ret
    /// ```
    ///
    /// Returns (func, header, iv, body).
    fn range_loop(stop: i64, nsw: bool) -> (TirFunction, BlockId, ValueId, BlockId) {
        let mut func = TirFunction::new("rl".into(), vec![], TirType::None);
        let start = func.fresh_value();
        let stop_v = func.fresh_value();
        let step = func.fresh_value();
        let iv = func.fresh_value();
        let cond = func.fresh_value();
        let next = func.fresh_value();

        let header = func.fresh_block();
        let body = func.fresh_block();
        let exit = func.fresh_block();

        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![
                const_int(start, 0),
                const_int(stop_v, stop),
                const_int(step, 1),
            ];
            entry.terminator = Terminator::Branch {
                target: header,
                args: vec![start],
            };
        }

        func.blocks.insert(
            header,
            TirBlock {
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

        let add = if nsw {
            op_nsw(OpCode::Add, vec![iv, step], vec![next])
        } else {
            op(OpCode::Add, vec![iv, step], vec![next])
        };
        func.blocks.insert(
            body,
            TirBlock {
                id: body,
                args: vec![],
                ops: vec![add],
                terminator: Terminator::Branch {
                    target: header,
                    args: vec![next],
                },
            },
        );
        func.blocks.insert(
            exit,
            TirBlock {
                id: exit,
                args: vec![],
                ops: vec![],
                terminator: Terminator::Return { values: vec![] },
            },
        );
        func.loop_roles.insert(exit, LoopRole::LoopEnd);

        (func, header, iv, body)
    }

    #[test]
    fn detects_canonical_induction_variable() {
        let (func, header, iv, _body) = range_loop(10, true);
        let scev = compute_scev(&func);
        let e = scev.scev_of(iv);
        match e {
            ScevExpr::AddRec {
                start,
                step,
                loop_header,
            } => {
                assert_eq!(*start, ScevExpr::Constant(0));
                assert_eq!(*step, ScevExpr::Constant(1));
                assert_eq!(loop_header, header);
            }
            other => panic!("expected AddRec, got {other:?}"),
        }
        assert!(scev.is_induction_var(iv));
    }

    #[test]
    fn wrapping_increment_is_not_addrec() {
        // Without no_signed_wrap, we MUST NOT form an AddRec.
        let (func, _header, iv, _body) = range_loop(10, false);
        let scev = compute_scev(&func);
        assert!(
            !scev.is_induction_var(iv),
            "a possibly-wrapping increment must not be an AddRec"
        );
    }

    #[test]
    fn constant_trip_count_for_range() {
        let (func, header, _iv, _body) = range_loop(10, true);
        let scev = compute_scev(&func);
        assert_eq!(scev.trip_count(header), TripCount::Constant(10));
    }

    #[test]
    fn empty_range_trip_count_zero() {
        let (func, header, _iv, _body) = range_loop(0, true);
        let scev = compute_scev(&func);
        assert_eq!(scev.trip_count(header), TripCount::Constant(0));
    }

    #[test]
    fn degree_two_recurrence_is_unknown() {
        // Build `total += i` inside the IV loop: total is a second header-arg
        // whose back-edge value is Add(total, iv) — step (iv) is itself an
        // AddRec → must classify total as Unknown (not an AddRec).
        let (mut func, header, iv, body) = range_loop(10, true);
        let total = func.fresh_value();
        let total_start = func.fresh_value();
        let total_next = func.fresh_value();

        // total_start = const 0 in entry; pass to header as 2nd arg.
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops.push(const_int(total_start, 0));
            if let Terminator::Branch { args, .. } = &mut entry.terminator {
                args.push(total_start);
            }
        }
        // header gets a 2nd block arg `total`.
        func.blocks.get_mut(&header).unwrap().args.push(TirValue {
            id: total,
            ty: TirType::I64,
        });
        // body: total_next = Add(total, iv) [nsw]; pass back as 2nd arg.
        {
            let b = func.blocks.get_mut(&body).unwrap();
            b.ops
                .push(op_nsw(OpCode::Add, vec![total, iv], vec![total_next]));
            if let Terminator::Branch { args, .. } = &mut b.terminator {
                args.push(total_next);
            }
        }

        let scev = compute_scev(&func);
        // iv is still a clean AddRec.
        assert!(scev.is_induction_var(iv));
        // total (degree-2: step is the iv AddRec) must be Unknown.
        assert_eq!(
            scev.scev_of(total),
            ScevExpr::Unknown,
            "accumulator total += i is degree-2 and must be Unknown (loop-IV OOM hazard)"
        );
    }

    #[test]
    fn loopless_function_has_no_scev() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let v = func.fresh_value();
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            entry.ops = vec![const_int(v, 5)];
            entry.terminator = Terminator::Return { values: vec![] };
        }
        let scev = compute_scev(&func);
        assert!(scev.headers().is_empty());
        assert_eq!(scev.trip_count(BlockId(0)), TripCount::Unknown);
    }

    #[test]
    fn non_unit_step_constant_trip_count() {
        // for i in range(0, 10, 2): step 2, trip = 5.
        let (mut func, header, _iv, body) = range_loop(10, true);
        // Rewrite the step const to 2 and re-mark the add nsw (still sound: the
        // guard bounds it; for the unit test we trust the nsw attr).
        // Find the step const op in entry and set it to 2.
        {
            let entry = func.blocks.get_mut(&func.entry_block).unwrap();
            for o in entry.ops.iter_mut() {
                if o.opcode == OpCode::ConstInt && o.attrs.get("value") == Some(&AttrValue::Int(1))
                {
                    o.attrs.insert("value".into(), AttrValue::Int(2));
                }
            }
        }
        let _ = body;
        let scev = compute_scev(&func);
        // ceil((10-0)/2) = 5.
        assert_eq!(scev.trip_count(header), TripCount::Constant(5));
    }
}
