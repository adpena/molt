use std::collections::HashSet;

use crate::repr::Repr;
use crate::representation_facts::raw_i64_carrier_values_for;
use crate::tir::function::TirFunction;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Floor a value's `TirType` to its representation and test whether that carrier
/// holds no refcounted heap obligation (Bool / FloatUnboxed / None / Never).
/// Values with no type fact floor to `DynBox` (heap-carrying) — conservative: at
/// worst a redundant drop that the runtime fast-paths.
fn carrier_is_non_heap_by_type(ty: &TirType) -> bool {
    matches!(ty, TirType::None)
        || matches!(
            Repr::default_for(ty),
            Repr::Bool | Repr::FloatUnboxed | Repr::Never
        )
}

/// The set of values whose carrier holds no refcounted heap obligation: the
/// value-range / CheckedAdd / GPU-index RawI64Safe set, plus every value whose
/// `TirType` floors to Bool / FloatUnboxed / None / Never.
pub(super) fn compute_raw_scalars(func: &TirFunction) -> HashSet<ValueId> {
    let scev = crate::tir::passes::scev::compute_scev(func);
    let vr = crate::tir::passes::value_range::compute_value_range(func, &scev);
    let mut raw = raw_i64_carrier_values_for(func, &vr);

    // Add the by-type non-heap carriers (bool / float / None / never). We must visit
    // every value the function defines (block args and op results).
    let type_of = |id: ValueId| -> Option<&TirType> { func.value_types.get(&id) };
    for block in func.blocks.values() {
        for arg in &block.args {
            // Block args carry their own type on `TirValue`; the function-owned
            // `value_types` mirror may also hold it. Prefer the arg's own type.
            if carrier_is_non_heap_by_type(&arg.ty) {
                raw.insert(arg.id);
            }
        }
        for op in &block.ops {
            for &res in &op.results {
                if let Some(ty) = type_of(res)
                    && carrier_is_non_heap_by_type(ty)
                {
                    raw.insert(res);
                }
            }
        }
    }
    raw
}
