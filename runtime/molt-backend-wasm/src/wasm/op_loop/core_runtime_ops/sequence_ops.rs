use super::super::call_emit::{OpLoopRuntimeCallContext, emit_op_loop_local_prefix_call};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm::container_runtime_select::selected_container_runtime_import;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::{FunctionIR, OpIR};
use wasm_encoder::Function;

#[allow(unused_variables)]
pub(super) fn emit_sequence_runtime_op(
    func: &mut Function,
    op: &OpIR,
    func_ir: &FunctionIR,
    import_ids: &TrackedImportIds,
    locals: &WasmFrameLocals,
    scalar_plan: &ScalarRepresentationPlan,
    reloc_enabled: bool,
    ops: &[OpIR],
    op_idx: usize,
) -> bool {
    let call_context = OpLoopRuntimeCallContext {
        import_ids,
        locals,
        reloc_enabled,
    };

    match op.kind.as_str() {
        "index" => {
            // Dispatch: list_int / dict / tuple -> generic.
            let import_key = selected_container_runtime_import(scalar_plan, op_idx, "index", op)
                .unwrap_or(WasmRuntimeImport::Index);
            emit_op_loop_local_prefix_call(&call_context, func, op, import_key, 2, &func_ir.name);
        }
        "store_index" => {
            // Dispatch: list_int / dict -> generic.
            let import_key =
                selected_container_runtime_import(scalar_plan, op_idx, "store_index", op)
                    .unwrap_or(WasmRuntimeImport::StoreIndex);
            emit_op_loop_local_prefix_call(&call_context, func, op, import_key, 3, &func_ir.name);
        }
        _ => return false,
    }
    true
}
