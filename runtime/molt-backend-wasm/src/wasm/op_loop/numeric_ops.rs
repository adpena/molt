use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use crate::OpIR;
use std::collections::BTreeMap;
use wasm_encoder::Function;

#[path = "numeric_ops/additive_ops.rs"]
mod additive_ops;
#[path = "numeric_ops/bitwise_ops.rs"]
mod bitwise_ops;
#[path = "numeric_ops/comparison_ops.rs"]
mod comparison_ops;
#[path = "numeric_ops/division_ops.rs"]
mod division_ops;
#[path = "numeric_ops/vector_reduction_ops.rs"]
mod vector_reduction_ops;

#[allow(unused_variables)]
pub(super) fn emit_numeric_op(
    func: &mut Function,
    op: &OpIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) -> bool {
    if additive_ops::emit_additive_numeric_op(
        func,
        op,
        import_ids,
        locals,
        const_cache,
        scalar_plan,
        reloc_enabled,
        known_raw_ints,
    ) {
        return true;
    }
    if vector_reduction_ops::emit_vector_reduction_numeric_op(
        func,
        op,
        import_ids,
        locals,
        const_cache,
        scalar_plan,
        reloc_enabled,
        known_raw_ints,
    ) {
        return true;
    }
    if bitwise_ops::emit_bitwise_numeric_op(
        func,
        op,
        import_ids,
        locals,
        const_cache,
        scalar_plan,
        reloc_enabled,
        known_raw_ints,
    ) {
        return true;
    }
    if division_ops::emit_division_numeric_op(
        func,
        op,
        import_ids,
        locals,
        const_cache,
        scalar_plan,
        reloc_enabled,
        known_raw_ints,
    ) {
        return true;
    }
    if comparison_ops::emit_comparison_numeric_op(
        func,
        op,
        import_ids,
        locals,
        const_cache,
        scalar_plan,
        reloc_enabled,
        known_raw_ints,
    ) {
        return true;
    }
    false
}
