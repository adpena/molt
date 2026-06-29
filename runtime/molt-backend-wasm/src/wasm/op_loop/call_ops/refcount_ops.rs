use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_refcount_call_op(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind.as_str() {
        "inc_ref" | "borrow" => {
            emit_inc_ref_like(call_ctx, func, op);
            CallOpEmission::Handled
        }
        "dec_ref" | "release" => {
            emit_dec_ref_like(call_ctx, func, op);
            CallOpEmission::Handled
        }
        _ => CallOpEmission::NotHandled,
    }
}

fn emit_inc_ref_like(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args_names = op.args.as_ref().expect("inc_ref/borrow args missing");
    let src_name = args_names
        .first()
        .expect("inc_ref/borrow requires one source arg");
    let src = call_ctx.locals[src_name];
    if !call_ctx.rc_skip_inc.contains(&call_ctx.rel_idx) {
        func.instruction(&Instruction::LocalGet(src));
        emit_call(
            func,
            call_ctx.reloc_enabled,
            call_ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IncRefObj],
        );
    }
    if let Some(out_name) = op.out.as_ref()
        && out_name != "none"
    {
        let out = call_ctx.locals[out_name];
        func.instruction(&Instruction::LocalGet(src));
        func.instruction(&Instruction::LocalSet(out));
    }
}

fn emit_dec_ref_like(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args_names = op.args.as_ref().expect("dec_ref/release args missing");
    let src_name = args_names
        .first()
        .expect("dec_ref/release requires one source arg");
    if call_ctx.rc_skip_inc.contains(&call_ctx.rel_idx)
        || call_ctx.rc_skip_dec.contains(src_name.as_str())
    {
        return;
    }
    let src = call_ctx.locals[src_name];
    func.instruction(&Instruction::LocalGet(src));
    emit_call(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::DecRefObj],
    );
    if let Some(out_name) = op.out.as_ref()
        && out_name != "none"
    {
        let out = call_ctx.locals[out_name];
        call_ctx.const_cache.emit_none(func);
        func.instruction(&Instruction::LocalSet(out));
    }
}
