use crate::tir::analysis::AnalysisManager;
use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::{AliasAnalysis, AliasAnalysisResult};

use super::super::PassStats;
use super::cross_block::eliminate_cross_block_pairs;
use super::facts::collect_rc_inert_values;
use super::local::eliminate_local_pairs;
use crate::tir::passes::value_range::ValueRange;

/// All pipelines preserve heap destruction and use the same pairing contract.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let mut stats = PassStats {
        name: "refcount_elim",
        ..Default::default()
    };
    let alias: AliasAnalysisResult = am.get::<AliasAnalysis>(func).clone();
    let ranges = am.get::<ValueRange>(func).clone();
    let inert_values = collect_rc_inert_values(func, &alias, &ranges);
    eliminate_local_pairs(func, &alias, &inert_values, &mut stats);
    eliminate_cross_block_pairs(func, am, &alias, &mut stats);
    stats
}

/// Drop-inserted pipelines share the same release-preserving implementation.
pub fn run_post_drop(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    run(func, am)
}
