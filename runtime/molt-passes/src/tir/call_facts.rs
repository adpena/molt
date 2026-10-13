//! Per-call-site facts requested by a consumer (foundation design 47).
//!
//! [`CallFactsTable::build_for_function`] records precise facts for one function
//! using an explicit, current module, call graph, summaries, and target. FactGraph
//! uses this path after module transformations. Ordinary code generation does
//! not build these tables. [`CallFactsTable::build_local`] supplies the sound
//! intraprocedural floor used by [`CallFactsAnalysis`] and local FactGraph views.
//!
//! Both paths use the shared [`FunctionCallSites`] projection and key each
//! result-bearing site by its first result [`ValueId`]. Unknown facts remain
//! pessimistic; neither path changes the function body.
//!
//! | field | source | conservative rule |
//! | --- | --- | --- |
//! | [`CallFacts::target`] | `call_targets` operation provenance + module membership | typed [`CallTargetFact`] — `StaticDirect{name}` only for proven direct user calls, else `Opaque` |
//! | [`CallFacts::typed_return`] | the result `ValueId`'s semantic type and `Repr::default_for` | conservative `Some(repr)` for a known semantic type (possibly `DynBox`); `None` for an unknown type |
//! | [`CallFacts::leaf`] | `CallGraph::makes_any_call` | `Proven` iff the resolved callee makes no call of any kind; `False` iff it provably does; `Unknown` for an unresolved (opaque) target |
//! | [`CallFacts::no_throw`] | shared operation-aware effect contract | `Proven` only when the operation contract is statically no-throw; else `Unknown`. Handler absence and builtin names are not effect proofs. |
//! | [`CallFacts::inlinable`] | `inliner::classify_inline_eligibility` | the typed [`InlineEligibility`] (Eligible \| WhyNot(reason)) — the SAME value `inliner::is_inlineable` derives its bool from (single source of truth) |
//!
//! `no_alloc` and `no_escape_args` remain conservative until a sound producer
//! and consumer are implemented. Unknown facts cannot authorize optimizations.
//!
//! Precise `leaf`, `inlinable`, and target classification require callee bodies
//! and module membership. The per-function [`Analysis`] trait cannot provide
//! that context; its cached local floor makes no callee-side claims. Precise
//! facts are built explicitly at the consumer, without prepopulating unrelated
//! per-function analysis managers.
//!
//! ## Invariants
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
use super::call_sites::FunctionCallSites;
use super::function::{TirFunction, TirModule};
use super::passes::ip_summary::ModuleSummaries;
use super::target_info::TargetInfo;
use super::values::ValueId;

mod model;
mod site_analysis;

pub use model::{
    CallFacts, CallTargetFact, Confidence, FactValue, GuardId, InlineEligibility, InlineWhyNot,
};
use site_analysis::{analyze_call_site_local, analyze_call_site_module};

/// Per-function side-table of [`CallFacts`], keyed by each call op's **result
/// `ValueId`**. A `BTreeMap` for deterministic iteration (the coverage tool and
/// any dump walk it in a stable order). Built precisely by
/// [`CallFactsTable::build_for_function`] (interprocedural) or as a fail-closed floor by
/// [`CallFactsTable::build_local`] (intraprocedural).
///
/// The internal key is the raw `ValueId` index; the public API stays typed.
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

    /// Build precise facts only for `func`, using analyses of the same current
    /// `module`. The caller owns the module context and requests the table when
    /// it has a consumer; this read-only analysis does not transform any body.
    pub fn build_for_function(
        func: &TirFunction,
        module: &TirModule,
        call_graph: &CallGraph,
        summaries: &ModuleSummaries,
        tti: &TargetInfo,
    ) -> Self {
        // Function bodies by name for inline eligibility. The ordered index
        // provides deterministic traversal and logarithmic callee lookup.
        let by_name: BTreeMap<&str, &TirFunction> = module
            .functions
            .iter()
            .map(|f| (f.name.as_str(), f))
            .collect();

        let mut table = CallFactsTable::default();
        let sites = FunctionCallSites::for_function(func);
        for (&bid, block) in &func.blocks {
            for (index, op) in block.ops.iter().enumerate() {
                let Some(site) = sites.at((bid, index)) else {
                    continue;
                };
                let Some(result) = op.results.first().copied() else {
                    continue;
                };
                let facts =
                    analyze_call_site_module(op, site, func, call_graph, summaries, tti, &by_name);
                table.facts.insert(result.0, facts);
            }
        }
        table
    }

    /// Build the **fail-closed intraprocedural floor** for `func` alone (no module
    /// context). Every callee-side fact that cannot be proven from `func` is
    /// `Unknown`; only purely-local facts (`typed_return`; `no_throw` via a
    /// statically-no-throw operation contract) can be proven. This is the
    /// [`Analysis::compute`] path — sound by construction, never out-claiming the
    /// precise [`Self::build_for_function`] table.
    pub fn build_local(func: &TirFunction) -> CallFactsTable {
        let mut table = CallFactsTable::default();
        let sites = FunctionCallSites::for_function(func);
        for (&bid, block) in &func.blocks {
            for (index, op) in block.ops.iter().enumerate() {
                let Some(site) = sites.at((bid, index)) else {
                    continue;
                };
                let Some(result) = op.results.first().copied() else {
                    continue;
                };
                table
                    .facts
                    .insert(result.0, analyze_call_site_local(op, site, func));
            }
        }
        table
    }
}

// ───────────────────────────────────────────────────────────────────────────
// AnalysisManager registration (the cached AnalysisId)
// ───────────────────────────────────────────────────────────────────────────

/// The cached [`Analysis`] for the per-function [`CallFactsTable`], keyed by
/// [`AnalysisId::CallFacts`].
///
/// `compute(func)` produces the fail-closed intraprocedural floor through
/// [`CallFactsTable::build_local`]. A consumer with current module context can
/// request [`CallFactsTable::build_for_function`] directly. The floor never
/// out-claims that precise table, so a local analysis can miss an optimization
/// but cannot invent a callee-side proof.
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
