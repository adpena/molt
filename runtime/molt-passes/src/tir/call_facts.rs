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
//! record is attached to every opcode whose generated [`CallOpcodeRole`] records
//! facts (`Call`, dynamic-method opcodes, and runtime builtins), keyed by the
//! op's result [`ValueId`] in a per-function
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
use super::passes::ip_summary::ModuleSummaries;
use super::target_info::TargetInfo;
use super::values::ValueId;

mod model;
mod site_analysis;

pub use model::{
    CallFacts, CallTargetFact, Confidence, FactValue, GuardId, InlineEligibility, InlineWhyNot,
};
use site_analysis::{analyze_call_site_local, analyze_call_site_module, call_op_result};

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
mod tests;
