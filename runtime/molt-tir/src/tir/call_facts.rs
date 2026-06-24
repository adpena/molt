//! **CallFacts** — the per-call-site fact record (foundation design 47).
//!
//! `tools/call_fact_coverage.py` measured that only **2 of 7** call-site facts a
//! world-class compiler records are actually *attached* to the call op
//! (`direct_target` as an `s_value` attr string and `typed_return` as the result
//! `Repr`). The other five — `leaf`, `no_throw`, `no_alloc`, `inlinable`,
//! `noescape_args` — are *computed inside a pass and discarded*, so no backend can
//! consume them and no tool can measure their site-level coverage. That missing
//! representation is the perf root: a large share of molt's runtime-helper traffic
//! exists because the compiler **cannot carry the proof needed to remove it**.
//!
//! This module is the IR primitive that stops the discarding. A [`CallFacts`]
//! record is attached to every call-bearing op (`Call` / `CallMethod` /
//! `CallBuiltin`), keyed by the op's result [`ValueId`] in a per-function
//! [`CallFactsTable`]. Each field is a [`FactValue`] — a confidence lattice, not a
//! bare bool — so the compiler distinguishes *proven* from *unknown* and **never
//! silently assumes** (doc 47 §1, §7).
//!
//! ## Phase 1a scope (this module)
//!
//! Phase 1a is **pure representation**: it *attaches* what the existing analyses
//! (`call_graph`, `ip_summary`, `inliner`, `effects`/`op_kinds`) already compute,
//! it is **consumed by nothing on the hot compile path**, and it is therefore
//! byte-identical (additive). The fields filled:
//!
//! | field | source | lattice rule (Phase 1) |
//! | --- | --- | --- |
//! | [`CallFacts::target`] | `call_graph::classify_call_op` | typed [`CallTargetFact`] — `StaticDirect{name}` iff the `Call`'s `s_value` names a module-defined function, else `Opaque` (the #71 / #59 class: a typed variant, never raw marker bits) |
//! | [`CallFacts::typed_return`] | the result `ValueId`'s `TirType` | `Some(repr)` when the type is precise (non-`DynBox`), `None` when `DynBox` |
//! | [`CallFacts::leaf`] | `CallGraph::makes_any_call` | `Proven` iff the resolved callee makes no call of any kind; `False` iff it provably does; `Unknown` for an unresolved (opaque) target |
//! | [`CallFacts::no_throw`] | `op_kinds` `may_throw` + callee handlers + builtin allowlist | `Proven` iff the opcode is statically no-throw **or** the resolved callee has no exception handlers **or** it is a no-throw-allowlisted builtin; else `Unknown` |
//! | [`CallFacts::inlinable`] | `inliner::classify_inline_eligibility` | the typed [`InlineEligibility`] (Eligible \| WhyNot(reason)) — the SAME value `inliner::is_inlineable` derives its bool from (single source of truth, doc 47 §7) |
//!
//! `no_alloc` and `no_escape_args` are **Phase 2** (escape-analysis-sourced) and
//! are deliberately *not* fabricated here — an unsound `Proven` is a miscompile,
//! whereas their absence is only a missed optimization. They are tracked as
//! `Unknown` in the lattice until Phase 2 fills them (doc 47 §5).
//!
//! ## Why this is an interprocedural (module-phase) analysis
//!
//! `leaf`, `inlinable`, the `StaticDirect`/`Opaque` classification, and the
//! callee-has-no-handlers half of `no_throw` are **callee-side facts**: they need
//! the whole-program [`CallGraph`], the bottom-up [`ModuleSummaries`], and the
//! callee bodies — exactly the inputs [`ModuleSummaries::compute`] and
//! [`is_inlineable`](super::passes::inliner::is_inlineable) already consume. So the
//! precise table is built by [`CallFactsTable::build_module`] in the module phase
//! (alongside the call graph + summaries it reads), NOT by the strictly
//! *intraprocedural* [`Analysis`] trait (whose `compute(&TirFunction)` cannot see
//! the module). Forcing the interprocedural computation through the
//! intraprocedural trait would require smuggling module context through a side
//! channel — a workaround this codebase forbids.
//!
//! The [`Analysis`] trait IS still implemented ([`CallFactsAnalysis`], keyed by
//! [`AnalysisId::CallFacts`]) so the per-function manager can **cache** a table
//! and so the FactGraph / coverage contract has a stable cache key (doc 47 §1).
//! Its `compute(func)` produces the **fail-closed intraprocedural floor**: every
//! callee-side fact that cannot be proven from `func` alone is `Unknown`, and only
//! the purely-local facts (`typed_return`; `no_throw` via a statically-no-throw
//! opcode or a no-throw builtin) are `Proven`. This floor is *sound by
//! construction* — it can only ever say `Unknown` where the precise module-phase
//! table would say `Proven`, never the reverse — so a cache miss can never yield a
//! wrong `Proven`. The module phase seeds the precise table via
//! [`AnalysisManager::prepopulate`](super::analysis::AnalysisManager::prepopulate).
//!
//! ## Invariants (doc 47 §7)
//!
//! * `Unknown` is the **fail-closed default**: every consumer treats it as the
//!   pessimistic answer. A wrong `Proven` is a miscompile; a conservative
//!   `Unknown` is only a missed opt. The two producers (floor + precise) obey a
//!   *monotone* relationship — the floor never out-claims the precise table.
//! * [`CallFacts::target`] is the typed [`CallTargetFact`] — no raw marker bits
//!   ever cross into a fast path (the #59 IC-marker class).
//! * `inlinable` reads [`is_inlineable`](super::passes::inliner::is_inlineable)'s
//!   own decision function, so the side-table can never disagree with the inliner
//!   (no second source of truth).
//! * The table is keyed by the call op's result `ValueId`; it is CFG- and
//!   ops-sensitive (a removed block / rewritten op can delete a call), so it
//!   invalidates with the same events as `DefMap` (see [`CallFactsAnalysis`]).

