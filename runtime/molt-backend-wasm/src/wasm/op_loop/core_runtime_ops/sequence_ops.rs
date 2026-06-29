use super::super::call_emit::{OpLoopRuntimeCallContext, emit_op_loop_local_prefix_call_id};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm_abi_generated::OpLoopRuntimeSinkSpec;
use crate::wasm_import_tracking::{TrackedImportIds, selected_import_id};
use crate::wasm_plan::wasm_specialized_container_import;
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
    arena_local: Option<u32>,
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
            let import_key = wasm_specialized_container_import(scalar_plan, op_idx, "index", op)
                .unwrap_or("index");
            let import_id =
                selected_import_id(import_ids, import_key, &func_ir.name, op.kind.as_str());
            emit_op_loop_local_prefix_call_id(
                &call_context,
                func,
                op,
                import_id,
                2,
                OpLoopRuntimeSinkSpec::ResultOrDrop,
            );
        }
        "store_index" => {
            // Dispatch: list_int / dict -> generic.
            let import_key =
                wasm_specialized_container_import(scalar_plan, op_idx, "store_index", op)
                    .unwrap_or("store_index");
            let import_id =
                selected_import_id(import_ids, import_key, &func_ir.name, op.kind.as_str());
            emit_op_loop_local_prefix_call_id(
                &call_context,
                func,
                op,
                import_id,
                3,
                OpLoopRuntimeSinkSpec::ResultOrDrop,
            );
        }
        _ => return false,
    }
    true
}
