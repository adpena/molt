use crate::tir::analysis::AnalysisManager;
use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::AliasAnalysis;

use super::super::PassStats;
use super::annotate::annotate;
use super::scan::analyze;

/// Convenience: analyze + annotate in one step.
pub fn run(func: &mut TirFunction, am: &mut AnalysisManager) -> PassStats {
    let alias = am.get::<AliasAnalysis>(func).clone();
    let candidates = analyze(func, &alias);
    annotate(func, &candidates)
}
