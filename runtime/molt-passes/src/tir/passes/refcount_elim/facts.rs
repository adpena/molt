use std::collections::HashSet;

use crate::tir::ValueRangeResult;
use crate::tir::function::TirFunction;
use crate::tir::passes::alias_analysis::AliasAnalysisResult;
use crate::tir::values::ValueId;

/// Only shared physical non-heap carrier facts grant unconditional RC elision.
/// Allocation placement and capture state do not. Exact aliases inherit representation; mixed
/// CFG arguments require the shared carrier analysis's all-incoming proof.
pub(super) fn collect_rc_inert_values(
    func: &TirFunction,
    alias: &AliasAnalysisResult,
    ranges: &ValueRangeResult,
) -> HashSet<ValueId> {
    crate::representation_facts::non_heap_values_for(func, ranges)
        .into_iter()
        .map(|value| alias.root(value))
        .collect()
}
