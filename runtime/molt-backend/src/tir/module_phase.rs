//! Whole-program **module pass phase** (Tier-0 substrate **S4**).
//!
//! Before this module existed, the TIR pipeline was *strictly per-function*:
//! [`crate::tir::parallel::compile_module_parallel`] ran the S1
//! [`PassManager`](crate::tir::pass_manager::PassManager) over each function
//! independently, and the only whole-program structure (the call graph, the leaf
//! set) lived in the SimpleIR layer of the native backend. There was no place to
//! compute a module-level analysis that the interprocedural tier (inliner E1,
//! IP-escape E3, IPSCCP E4, monomorphization E5) could read.
//!
//! [`run_module_pipeline`] is that place. It runs **once, before** the
//! per-function pipeline, over the whole [`TirModule`], and produces a
//! [`ModuleAnalysis`] — the call graph plus bottom-up function summaries — that
//! whole-program consumers read. It is the module-scope analog of S1's
//! per-function [`AnalysisManager`](crate::tir::analysis::AnalysisManager): a
//! single, shared, lazily-built source of interprocedural truth.
//!
//! ## How it composes with S1 + S2 + `parallel.rs` (the reconciliation)
//!
//! This phase **does not fork a parallel pipeline.** The contract is:
//!
//! 1. The driver lifts every function to TIR and assembles a [`TirModule`].
//! 2. `run_module_pipeline(&mut module)` runs here, producing [`ModuleAnalysis`]
//!    (read-only over the module — it builds the call graph + summaries, it does
//!    not transform function bodies). The `&mut` is reserved for the future
//!    module *transforms* (inlining splices bodies across functions); today the
//!    phase is analysis-only, so it leaves bodies untouched.
//! 3. The driver then runs the existing per-function pipeline
//!    ([`compile_module_parallel`](crate::tir::parallel::compile_module_parallel),
//!    rayon work-stealing, each function threading its own S1
//!    [`AnalysisManager`] and the shared S2
//!    [`TargetInfo`](crate::tir::target_info::TargetInfo)). The module phase
//!    runs *before* this and its result is available for the duration of module
//!    compilation.
//!
//! The S2 [`TargetInfo`] threads through unchanged: it is the same `&TargetInfo`
//! that `compile_module_parallel` already shares with every function, and it is
//! passed here so a future cost-model-gated module transform (the inliner's
//! per-call ROI) consults the one source of truth rather than a fresh constant.
//!
//! ## The native-backend caveat (batching / per-function roundtrip)
//!
//! The native driver compiles in two shapes (see `native_backend::simple_backend`
//! and `main.rs`):
//!
//! * **Non-batched**: the whole program is one object; the leaf set is computed
//!   over all functions.
//! * **Batched** (>64 functions): `main.rs` builds ONE whole-program module
//!   context (including the leaf set) over *all* functions, then partitions into
//!   batches that each receive that same whole-program context. So even in the
//!   batched shape the leaf set this phase computes is whole-program — sound.
//!
//! Where a batch backend recomputes analysis over only its own function subset
//! (the per-batch fallback when no module context is threaded), the call graph
//! sees only that subset: any call to a function outside the batch resolves to
//! [`crate::tir::call_graph::CallEdge::Opaque`] (the named callee is not in this
//! sub-module), which *disqualifies* leaf-ness — strictly conservative, never
//! unsound. A function the per-batch graph would have called a leaf can only be
//! "more leaf" than the whole-program truth allows, and since opaque calls block
//! leaf-ness, the per-batch set is a subset of the whole-program set. Skipping
//! the recursion guard on a subset of the truly-safe leaves is sound (it just
//! forgoes an optimization on the cross-batch callers).

use super::call_graph::CallGraph;
use super::function::TirModule;
use super::passes::ip_summary::ModuleSummaries;
use super::target_info::TargetInfo;

/// The result of the whole-program module phase: the interprocedural analysis
/// the IPO tier reads. Computed once per module, shared read-only.
#[derive(Debug, Clone, Default)]
pub struct ModuleAnalysis {
    /// The whole-program call graph (static-direct + opaque edges, SCC /
    /// bottom-up order / recursive set / leaf set).
    pub call_graph: CallGraph,
    /// Bottom-up function summaries (leaf / op-count / return-type), one per
    /// function in the module.
    pub summaries: ModuleSummaries,
    /// Names of the functions the inliner CHANGED (had ≥1 callee spliced in).
    /// Production codegen back-converts ONLY these functions' (post-inline) TIR
    /// to SimpleIR; every other function keeps its byte-identical per-function
    /// pipeline output (no redundant second TIR roundtrip).
    pub changed_functions: Vec<String>,
}

