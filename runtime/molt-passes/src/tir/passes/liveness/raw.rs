use molt_ir::deterministic_hash::HashSet;

use crate::representation_facts::analyze_non_heap_carriers;
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

/// Project the shared carrier authority; liveness must not reinterpret Python
/// annotations as proof that a value has no refcounted heap obligation.
pub(crate) fn compute_raw_scalars(func: &TirFunction) -> HashSet<ValueId> {
    analyze_non_heap_carriers(func)
}
