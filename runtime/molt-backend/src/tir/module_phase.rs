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
/// Today this builds the call graph and the bottom-up summaries (analysis only;
/// it does not transform bodies — hence `module` is read through `&*module`).
/// The `&mut TirModule` and the `&TargetInfo` are the stable signature the
/// module *transforms* (the E1 inliner) will use without a further API change:
/// the inliner mutates bodies across functions and consults the cost model.
pub fn run_module_pipeline(module: &mut TirModule, _tti: &TargetInfo) -> ModuleAnalysis {
    let call_graph = CallGraph::build(module);
    let summaries = ModuleSummaries::compute(module, &call_graph);
    ModuleAnalysis {
        call_graph,
        summaries,
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
        let mut m = module(vec![
            func_calling("a", &["b"]),
            func_calling("b", &[]),
        ]);
        let tti = TargetInfo::native_release_fast();
        let analysis = run_module_pipeline(&mut m, &tti);

        assert_eq!(analysis.call_graph.callees("a"), &["b".to_string()]);
        assert!(analysis.summaries.get("a").is_some());
        assert!(analysis.summaries.get("b").is_some());
        assert!(analysis.leaf_functions().contains("b"));
        assert!(!analysis.leaf_functions().contains("a"));
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
    fn module_phase_does_not_mutate_bodies() {
        // Analysis-only: op counts are unchanged after the phase runs.
        let before = func_calling("a", &["b"]);
        let before_ops: usize = before.blocks.values().map(|b| b.ops.len()).sum();
        let mut m = module(vec![before, func_calling("b", &[])]);
        let tti = TargetInfo::native_release_fast();
        let _ = run_module_pipeline(&mut m, &tti);
        let after_ops: usize = m.functions[0].blocks.values().map(|b| b.ops.len()).sum();
        assert_eq!(before_ops, after_ops);
    }
}
