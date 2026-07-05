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
mod tests {
    use super::*;
    use crate::repr::Repr;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirModule;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
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
