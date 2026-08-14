use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

/// Lower representation aliases that are not ordinary local-slot moves.
/// `local_slot_ops` is the sole store/load/copy ownership authority.
pub(super) fn emit_conversion_call_op(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "box" | "unbox" | "cast" | "widen" => {
            emit_conversion_alias(call_ctx, func, op);
            CallOpEmission::Handled
        }
        _ => CallOpEmission::NotHandled,
    }
}

fn emit_conversion_alias(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args_names = op.args.as_ref().expect("conversion args missing");
    let src_name = args_names
        .first()
        .expect("conversion op requires one source arg");
    let src = call_ctx.locals[src_name];
    if let Some(out_name) = op.out.as_ref() {
        if out_name != "none" {
            func.instruction(&Instruction::LocalGet(src));
            emit_call(
                func,
                call_ctx.reloc_enabled,
                call_ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
            );
            func.instruction(&Instruction::LocalGet(src));
            let out = call_ctx.locals[out_name];
            func.instruction(&Instruction::LocalSet(out));
        } else {
            func.instruction(&Instruction::LocalGet(src));
            func.instruction(&Instruction::Drop);
        }
    } else {
        func.instruction(&Instruction::LocalGet(src));
        func.instruction(&Instruction::Drop);
    }
}
