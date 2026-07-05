use crate::tir::analysis::AnalysisManager;
use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::{AliasAnalysis, AliasAnalysisResult};

use super::super::PassStats;
use super::cross_block::eliminate_cross_block_pairs;
use super::deferred::{eliminate_non_heap_exposed_refs, promote_unique_decref_to_free};
use super::facts::collect_stack_alloc_values;
use super::local::eliminate_local_pairs;
use super::loops::eliminate_loop_invariant_pairs;

/// Eliminate redundant IncRef/DecRef pairs.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let has_tir_inserted_drops = super::super::drop_insertion::attr_is_true(
        func,
        super::super::drop_insertion::DROP_INSERTED_ATTR,
    ) || super::super::drop_insertion::attr_is_true(
        func,
        super::super::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR,
    );
    run_with(func, am, has_tir_inserted_drops)
}

/// Post-drop-insertion elision, limited to balance-preserving steps.
pub fn run_post_drop(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    run_with(func, am, true)
}

fn run_with(func: &mut TirFunction, am: &mut AnalysisManager, post_drop: bool) -> PassStats {
    let mut stats = PassStats {
        name: "refcount_elim",
        ..Default::default()
    };

    let alias: AliasAnalysisResult = am.get::<AliasAnalysis>(func).clone();
    let stack_alloc_vals = collect_stack_alloc_values(func);

    eliminate_local_pairs(func, &alias, &stack_alloc_vals, &mut stats);
    eliminate_cross_block_pairs(func, am, &alias, &mut stats);
    eliminate_loop_invariant_pairs(func, am, &alias, &mut stats);

    if post_drop {
        return stats;
    }

    eliminate_non_heap_exposed_refs(func, &mut stats);
    promote_unique_decref_to_free(func, &mut stats);

    stats
}
