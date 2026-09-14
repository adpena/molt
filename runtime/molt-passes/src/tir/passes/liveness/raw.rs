use std::collections::HashSet;

use crate::representation_facts::{non_heap_values_for, value_range_for};
use crate::tir::function::TirFunction;
use crate::tir::values::ValueId;

/// Project the shared carrier authority; liveness must not reinterpret Python
/// annotations as proof that a value has no refcounted heap obligation.
pub(super) fn compute_raw_scalars(func: &TirFunction) -> HashSet<ValueId> {
    non_heap_values_for(func, &value_range_for(func))
}
