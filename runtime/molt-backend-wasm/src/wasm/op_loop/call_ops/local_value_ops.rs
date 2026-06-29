use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_local_value_call_op(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "store_var" => {
            emit_store_var(call_ctx, func, op);
            CallOpEmission::Handled
        }
        "load_var" | "copy_var" | "copy" | "identity_alias" | "binding_alias" => {
            emit_load_or_copy(call_ctx, func, op);
            CallOpEmission::Handled
        }
        "box" | "unbox" | "cast" | "widen" => {
            emit_conversion_alias(call_ctx, func, op);
            CallOpEmission::Handled
        }
        _ => CallOpEmission::NotHandled,
    }
}

fn emit_store_var(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args_names = op.args.as_ref().expect("store_var args missing");
    let src_name = args_names
        .first()
        .expect("store_var requires one source arg");
    let src = call_ctx.locals[src_name];
    let dst_name = op
        .var
        .as_ref()
        .or(op.out.as_ref())
        .expect("store_var requires destination");
    let dst = call_ctx.locals[dst_name];
    func.instruction(&Instruction::LocalGet(src));
    func.instruction(&Instruction::LocalSet(dst));
}

fn emit_load_or_copy(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let src_name = op
        .var
        .as_ref()
        .or_else(|| op.args.as_ref().and_then(|args| args.first()))
        .expect("load_var/copy_var requires source");
    let src = call_ctx.locals[src_name];
    if let Some(out_name) = op.out.as_ref()
        && out_name != "none"
    {
        func.instruction(&Instruction::LocalGet(src));
        emit_call(
            func,
            call_ctx.reloc_enabled,
            call_ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
        );
        let out = call_ctx.locals[out_name];
        func.instruction(&Instruction::LocalGet(src));
        func.instruction(&Instruction::LocalSet(out));
    }
}

fn emit_conversion_alias(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args_names = op.args.as_ref().expect("conversion args missing");
    let src_name = args_names
        .first()
        .expect("conversion op requires one source arg");
    let src = call_ctx.locals[src_name];
    func.instruction(&Instruction::LocalGet(src));
    if let Some(out_name) = op.out.as_ref() {
        if out_name != "none" {
            emit_call(
                func,
                call_ctx.reloc_enabled,
                call_ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
            );
            func.instruction(&Instruction::LocalGet(src));
            let out = call_ctx.locals[out_name];
            func.instruction(&Instruction::LocalSet(out));
        } else {
            func.instruction(&Instruction::Drop);
        }
    } else {
        func.instruction(&Instruction::Drop);
    }
}
