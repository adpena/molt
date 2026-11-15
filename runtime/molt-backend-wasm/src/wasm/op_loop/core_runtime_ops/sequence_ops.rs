use super::super::call_emit::{
    OpLoopRuntimeCallContext, emit_op_loop_local_prefix_body_call, emit_op_loop_local_prefix_call,
};
use crate::representation_plan::ScalarRepresentationPlan;
use crate::wasm::WasmFrameLocals;
use crate::wasm::container_runtime_select::selected_container_runtime_import;
use crate::wasm_abi_generated::WasmRuntimeImport;
use crate::wasm_binary::Function;
use crate::wasm_import_tracking::TrackedImportIds;
use crate::{FunctionIR, OpIR};

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
        guard_profile_local: None,
    };

    match op.kind {
        molt_ir::OpKind::Index => {
            // Dispatch: list_int / dict / tuple -> their entries; the generic
            // read is the shared list body, whose miss is `molt_index`.
            match selected_container_runtime_import(scalar_plan, op_idx, molt_ir::OpKind::Index, op)
            {
                Some(import_key) => emit_op_loop_local_prefix_call(
                    &call_context,
                    func,
                    op,
                    import_key,
                    2,
                    &func_ir.name,
                ),
                None => emit_op_loop_local_prefix_body_call(
                    &call_context,
                    func,
                    op,
                    import_ids.list_item_bodies().get(),
                    WasmRuntimeImport::Index,
                    2,
                ),
            }
        }
        molt_ir::OpKind::StoreIndex => {
            // Dispatch: list_int / dict -> their entries; the generic store is
            // the shared list body, whose miss is `molt_store_index`.
            match selected_container_runtime_import(
                scalar_plan,
                op_idx,
                molt_ir::OpKind::StoreIndex,
                op,
            ) {
                Some(import_key) => emit_op_loop_local_prefix_call(
                    &call_context,
                    func,
                    op,
                    import_key,
                    3,
                    &func_ir.name,
                ),
                None => emit_op_loop_local_prefix_body_call(
                    &call_context,
                    func,
                    op,
                    import_ids.list_item_bodies().set(),
                    WasmRuntimeImport::StoreIndex,
                    3,
                ),
            }
        }
        _ => return false,
    }
    true
}
