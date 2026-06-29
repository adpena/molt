use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm_binary::emit_call;
use crate::wasm_plan::gpu_runtime_call_symbol;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_gpu_call_op(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "gpu_thread_id" | "gpu_block_id" | "gpu_block_dim" | "gpu_grid_dim" | "gpu_barrier" => {
            let runtime_name =
                gpu_runtime_call_symbol(op.kind.as_str()).expect("gpu runtime symbol");
            let import_name = runtime_name.strip_prefix("molt_").unwrap_or(runtime_name);
            let out = call_ctx.locals[op.out.as_ref().expect("gpu op result missing")];
            emit_call(
                func,
                call_ctx.reloc_enabled,
                call_ctx.import_ids[import_name],
            );
            func.instruction(&Instruction::LocalSet(out));
            CallOpEmission::Handled
        }
        _ => CallOpEmission::NotHandled,
    }
}