use std::collections::BTreeMap;

use super::analysis::{Analysis, AnalysisId};
use super::call_graph::CallGraph;
use super::function::{TirFunction, TirModule};
use super::ops::{AttrValue, OpCode, TirOp};
use super::passes::inliner::classify_inline_eligibility;
use super::passes::ip_summary::ModuleSummaries;
use super::target_info::TargetInfo;
use super::types::TirType;
use super::values::ValueId;
use crate::representation_plan::Repr;

// ───────────────────────────────────────────────────────────────────────────
// The confidence lattice (doc 47 §1)
// ───────────────────────────────────────────────────────────────────────────

/// Identifier for a runtime guard that conditions a [`FactValue::Guarded`] fact
/// (e.g. a class-version or type guard). Phase 3 populates these; Phase 1 never
/// emits `Guarded`, but the lattice variant exists so the representation is
/// complete and consumers fail-closed on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GuardId(pub u32);

/// Observed-confidence weight for a [`FactValue::Profiled`] fact (0–255, scaled
/// from a profile observation). Phase 1 never emits `Profiled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Confidence(pub u8);

/// Confidence lattice for one call-site fact.
///
/// [`FactValue::Unknown`] is the **fail-closed default**, treated as the
/// pessimistic answer by every consumer; [`FactValue::False`] is a proven
/// negative (also pessimistic, but cacheable / actionable). `Guarded` / `Profiled`
/// carry their guard / evidence and are reserved for Phase 3 — Phase 1 producers
/// only ever emit `Proven`, `Unknown`, or `False`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactValue {
    /// Statically established — no runtime check required.
    Proven,
    /// True under the named runtime guard (class-version, type, …). Phase 3.
    Guarded(GuardId),
    /// Observed at the given confidence; needs a guard to exploit soundly.
    /// Phase 3.
    Profiled(Confidence),
    /// Fail-closed default — assume the hazard holds.
    Unknown,
    /// Proven NOT to hold.
    False,
}

impl FactValue {
    /// True iff this is the statically-proven rung. The conservative query every
    /// consumer uses: only `Proven` may drive an unconditional fast path; every
    /// other rung (including `Unknown`) is pessimistic in Phase 1.
    #[inline]
    pub fn is_proven(self) -> bool {
        matches!(self, FactValue::Proven)
    }

