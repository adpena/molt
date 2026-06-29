use super::super::super::call_emit::emit_op_loop_local_prefix_call_id;
use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm_abi_generated::OpLoopRuntimeSinkSpec;
use crate::wasm_import_tracking::selected_import_id;
use crate::wasm_plan::wasm_specialized_container_import;
use wasm_encoder::Function;

pub(super) fn emit_container_query_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let call_context = ctx.op_loop_call_context();

    match op.kind.as_str() {
        "contains" => {
            let import_key =
                wasm_specialized_container_import(ctx.scalar_plan, ctx.op_idx, "contains", op)
                    .unwrap_or("contains");
            let import_id =
                selected_import_id(import_ids, import_key, &ctx.func_ir.name, op.kind.as_str());
            emit_op_loop_local_prefix_call_id(
                &call_context,
                func,
                op,
                import_id,
                2,
                OpLoopRuntimeSinkSpec::ResultOrDrop,
            );
        }
        "len" => {
            // Dispatch to specialized fast-path len when container
            // type is known, skipping the 18-type dispatch.
            let import_key =
                wasm_specialized_container_import(ctx.scalar_plan, ctx.op_idx, "len", op)
                    .unwrap_or("len");
            let import_id =
                selected_import_id(import_ids, import_key, &ctx.func_ir.name, op.kind.as_str());
            emit_op_loop_local_prefix_call_id(
                &call_context,
                func,
                op,
                import_id,
                1,
                OpLoopRuntimeSinkSpec::ResultOrDrop,
            );
        }
        _ => return false,
    }
    true
}
