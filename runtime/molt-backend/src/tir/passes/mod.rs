//! TIR optimization passes.
//! Each pass transforms a TirFunction in-place and returns statistics.

pub mod alias_analysis;
pub mod bce;
pub mod block_versioning;
pub mod branchless_count;
pub mod canonicalize;
pub mod check_exception_elim;
pub mod copy_prop;
pub mod counted_loop;
pub mod dce;
pub mod dead_store_elim;
pub mod deforestation;
pub mod effects;
pub mod escape_analysis;
pub mod fast_math;
pub mod gvn;
pub mod inliner;
pub mod ip_summary;
pub mod iter_devirt;
pub mod licm;
pub mod loop_unroll;
pub mod mem_gvn;
pub mod memory_ssa;
pub mod module_slot_promotion;
pub mod overflow_peel;
pub mod polyhedral;
pub mod range_devirt;
mod reachability;
pub mod refcount_elim;
pub mod reuse_analysis;
pub mod scev;
pub mod sccp;
pub mod strength_reduction;
pub mod type_guard_hoist;
pub mod unboxing;
pub mod value_range;
pub mod vectorize;

/// Statistics returned by each optimization pass.
#[derive(Debug, Default, Clone)]
pub struct PassStats {
    pub name: &'static str,
    pub values_changed: usize,
    pub ops_removed: usize,
    pub ops_added: usize,
}

/// Generous upper bound on the number of pass stats produced per pipeline
/// run. Used purely as a `Vec::with_capacity` hint to avoid reallocations
/// in the hot pipeline path. The pipeline body (the `run_pass!` invocations
/// in [`run_pipeline`]) is the source of truth for the actual pass count;
/// this hint only needs to be safely-too-large, never exact.
pub const PIPELINE_PASS_CAPACITY_HINT: usize = 32;

/// Run the full TIR optimization pipeline on a function.
///
/// This is the public entry point. It builds the canonical 26-pass pipeline
/// ([`pass_manager::build_default_pipeline`](crate::tir::pass_manager::build_default_pipeline))
/// and runs it through the [`PassManager`](crate::tir::pass_manager::PassManager),
/// which threads a per-function
/// [`AnalysisManager`](crate::tir::analysis::AnalysisManager) so dominators,
/// the predecessor map, reachability sets, the loop forest and the def map are
/// computed once and shared across passes — with CFG-aware invalidation after
/// every CFG-mutating pass.
///
/// The pass set, ordering, snapshot/restore-on-zero-delta behavior, and the
/// post-pipeline `verify_function` are identical to the former monolithic
/// pipeline; only the dispatch and analysis-caching mechanism changed. See
/// [`pass_manager::build_default_pipeline`](crate::tir::pass_manager::build_default_pipeline)
/// for the phase-ordering rationale and per-pass mutation classes.
///
/// `tti` is the unified cost model (Tier-0 S2): the single, target-aware source
/// of truth for every profitability decision (inline/unroll/vectorize/tile/
/// branchless thresholds). Callers pass the per-(target, profile) instance for
/// the backend they are lowering to; the behavioral baseline
/// [`TargetInfo::native_release_fast`](crate::tir::target_info::TargetInfo::native_release_fast)
/// reproduces every pre-S2 decision exactly.
pub fn run_pipeline(
    func: &mut super::function::TirFunction,
    tti: &super::target_info::TargetInfo,
) -> Vec<PassStats> {
    super::pass_manager::build_default_pipeline(tti.clone()).run(func)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::tir::blocks::{BlockId, Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::types::TirType;

    use super::run_pipeline;

    fn minimal_function() -> TirFunction {
        let entry = BlockId(0);
        let mut blocks = HashMap::new();
        blocks.insert(
            entry,
            TirBlock {
                id: entry,
                args: Vec::new(),
                ops: Vec::new(),
                terminator: Terminator::Return { values: Vec::new() },
            },
        );
        TirFunction {
            name: "pipeline_shape".into(),
            param_names: Vec::new(),
            param_types: Vec::new(),
            return_type: TirType::None,
            blocks,
            entry_block: entry,
            next_value: 0,
            next_block: 1,
            attrs: HashMap::new(),
            value_types: HashMap::new(),
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        }
    }

    #[test]
    fn pipeline_records_every_pass_unconditionally() {
        let mut func = minimal_function();
        let stats = run_pipeline(&mut func, &crate::tir::target_info::TargetInfo::native_release_fast());
        let names: Vec<_> = stats.iter().map(|stat| stat.name).collect();
        assert_eq!(
            names,
            vec![
                "range_devirt",
                "iter_devirt",
                "tuple_scalarize",
                "loop_unroll",
                "canonicalize",
                "unboxing",
                "block_versioning",
                "canonicalize_post",
                "gvn",
                "licm",
                "escape_analysis",
                "refcount_elim",
                "reuse_analysis",
                "dead_store_elim",
                "mem_gvn",
                "type_guard_hoist",
                "sccp",
                "strength_reduction",
                "fast_math",
                "branchless_count",
                "bce",
                "vectorize",
                "polyhedral",
                "check_exception_elim",
                "overflow_peel",
                "copy_prop",
                "dce",
            ],
        );
    }
}