impl ModuleAnalysis {
    /// The leaf-function set — functions that make no call of any kind and so
    /// cannot recurse. This is the SOUND, strictly-more-precise replacement for
    /// the native backend's legacy SimpleIR "has no call op" leaf scan.
    pub fn leaf_functions(&self) -> std::collections::BTreeSet<String> {
        self.call_graph.leaf_functions()
    }
}

/// Run the whole-program module pass phase over `module`, returning the
/// [`ModuleAnalysis`] for the duration of module compilation.
///
/// Runs **before** the per-function
/// [`compile_module_parallel`](crate::tir::parallel::compile_module_parallel).
/// The phase:
///
/// 1. Builds the call graph + bottom-up summaries over the lifted module.
/// 2. Runs the **E1 function inliner** ([`run_inliner`](super::passes::inliner::run_inliner)):
///    a module *transform* that splices in-budget, exception-free,
///    non-recursive, non-generator leaf-ish callees at their static call sites,
///    bottom-up, re-optimizing each merged caller via the per-function pipeline.
/// 3. Because step 2 mutates bodies (calls disappear, op counts change), the
///    call graph and summaries are **rebuilt** so the returned [`ModuleAnalysis`]
///    reflects the post-inline module — the leaf set the native backend's
///    recursion-guard skip consults must describe the *inlined* program, not the
///    pre-inline one.
///
/// `tti` is the unified cost model (Tier-0 S2): the single source of truth for
/// the inliner's per-callee budget.
pub fn run_module_pipeline(module: &mut TirModule, tti: &TargetInfo) -> ModuleAnalysis {
    let call_graph = CallGraph::build(module);
    let summaries = ModuleSummaries::compute(module, &call_graph);

    // E1: inline (a module transform — mutates bodies across functions).
    let inline_stats = super::passes::inliner::run_inliner(module, &call_graph, &summaries, tti);
    // Observability (mirrors TIR_OPT_STATS): the per-module inliner outcome, so
    // an unexpectedly-inert activation is visible instead of silently zero.
    if std::env::var("MOLT_INLINE_STATS").as_deref() == Ok("1") {
        eprintln!(
            "[E1] module '{}': {} call sites inlined into {} functions {:?}",
            module.name,
            inline_stats.sites_inlined,
            inline_stats.functions_changed,
            inline_stats.changed_functions,
        );
    }

    // Module-slot promotion (after inlining, so merged bodies — whose calls
    // disappeared — become promotable loops). Promoted functions are
    // re-optimized through the same refine→pipeline→refine contract the inliner
    // uses, so the value-range/RawI64Safe machinery proves the now-SSA loop
    // phis and the backends receive fully-refined bodies.
    let (promo_stats, promo_changed) =
        super::passes::module_slot_promotion::run_module_slot_promotion(module);
    if std::env::var("MOLT_INLINE_STATS").as_deref() == Ok("1") {
        eprintln!(
            "[E1] module '{}': slot-promotion {} slots / {} ops eliminated in {} functions {:?}",
            module.name,
            promo_stats.slots_promoted,
            promo_stats.ops_eliminated,
            promo_stats.functions_changed,
            promo_changed,
        );
    }
    if !promo_changed.is_empty() {
        let changed_set: std::collections::HashSet<&str> =
            promo_changed.iter().map(String::as_str).collect();
        for func in &mut module.functions {
            if changed_set.contains(func.name.as_str()) {
                super::type_refine::refine_types(func);
                let _ = super::passes::run_pipeline(func, tti);
                super::type_refine::refine_types(func);
            }
        }
    }

    let mut changed_functions = inline_stats.changed_functions;
    for name in promo_changed {
        if !changed_functions.contains(&name) {
            changed_functions.push(name);
        }
    }

    // Rebuild over the post-inline module: inlining removed `Call` ops and grew
    // caller bodies, so the leaf set / edges / op counts the returned analysis
    // exposes must reflect the merged program.
    let call_graph = CallGraph::build(module);
    let summaries = ModuleSummaries::compute(module, &call_graph);
    ModuleAnalysis {
        call_graph,
        summaries,
        changed_functions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;

    fn func_calling(name: &str, callees: &[&str]) -> TirFunction {
        let mut func = TirFunction::new(name.into(), vec![], TirType::None);
        let entry = func.entry_block;
        let block = func.blocks.get_mut(&entry).unwrap();
        for callee in callees {
            let mut attrs = AttrDict::new();
            attrs.insert("s_value".into(), AttrValue::Str((*callee).to_string()));
            block.ops.push(TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::Call,
                operands: vec![],
                results: vec![],
                attrs,
                source_span: None,
            });
        }
        block.terminator = Terminator::Return { values: vec![] };
        func
    }

    fn module(funcs: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "m".into(),
            functions: funcs,
        }
    }

    #[test]
    fn module_phase_produces_call_graph_and_summaries() {
        // `b` is a trivial inlinable leaf (no ops, just Return), so the module
        // phase inlines the static call `a → b`. The returned analysis is
        // rebuilt over the post-inline module: every function is summarized, and
        // because a's only call was to the inlined-away b, the post-inline graph
        // records no a→b edge (a is now a leaf).
        let mut m = module(vec![
            func_calling("a", &["b"]),
            func_calling("b", &[]),
        ]);
        let tti = TargetInfo::native_release_fast();
        let analysis = run_module_pipeline(&mut m, &tti);

        // Both functions remain summarized (the post-inline rebuild covers all).
        assert!(analysis.summaries.get("a").is_some());
        assert!(analysis.summaries.get("b").is_some());
        // b was inlined into a, so a no longer statically calls b.
        assert!(
            analysis.call_graph.callees("a").is_empty(),
            "a's static call to b was inlined away"
        );
        // After inlining, a makes no call → it joins the leaf set alongside b.
        assert!(analysis.leaf_functions().contains("a"));
        assert!(analysis.leaf_functions().contains("b"));
    }

    #[test]
    fn module_phase_leaf_set_matches_call_graph() {
        let mut m = module(vec![
            func_calling("a", &["b"]),
            func_calling("b", &["c"]),
            func_calling("c", &[]),
            func_calling("d", &[]),
        ]);
        let tti = TargetInfo::native_release_fast();
        let analysis = run_module_pipeline(&mut m, &tti);
        assert_eq!(
            analysis.leaf_functions(),
            analysis.call_graph.leaf_functions()
        );
        assert_eq!(analysis.summaries.leaf_functions(), analysis.leaf_functions());
    }

    #[test]
    fn module_phase_inlines_static_calls() {
        // The module phase now runs the E1 inliner (a body transform). A static
        // call `a → b` to an in-budget, exception-free, non-recursive leaf `b`
        // is inlined: a's body no longer contains the `Call` to b, and the
        // post-inline analysis no longer records the a→b edge.
        let a = func_calling("a", &["b"]);
        // `b` is a trivial leaf: a single ConstNone op + Return. Inlinable.
        let mut b = TirFunction::new("b".into(), vec![], TirType::None);
        let bentry = b.entry_block;
        let v = b.fresh_value();
        b.blocks.get_mut(&bentry).unwrap().ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstNone,
            operands: vec![],
            results: vec![v],
            attrs: AttrDict::new(),
            source_span: None,
        });
        b.blocks.get_mut(&bentry).unwrap().terminator = Terminator::Return { values: vec![] };

        let mut m = module(vec![a, b]);
        let tti = TargetInfo::native_release_fast();
        let _ = run_module_pipeline(&mut m, &tti);

        // a's body no longer has a Call op (b was inlined).
        let a_after = m.functions.iter().find(|f| f.name == "a").unwrap();
        let a_calls: usize = a_after
            .blocks
            .values()
            .flat_map(|blk| blk.ops.iter())
            .filter(|op| op.opcode == OpCode::Call)
            .count();
        assert_eq!(a_calls, 0, "the static call a→b is inlined away");

        // a is valid SSA after the merge + per-function re-optimization.
        crate::tir::verify::verify_function(a_after)
            .unwrap_or_else(|e| panic!("a invalid after module-phase inlining: {e:?}"));
    }
}
