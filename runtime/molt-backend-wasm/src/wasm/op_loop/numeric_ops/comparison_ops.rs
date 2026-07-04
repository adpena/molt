mod equality;
mod ordered;

use super::common::emit_boxed_binary_result;
use crate::OpIR;
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use crate::wasm_import_tracking::TrackedImportIds;
use crate::wasm_values::ConstantCache;
use std::collections::BTreeMap;
use wasm_encoder::Function;

#[allow(unused_variables)]
pub(super) fn emit_comparison_numeric_op(
    func: &mut Function,
    op: &OpIR,
    selection: WasmNumericRuntimeSelection,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    const_cache: &ConstantCache,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    known_raw_ints: &BTreeMap<u32, i64>,
) {
    match selection.op_loop_kind {
        WasmNumericOpLoopKind::Lt
        | WasmNumericOpLoopKind::Le
        | WasmNumericOpLoopKind::Gt
        | WasmNumericOpLoopKind::Ge => ordered::emit_ordered_compare_op(
            func,
            op,
            import_ids,
            locals,
            const_cache,
            scalar_plan,
            reloc_enabled,
            known_raw_ints,
            selection.op_loop_kind,
            selection.import,
        ),
        WasmNumericOpLoopKind::Eq | WasmNumericOpLoopKind::Ne => {
            equality::emit_equality_compare_op(
                func,
                op,
                import_ids,
                locals,
                scalar_plan,
                reloc_enabled,
                known_raw_ints,
                selection.op_loop_kind,
                selection.import,
            )
        }
        WasmNumericOpLoopKind::StringEq => emit_boxed_binary_result(
            func,
            op,
            import_ids,
            locals,
            selection.import,
            reloc_enabled,
        ),
        _ => unreachable!("non-comparison numeric selector routed to comparison emitter"),
    }
}
