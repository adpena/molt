use super::super::super::call_emit::emit_op_loop_local_prefix_call;
use super::AggregateRuntimeContext;
use crate::OpIR;
use crate::wasm::container_runtime_select::selected_container_runtime_import;
use crate::wasm_abi_generated::WasmRuntimeImport;
use wasm_encoder::Function;

pub(super) fn emit_container_query_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let call_context = ctx.op_loop_call_context();

    match op.kind.as_str() {
        "contains" => {
            let import_key =
                selected_container_runtime_import(ctx.scalar_plan, ctx.op_idx, "contains", op)
                    .unwrap_or(WasmRuntimeImport::Contains);
            emit_op_loop_local_prefix_call(
                &call_context,
                func,
                op,
                import_key,
                2,
                &ctx.func_ir.name,
            );
        }
        "len" => {
            // Dispatch to specialized fast-path len when container
            // type is known, skipping the 18-type dispatch.
            let import_key =
                selected_container_runtime_import(ctx.scalar_plan, ctx.op_idx, "len", op)
                    .unwrap_or(WasmRuntimeImport::Len);
            emit_op_loop_local_prefix_call(
                &call_context,
                func,
                op,
                import_key,
                1,
                &ctx.func_ir.name,
            );
        }
        _ => return false,
    }
    true
}