    /// Encode a definitely-true / definitely-false static fact onto the lattice:
    /// `true → Proven`, `false → False`. Used where a producer has a *decided*
    /// boolean (e.g. "the resolved callee makes a call" → `leaf = False`). A
    /// producer that merely *lacks* a proof must emit [`FactValue::Unknown`]
    /// explicitly, NOT `False` — the two are different claims.
    #[inline]
    pub fn from_decided(b: bool) -> FactValue {
        if b {
            FactValue::Proven
        } else {
            FactValue::False
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Typed call target (the #71 / #59 seed)
// ───────────────────────────────────────────────────────────────────────────

/// The resolved target of a call site, as a **typed variant** — never a decoded
/// raw `u64` marker. This is the Phase-1 seed of the #71 `CallableTarget`
/// (`DirectCodePtr | RuntimeMarker | Closure | BoundMethod | MethodDescriptor |
/// Deopt`): by making the target a typed enum, the IC-marker SIGSEGV (#59) class
/// — a raw marker bit misread on a fast path — becomes *unexpressible*.
///
/// Phase 1 distinguishes the two cases the call graph already proves; the richer
/// variants (`Closure`, `BoundMethod`, `MethodDescriptor`, guarded/deopt targets)
/// are added by Phase 3 when the devirt / guard machinery lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallTargetFact {
    /// A statically-resolved direct call to a function defined in this module.
    /// The inliner / IPSCCP / direct-branch lowering act on exactly these.
    StaticDirect {
        /// The module-defined callee name (the same key the call graph uses).
        callee: String,
    },
    /// The concrete target is not statically known to this module: an indirect /
    /// computed callee, a dynamic method dispatch, an extern / cross-batch
    /// callee, or a runtime-helper call. Conservatively treated as reaching any
    /// function (recursion-capable).
    Opaque,
}

impl CallTargetFact {
    /// The resolved callee name iff this is a [`CallTargetFact::StaticDirect`].
    #[inline]
    pub fn static_callee(&self) -> Option<&str> {
        match self {
            CallTargetFact::StaticDirect { callee } => Some(callee.as_str()),
            CallTargetFact::Opaque => None,
        }
    }

    /// True iff the target is statically resolved (devirtualized).
    #[inline]
    pub fn is_static_direct(&self) -> bool {
        matches!(self, CallTargetFact::StaticDirect { .. })
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Inline eligibility (the single source of truth shared with the inliner)
// ───────────────────────────────────────────────────────────────────────────

/// Why a callee is NOT eligible to inline — the typed reason the inliner's
/// exclusion gates produce. Recorded so the inliner stops *recomputing* it and so
/// the coverage tool can report *why* a call stayed generic
/// (`generic_call_fallback_by_reason`, doc 47 §6). The ordering matches the
/// gate-evaluation order in
/// [`classify_inline_eligibility`](super::passes::inliner::classify_inline_eligibility)
/// (the first failing gate wins), so the reason is deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineWhyNot {
    /// A member of the call graph's recursive set (a recursion cycle, a
    /// self-edge, or a function with an opaque call). Inlining is unbounded.
    Recursive,
    /// The callee has a true exception **handler** region (`try`/`except` or a
    /// generator/async state region) — the splice does not remap handler labels.
    HasHandlers,
    /// The callee contains a generator/async state-machine opcode; its body
    /// cannot be linearly spliced without reconstructing the suspension
    /// machinery.
    Generator,
    /// The callee's entry block is itself a branch target (has predecessors), so
    /// the direct param→argument binding splice (which clones the entry as an
    /// argument-less block) would not be SSA-valid.
    EntryHasPredecessor,
    /// The callee is a closure (its first param is the implicit captured-env
    /// param), which the direct param→operand splice would miscompile.
    Closure,
    /// The callee's op count exceeds the cost model's inline budget for it.
    OverBudget,
}

/// Whether a callee may be inlined, and if not, the typed reason. This is the
/// value [`is_inlineable`](super::passes::inliner::is_inlineable) reduces to a
/// bool (`is_inlineable == classify_inline_eligibility(...).is_eligible()`), so a
/// [`CallFacts`] record carrying this can never disagree with the inliner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineEligibility {
    /// The callee passes every correctness gate AND the cost-model budget.
    Eligible,
    /// The callee is excluded for the given reason (the first failing gate).
    WhyNot(InlineWhyNot),
    /// The target is not a statically-resolved module-defined callee, so inline
    /// eligibility is not even a question this analysis can answer (e.g. an
    /// opaque / dynamic / builtin call). Distinct from `WhyNot`: there is no
    /// callee body to evaluate gates against. Fail-closed (never inlined).
    Unknown,
}

impl InlineEligibility {
    /// True iff the callee is eligible to inline. The exact predicate
    /// [`is_inlineable`](super::passes::inliner::is_inlineable) returns.
    #[inline]
    pub fn is_eligible(self) -> bool {
        matches!(self, InlineEligibility::Eligible)
    }

    /// The typed why-not reason, if excluded for a concrete gate.
    #[inline]
    pub fn why_not(self) -> Option<InlineWhyNot> {
        match self {
            InlineEligibility::WhyNot(r) => Some(r),
            InlineEligibility::Eligible | InlineEligibility::Unknown => None,
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The CallFacts record + per-function table
// ───────────────────────────────────────────────────────────────────────────

/// The fact record attached to one call-bearing op, keyed by its result
/// [`ValueId`] in a [`CallFactsTable`]. Phase 1 fills `target`, `typed_return`,
/// `leaf`, `no_throw`, `inlinable`; `no_alloc` / `no_escape_args` are Phase 2 and
/// stay `Unknown` until then.
#[derive(Debug, Clone, PartialEq)]
pub struct CallFacts {
    /// The typed call target (#71 seed). Never a raw marker bit.
    pub target: CallTargetFact,
    /// The result `Repr` when precise, else `None` (= `DynBox`, the boxed
    /// universal carrier). Lets a consumer keep a typed return raw across the
    /// call (doc 47 §3, generator-fusion / typed-return).
    pub typed_return: Option<Repr>,
    /// The callee makes no further call of any kind (frame-elision / no-spill
    /// fast path). `Unknown` for an unresolved target (fail-closed).
    pub leaf: FactValue,
    /// The call provably cannot raise on this edge (the AOT analogue of
    /// CPython-3.11 zero-cost exceptions). Phase-1 skeleton: opcode statically
    /// no-throw ∨ callee has no handlers ∨ no-throw-allowlisted builtin.
    pub no_throw: FactValue,
    /// The call performs no heap allocation (alloc-free call fast path).
    /// **Phase 2** (escape-analysis-sourced) — `Unknown` in Phase 1.
    pub no_alloc: FactValue,
    /// Inline eligibility + the typed why-not reason. The same value the inliner
    /// decides on (single source of truth).
    pub inlinable: InlineEligibility,
}

impl CallFacts {
    /// The fully fail-closed record: every fact `Unknown`, target `Opaque`,
    /// inline eligibility `Unknown`, return un-typed. The conservative baseline a
    /// producer *upgrades* from — it is never a miscompile to leave a field here.
    pub fn unknown() -> CallFacts {
        CallFacts {
            target: CallTargetFact::Opaque,
            typed_return: None,
            leaf: FactValue::Unknown,
            no_throw: FactValue::Unknown,
            no_alloc: FactValue::Unknown,
            inlinable: InlineEligibility::Unknown,
        }
    }
}

/// Per-function side-table of [`CallFacts`], keyed by each call op's **result
/// `ValueId`**. A `BTreeMap` for deterministic iteration (the coverage tool and
/// any dump walk it in a stable order). Built precisely by
/// [`CallFactsTable::build_module`] (interprocedural) or as a fail-closed floor by
/// [`CallFactsTable::build_local`] (intraprocedural).
///
/// Keyed internally by the call op's result [`ValueId`]'s raw index (`ValueId.0`,
/// a `u32`) rather than by `ValueId` itself: `ValueId` derives only `Hash`/`Eq`
/// (the codebase keys it with `HashMap` everywhere — `DefMap`, `value_types`),
/// not `Ord`, so a `BTreeMap<ValueId, _>` would not compile and adding `Ord` to a
/// shared core type for one side-table's iteration order would be a wrong-layer
/// change. The `u32` index *is* the canonical SSA-definition order, so a
/// `BTreeMap<u32, _>` gives the promised deterministic walk while the public API
/// stays `ValueId`-typed (the `.0` translation lives only at this boundary).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallFactsTable {
    facts: BTreeMap<u32, CallFacts>,
}

impl CallFactsTable {
    /// The facts for the call op that produced `result`, if recorded.
    #[inline]
    pub fn get(&self, result: ValueId) -> Option<&CallFacts> {
        self.facts.get(&result.0)
    }

    /// Number of call sites with a recorded fact record.
    #[inline]
    pub fn len(&self) -> usize {
        self.facts.len()
    }

    /// True if no call sites are recorded.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.facts.is_empty()
    }

    /// Iterate `(result ValueId, &CallFacts)` in deterministic `ValueId` order
    /// (ascending raw SSA index — the `BTreeMap<u32, _>` key order).
    pub fn iter(&self) -> impl Iterator<Item = (ValueId, &CallFacts)> {
        self.facts.iter().map(|(&v, f)| (ValueId(v), f))
    }

    /// Build the **precise** per-function table for every function in `module`,
    /// using the whole-program interprocedural context. Returns one table per
    /// function, keyed by function name — the module phase prepopulates each into
    /// its function's [`AnalysisManager`](super::analysis::AnalysisManager).
    ///
    /// `call_graph` + `summaries` + `tti` are the same instances
    /// [`run_module_pipeline`](super::module_phase::run_module_pipeline) already
    /// builds; this analysis reads them, it never mutates a body.
    pub fn build_module(
        module: &TirModule,
        call_graph: &CallGraph,
        summaries: &ModuleSummaries,
        tti: &TargetInfo,
    ) -> BTreeMap<String, CallFactsTable> {
        // Function bodies by name, for the callee-side fact lookups (no_throw via
        // has-handlers, inline eligibility). O(1) per query.
        let by_name: BTreeMap<&str, &TirFunction> = module
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f))
            .collect();

        let mut out: BTreeMap<String, CallFactsTable> = BTreeMap::new();
        for func in &module.functions {
            let mut table = CallFactsTable::default();
            for block in func.blocks.values() {
                for op in &block.ops {
                    let Some(result) = call_op_result(op) else {
                        continue;
                    };
                    let facts =
                        analyze_call_site_module(op, func, call_graph, summaries, tti, &by_name);
                    table.facts.insert(result.0, facts);
                }
            }
            out.insert(func.name.clone(), table);
        }
        out
    }

    /// Build the **fail-closed intraprocedural floor** for `func` alone (no module
    /// context). Every callee-side fact that cannot be proven from `func` is
    /// `Unknown`; only purely-local facts (`typed_return`; `no_throw` via a
    /// statically-no-throw opcode or a no-throw builtin) are proven. This is the
    /// [`Analysis::compute`] path — sound by construction, never out-claiming the
    /// precise [`Self::build_module`] table.
    pub fn build_local(func: &TirFunction) -> CallFactsTable {
        let mut table = CallFactsTable::default();
        for block in func.blocks.values() {
            for op in &block.ops {
                let Some(result) = call_op_result(op) else {
                    continue;
                };
                table
                    .facts
                    .insert(result.0, analyze_call_site_local(op, func));
            }
        }
        table
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Per-call-site analysis
// ───────────────────────────────────────────────────────────────────────────

/// The result `ValueId` of a call-bearing op (`Call` / `CallMethod` /
/// `CallBuiltin`), if it is a call op that produces a value. Returns `None` for
/// non-call ops and for a (rare) result-less call. The key the side-table uses.
fn call_op_result(op: &TirOp) -> Option<ValueId> {
    if !is_call_op(op.opcode) {
        return None;
    }
    op.results.first().copied()
}

/// Whether `opcode` is one of the three call-bearing opcodes CallFacts records.
#[inline]
fn is_call_op(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Call | OpCode::CallMethod | OpCode::CallBuiltin
    )
}

/// Read an op's `s_value` string attr (the `Call` callee name), if present.
fn s_value(op: &TirOp) -> Option<&str> {
    match op.attrs.get("s_value") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// Read a `CallBuiltin`'s builtin name. The SSA lift stores it under the `name`
/// attr key (not `s_value`); `range_new` is normalized to `name = "range"`.
fn builtin_name(op: &TirOp) -> Option<&str> {
    match op.attrs.get("name") {
        Some(AttrValue::Str(s)) => Some(s.as_str()),
        _ => None,
    }
}

/// The typed return `Repr` for a call op's result, derived from the result
/// `ValueId`'s `TirType` in `func.value_types`. `Some(repr)` when the type is
/// precise (non-`DynBox`); `None` when `DynBox` (the boxed universal carrier) or
/// when the type is unknown. The lattice floor [`Repr::default_for`] maps a
/// `TirType` to its conservative carrier — Phase 1 reports that floor (e.g.
/// `I64 → MaybeBigInt`); the value-range / unboxing passes raise it later, and a
/// future coverage join over `typed_repr_report` reads the *post-pass* repr.
fn typed_return_for(result: ValueId, func: &TirFunction) -> Option<Repr> {
    match func.value_types.get(&result) {
        Some(TirType::DynBox) | None => None,
        Some(ty) => Some(Repr::default_for(ty)),
    }
}

/// Builtins that provably cannot raise for *any* arguments — the Phase-1 no-throw
/// allowlist (doc 47 §2). Conservative: only pure, total builtins whose molt
/// runtime implementation has no error path for valid (already type-checked)
/// operands. A builtin not on this list is `Unknown` (fail-closed), never
/// asserted no-throw. `len` is intentionally EXCLUDED — it dispatches `__len__`,
/// which can raise.
fn builtin_is_no_throw(name: &str) -> bool {
    matches!(
        name,
        // Identity / introspection on an already-realized object: no dispatch,
        // no allocation failure path that surfaces as a Python exception.
        "id" | "type" | "is" | "isinstance_fast"
    )
}

/// The typed call target for a `Call` op, resolved against the module's defined
/// function set. `StaticDirect` iff the `Call`'s `s_value` names a defined,
/// non-gpu-runtime function; else `Opaque`. `CallMethod` (dynamic dispatch) and
/// `CallBuiltin` (runtime helper) are always `Opaque`. This mirrors
/// `call_graph::classify_call_op` exactly — same `s_value`/defined predicate, same
/// gpu-runtime carve-out — but returns the *typed* fact rather than a `CallEdge`.
fn target_for_module(op: &TirOp, call_graph: &CallGraph) -> CallTargetFact {
    match op.opcode {
        OpCode::Call => match s_value(op) {
            // A gpu_* runtime symbol lifts to `Call` but is a runtime helper, not
            // a user function — the call graph excludes it as an edge, so it is
            // not a static-direct user target here either.
            Some(name) if is_gpu_runtime_symbol(name) => CallTargetFact::Opaque,
            Some(name) if call_graph.is_defined(name) => CallTargetFact::StaticDirect {
                callee: name.to_string(),
            },
            _ => CallTargetFact::Opaque,
        },
        // Method dispatch is always dynamic; a builtin is always a runtime helper.
        OpCode::CallMethod | OpCode::CallBuiltin => CallTargetFact::Opaque,
        _ => CallTargetFact::Opaque,
    }
}

/// The fixed gpu_* runtime-intrinsic `s_value` symbols that lift to `Call` but are
/// runtime-helper calls, never user functions. Mirrors `call_graph`'s and the
/// inliner's identical carve-out (one predicate, three readers).
fn is_gpu_runtime_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "molt_gpu_thread_id"
            | "molt_gpu_block_id"
            | "molt_gpu_block_dim"
            | "molt_gpu_grid_dim"
            | "molt_gpu_barrier"
    )
}

/// Compute the precise [`CallFacts`] for one call op, using the whole-program
/// context. The interprocedural path.
fn analyze_call_site_module(
    op: &TirOp,
    func: &TirFunction,
    call_graph: &CallGraph,
    summaries: &ModuleSummaries,
    tti: &TargetInfo,
    by_name: &BTreeMap<&str, &TirFunction>,
) -> CallFacts {
    let result = op
        .results
        .first()
        .copied()
        .expect("analyze_call_site_module called on a result-less op");

    let target = target_for_module(op, call_graph);
    let typed_return = typed_return_for(result, func);

    // leaf / inlinable / callee-handler no_throw are callee-side: resolved only
    // for a StaticDirect target whose body is in this module.
    let resolved_callee: Option<&TirFunction> = target
        .static_callee()
        .and_then(|name| by_name.get(name).copied());

    // leaf: the resolved callee makes no call of any kind. `Proven` iff it is a
    // leaf, `False` iff it provably makes a call, `Unknown` if unresolved.
    let leaf = match target.static_callee() {
        Some(callee) => FactValue::from_decided(!call_graph.makes_any_call(callee)),
        None => FactValue::Unknown,
    };

    // no_throw: opcode statically no-throw ∨ resolved callee has no handlers ∨
    // a no-throw-allowlisted builtin. Else Unknown (fail-closed).
    let no_throw = no_throw_for(op, resolved_callee);

    // inlinable: the inliner's own decision (single source of truth). Only a
    // StaticDirect, module-resident callee is even a candidate; everything else
    // is `Unknown` (no body to gate against).
    let inlinable = match resolved_callee {
        Some(callee) => classify_inline_eligibility(callee, call_graph, summaries, tti),
        None => InlineEligibility::Unknown,
    };

    CallFacts {
        target,
        typed_return,
        leaf,
        no_throw,
        // Phase 2 (escape analysis). Fail-closed until then.
        no_alloc: FactValue::Unknown,
        inlinable,
    }
}

/// Compute the fail-closed intraprocedural floor [`CallFacts`] for one call op
/// (no module context). The [`Analysis::compute`] path.
fn analyze_call_site_local(op: &TirOp, func: &TirFunction) -> CallFacts {
    let result = op
        .results
        .first()
        .copied()
        .expect("analyze_call_site_local called on a result-less op");

    // Without `defined`, a named `Call` target cannot be confirmed module-local,
    // so the target floors to `Opaque` (fail-closed: never claim StaticDirect we
    // cannot prove).
    let typed_return = typed_return_for(result, func);

    // no_throw: only the *locally* decidable halves — a statically-no-throw
    // opcode (none of the call opcodes are, but a future opcode might be) or a
    // no-throw builtin. The callee-has-no-handlers half needs the body, so it is
    // omitted here (yields `Unknown`, not a false claim).
    let no_throw = no_throw_for(op, None);

    CallFacts {
        target: CallTargetFact::Opaque,
        typed_return,
        leaf: FactValue::Unknown,
        no_throw,
        no_alloc: FactValue::Unknown,
        inlinable: InlineEligibility::Unknown,
    }
}

/// The Phase-1 `no_throw` skeleton (doc 47 §2). `Proven` iff:
///   1. the opcode is statically no-throw (per the generated `op_kinds` registry —
///      the authoritative effect oracle, read never re-decided, doc 47 §7), OR
///   2. `resolved_callee` is `Some` and has no exception **handler** region
///      (`TirFunction::has_exception_handlers` — a callee that cannot itself
///      enter a handler cannot raise *through* one on this edge), OR
///   3. the op is a `CallBuiltin` whose builtin is on the no-throw allowlist.
///
/// Otherwise `Unknown` (fail-closed).
///
/// `resolved_callee` is `None` on the intraprocedural floor and for opaque/builtin
/// targets, so case 2 only fires when the precise module path resolved a body.
fn no_throw_for(op: &TirOp, resolved_callee: Option<&TirFunction>) -> FactValue {
    // (1) The op-kind registry is the single source of truth for may_throw. All
    // three call opcodes have may_throw = true today, but reading the registry
    // (never hardcoding) means a future statically-no-throw call opcode is picked
    // up for free — and keeps the discovery-vs-authority rule (doc 46 §1).
    if !crate::tir::op_kinds_generated::opcode_may_throw_table(op.opcode) {
        return FactValue::Proven;
    }
    // (2) A resolved callee with no handler region.
    if let Some(callee) = resolved_callee
        && !callee.has_exception_handlers()
    {
        return FactValue::Proven;
    }
    // (3) A no-throw-allowlisted builtin.
    if op.opcode == OpCode::CallBuiltin
        && let Some(name) = builtin_name(op)
        && builtin_is_no_throw(name)
    {
        return FactValue::Proven;
    }
    FactValue::Unknown
}

// ───────────────────────────────────────────────────────────────────────────
// AnalysisManager registration (the cached AnalysisId, doc 47 §1)
// ───────────────────────────────────────────────────────────────────────────

/// The cached [`Analysis`] for the per-function [`CallFactsTable`], keyed by
/// [`AnalysisId::CallFacts`].
///
/// `compute(func)` produces the **fail-closed intraprocedural floor**
/// ([`CallFactsTable::build_local`]) — sound on a cache miss. The precise,
/// interprocedural table is computed once in the module phase
/// ([`CallFactsTable::build_module`]) and seeded into each function's manager via
/// [`prepopulate`](super::analysis::AnalysisManager::prepopulate). The two paths
/// are monotone: the floor never out-claims the precise table, so a consumer that
/// reads a floor on a miss can only ever miss an optimization, never miscompile.
///
/// CFG- and ops-sensitive: the table is keyed by call-op result `ValueId`, and a
/// removed block (CFG) or a rewritten/removed op (ops) can delete a call site, so
/// the cached table is invalidated by exactly the events that invalidate
/// [`DefMap`](super::analysis::DefMap).
pub struct CallFactsAnalysis;

impl Analysis for CallFactsAnalysis {
    type Result = CallFactsTable;
    const ID: AnalysisId = AnalysisId::CallFacts;
    const CFG_SENSITIVE: bool = true;
    const OPS_SENSITIVE: bool = true;
    fn compute(func: &TirFunction) -> Self::Result {
        CallFactsTable::build_local(func)
    }
}

// Re-export the inliner's eligibility classifier so doc-links resolve and so the
// "single source of truth" relationship is visible from this module.
#[allow(unused_imports)]
use super::passes::inliner::is_inlineable;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirModule;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    /// A function `name` whose entry block makes a `Call` to each callee (with a
    /// captured result `ValueId`), plus `extra_ops` filler `ConstNone` ops, then
    /// returns. Returns the function and the result `ValueId` of the FIRST call.
    fn func_calling(
        name: &str,
        ret: TirType,
        callees: &[&str],
        extra_ops: usize,
    ) -> (TirFunction, Option<ValueId>) {
        let mut func = TirFunction::new(name.into(), vec![], ret);
        let entry = func.entry_block;
        // Allocate result ids for each call + filler up front (mutable borrow of
        // `func` must not overlap the block borrow).
        let call_results: Vec<ValueId> = (0..callees.len()).map(|_| func.fresh_value()).collect();
        let filler: Vec<ValueId> = (0..extra_ops).map(|_| func.fresh_value()).collect();
        let first_result = call_results.first().copied();
        let block = func.blocks.get_mut(&entry).unwrap();
        for (callee, &res) in callees.iter().zip(&call_results) {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str((*callee).to_string()));
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![res],
                attrs,
                source_span: None,
            });
        }
        for v in filler {
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstNone,
                operands: vec![],
                results: vec![v],
                attrs: AttrDict::new(),
                source_span: None,
            });
        }
        block.terminator = Terminator::Return { values: vec![] };
        (func, first_result)
    }

    /// A trivial inlinable leaf: a single `ConstNone` op + `Return`. No calls, no
    /// handlers, small.
    fn leaf_callee(name: &str, ret: TirType) -> TirFunction {
        let mut f = TirFunction::new(name.into(), vec![], ret);
        let entry = f.entry_block;
        let v = f.fresh_value();
        let block = f.blocks.get_mut(&entry).unwrap();
        block.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v],
            attrs: AttrDict::new(),
            source_span: None,
        });
        block.terminator = Terminator::Return { values: vec![] };
        f
    }

    /// A callee with a real exception handler region (`TryStart`/`TryEnd`).
    fn callee_with_handlers(name: &str) -> TirFunction {
        let mut f = TirFunction::new(name.into(), vec![], TirType::None);
        let entry = f.entry_block;
        let block = f.blocks.get_mut(&entry).unwrap();
        for oc in [OpCode::TryStart, OpCode::TryEnd] {
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: oc,
                operands: vec![],
                results: vec![],
                attrs: AttrDict::new(),
                source_span: None,
            });
        }
        block.terminator = Terminator::Return { values: vec![] };
        f
    }

    fn module(funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "m".into(),
            functions: funcs,
        }
    }

    /// Build the precise module table for the function named `caller`.
    fn module_table_for(m: &TirModule, caller: &str) -> CallFactsTable {
        let cg = CallGraph::build(m);
        let summaries = ModuleSummaries::compute(m, &cg);
        let tti = TargetInfo::native_release_fast();
        let mut tables = CallFactsTable::build_module(m, &cg, &summaries, &tti);
        tables.remove(caller).expect("caller table present")
    }

    // -- target classification (the #71 typed fact) ---------------------------

    #[test]
    fn static_direct_target_for_defined_callee() {
        let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        let m = module(vec![caller, leaf_callee("b", TirType::None)]);
        let table = module_table_for(&m, "a");
        let facts = table.get(res).expect("call site recorded");
        assert_eq!(
            facts.target,
            CallTargetFact::StaticDirect { callee: "b".into() }
        );
        assert!(facts.target.is_static_direct());
        assert_eq!(facts.target.static_callee(), Some("b"));
    }

    #[test]
    fn opaque_target_for_extern_callee() {
        // `b` is NOT defined in the module → opaque (extern / cross-batch).
        let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        let m = module(vec![caller]);
        let table = module_table_for(&m, "a");
        let facts = table.get(res).unwrap();
        assert_eq!(facts.target, CallTargetFact::Opaque);
        assert_eq!(facts.target.static_callee(), None);
    }

    // -- leaf ----------------------------------------------------------------

    #[test]
    fn leaf_callee_proven_leaf() {
        // `a` calls `b`; `b` is a leaf (no calls) → leaf = Proven.
        let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        let m = module(vec![caller, leaf_callee("b", TirType::None)]);
        let table = module_table_for(&m, "a");
        assert_eq!(table.get(res).unwrap().leaf, FactValue::Proven);
    }

    #[test]
    fn non_leaf_callee_is_false_leaf() {
        // `a` calls `b`; `b` calls `c` → b is not a leaf → leaf = False (a
        // *decided* negative, not Unknown).
        let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        let (b, _) = func_calling("b", TirType::None, &["c"], 0);
        let m = module(vec![caller, b, leaf_callee("c", TirType::None)]);
        let table = module_table_for(&m, "a");
        assert_eq!(table.get(res).unwrap().leaf, FactValue::False);
    }

    #[test]
    fn opaque_target_leaf_is_unknown() {
        // Extern callee → leaf cannot be decided → Unknown (fail-closed), NOT
        // False.
        let (caller, res) = func_calling("a", TirType::None, &["ext"], 0);
        let res = res.unwrap();
        let m = module(vec![caller]);
        let table = module_table_for(&m, "a");
        assert_eq!(table.get(res).unwrap().leaf, FactValue::Unknown);
    }

    // -- inlinable (single source of truth vs the inliner) -------------------

    #[test]
    fn inlinable_leaf_is_eligible_and_matches_is_inlineable() {
        let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        let b = leaf_callee("b", TirType::None);
        let m = module(vec![caller, b]);
        let cg = CallGraph::build(&m);
        let summaries = ModuleSummaries::compute(&m, &cg);
        let tti = TargetInfo::native_release_fast();
        let tables = CallFactsTable::build_module(&m, &cg, &summaries, &tti);
        let facts = tables["a"].get(res).unwrap();
        assert_eq!(facts.inlinable, InlineEligibility::Eligible);
        // EQUIVALENCE: the side-table eligibility bool == is_inlineable's bool.
        let b_body = m.functions.iter().find(|f| f.name == "b").unwrap();
        assert_eq!(
            facts.inlinable.is_eligible(),
            is_inlineable(b_body, &cg, &summaries, &tti)
        );
    }

    #[test]
    fn inlinable_why_not_has_handlers() {
        // `a` calls `b`; `b` has a try/except handler region → WhyNot(HasHandlers).
        let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        let m = module(vec![caller, callee_with_handlers("b")]);
        let table = module_table_for(&m, "a");
        let facts = table.get(res).unwrap();
        assert_eq!(
            facts.inlinable,
            InlineEligibility::WhyNot(InlineWhyNot::HasHandlers)
        );
        assert_eq!(facts.inlinable.why_not(), Some(InlineWhyNot::HasHandlers));
        // A handler-bearing callee is NOT no-throw via the callee-handler rule.
        assert_eq!(facts.no_throw, FactValue::Unknown);
    }

    #[test]
    fn inlinable_why_not_recursive() {
        // Direct self-recursion: `a` calls `a`. The recursive set contains `a`,
        // so a call to it is WhyNot(Recursive).
        let (caller, res) = func_calling("a", TirType::None, &["a"], 0);
        let res = res.unwrap();
        let m = module(vec![caller]);
        let table = module_table_for(&m, "a");
        let facts = table.get(res).unwrap();
        // Self-call target IS static-direct (a is defined) and resolves to a's
        // own body, which is in the recursive set.
        assert_eq!(
            facts.inlinable,
            InlineEligibility::WhyNot(InlineWhyNot::Recursive)
        );
    }

    // -- no_throw ------------------------------------------------------------

    #[test]
    fn no_throw_proven_for_handlerless_callee() {
        // `b` is a plain leaf with no handlers → calling it is no_throw = Proven.
        let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        let m = module(vec![caller, leaf_callee("b", TirType::None)]);
        let table = module_table_for(&m, "a");
        assert_eq!(table.get(res).unwrap().no_throw, FactValue::Proven);
    }

    #[test]
    fn no_throw_unknown_for_opaque_target() {
        let (caller, res) = func_calling("a", TirType::None, &["ext"], 0);
        let res = res.unwrap();
        let m = module(vec![caller]);
        let table = module_table_for(&m, "a");
        assert_eq!(table.get(res).unwrap().no_throw, FactValue::Unknown);
    }

    // -- typed_return --------------------------------------------------------

    #[test]
    fn typed_return_none_for_dynbox_result() {
        // The call result's TirType defaults to DynBox (TirFunction::new doesn't
        // type fresh values) → typed_return = None.
        let (caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        let m = module(vec![caller, leaf_callee("b", TirType::None)]);
        let table = module_table_for(&m, "a");
        assert_eq!(table.get(res).unwrap().typed_return, None);
    }

    #[test]
    fn typed_return_some_for_typed_result() {
        // Tag the call result with a concrete I64 type → typed_return = Some.
        let (mut caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        caller.value_types.insert(res, TirType::I64);
        let m = module(vec![caller, leaf_callee("b", TirType::None)]);
        let table = module_table_for(&m, "a");
        // I64 floors to MaybeBigInt in the Phase-0 lattice.
        assert_eq!(
            table.get(res).unwrap().typed_return,
            Some(Repr::MaybeBigInt)
        );
    }

    // -- intraprocedural floor (Analysis::compute) is fail-closed -------------

    #[test]
    fn local_floor_is_fail_closed() {
        // The local floor sees no module: target = Opaque, leaf = Unknown,
        // inlinable = Unknown, no_throw = Unknown (a plain `Call` opcode throws
        // and there is no resolved body) — but typed_return is still local.
        let (mut caller, res) = func_calling("a", TirType::None, &["b"], 0);
        let res = res.unwrap();
        caller.value_types.insert(res, TirType::Str);
        let table = CallFactsTable::build_local(&caller);
        let facts = table.get(res).unwrap();
        assert_eq!(facts.target, CallTargetFact::Opaque);
        assert_eq!(facts.leaf, FactValue::Unknown);
        assert_eq!(facts.inlinable, InlineEligibility::Unknown);
        assert_eq!(facts.no_throw, FactValue::Unknown);
        // typed_return is purely local → still resolved (Str → DynBox carrier).
        assert_eq!(facts.typed_return, Some(Repr::DynBox));
    }

    #[test]
    fn local_floor_never_out_claims_module_table() {
        // MONOTONICITY: for every recorded call site, the local floor's facts are
        // never *stronger* (more Proven / more StaticDirect) than the precise
        // module table's. This is the soundness contract: a cache miss can only
        // miss an opt, never miscompile.
        let (caller, _) = func_calling("a", TirType::None, &["b", "c"], 1);
        let (b, _) = func_calling("b", TirType::None, &["c"], 0); // non-leaf
        let m = module(vec![caller, b, leaf_callee("c", TirType::None)]);
        let cg = CallGraph::build(&m);
        let summaries = ModuleSummaries::compute(&m, &cg);
        let tti = TargetInfo::native_release_fast();
        let module_tables = CallFactsTable::build_module(&m, &cg, &summaries, &tti);
        let a_body = m.functions.iter().find(|f| f.name == "a").unwrap();
        let local = CallFactsTable::build_local(a_body);
        for (res, mfacts) in module_tables["a"].iter() {
            let lfacts = local.get(res).expect("same call sites keyed");
            // The floor's target is always Opaque (weakest).
            assert_eq!(lfacts.target, CallTargetFact::Opaque);
            // The floor never claims Proven where the module table is weaker.
            if lfacts.leaf.is_proven() {
                assert!(mfacts.leaf.is_proven(), "floor out-claimed leaf");
            }
            if lfacts.no_throw.is_proven() {
                assert!(mfacts.no_throw.is_proven(), "floor out-claimed no_throw");
            }
            // The floor never claims Eligible where the module table did not.
            if lfacts.inlinable.is_eligible() {
                assert!(
                    mfacts.inlinable.is_eligible(),
                    "floor out-claimed inlinable"
                );
            }
        }
    }

    // -- table mechanics -----------------------------------------------------

    #[test]
    fn table_records_one_fact_per_call_site() {
        let (caller, _) = func_calling("a", TirType::None, &["b", "c"], 0);
        let m = module(vec![
            caller,
            leaf_callee("b", TirType::None),
            leaf_callee("c", TirType::None),
        ]);
        let table = module_table_for(&m, "a");
        assert_eq!(table.len(), 2, "two call sites → two records");
        assert!(!table.is_empty());
    }

    #[test]
    fn fact_value_from_decided_is_proven_or_false_not_unknown() {
        assert_eq!(FactValue::from_decided(true), FactValue::Proven);
        assert_eq!(FactValue::from_decided(false), FactValue::False);
        assert!(FactValue::Proven.is_proven());
        assert!(!FactValue::Unknown.is_proven());
        assert!(!FactValue::False.is_proven());
    }

    /// `CallGraph::is_defined` must exist for the typed-target resolution (the
    /// classifier reads it instead of the private `classify_call_op`'s `defined`
    /// set). This pins that public accessor.
    #[test]
    fn call_graph_is_defined_accessor() {
        let m = module(vec![leaf_callee("b", TirType::None)]);
        let cg = CallGraph::build(&m);
        assert!(cg.is_defined("b"));
        assert!(!cg.is_defined("nope"));
    }
}
