use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm_binary::Function;
use wasm_encoder::Instruction;

pub(super) fn emit_refcount_call_op(
    call_ctx: &CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    match op.kind {
        molt_ir::OpKind::IncRef | molt_ir::OpKind::Borrow => {
            emit_inc_ref_like(call_ctx, func, op);
            CallOpEmission::Handled
        }
        molt_ir::OpKind::DecRef | molt_ir::OpKind::Release | molt_ir::OpKind::DelBoundary => {
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
    if !call_ctx.rc_skip_inc.contains(&call_ctx.op_idx)
        && !call_ctx
            .frame
            .scalar_plan()
            .boxed_refcount_is_noop(src_name)
    {
        func.instruction(&Instruction::LocalGet(src));
        crate::wasm::refcount::emit_refcount(
            func,
            call_ctx.reloc_enabled,
            call_ctx.import_ids,
            call_ctx.locals.refcount(),
            molt_codegen_abi::inline_refcount::RefcountOperation::Retain,
        );
    }
    if let Some(out) = call_ctx.locals.bound_op_result_slot(op) {
        func.instruction(&Instruction::LocalGet(src));
        func.instruction(&Instruction::LocalSet(out));
    }
}

fn emit_dec_ref_like(call_ctx: &CallOpContext<'_, '_, '_>, func: &mut Function, op: &OpIR) {
    let args_names = op
        .args
        .as_ref()
        .expect("dec_ref/release/del_boundary args missing");
    let src_name = args_names
        .first()
        .expect("dec_ref/release/del_boundary requires one source arg");
    if call_ctx.rc_skip_inc.contains(&call_ctx.op_idx)
        || call_ctx.rc_skip_dec.contains(src_name.as_str())
        || call_ctx
            .frame
            .scalar_plan()
            .boxed_refcount_is_noop(src_name)
    {
        if let Some(out) = call_ctx.locals.bound_op_result_slot(op) {
            call_ctx.const_cache.emit_none(func);
            func.instruction(&Instruction::LocalSet(out));
        }
        return;
    }
    let src = call_ctx.locals[src_name];
    func.instruction(&Instruction::LocalGet(src));
    crate::wasm::refcount::emit_refcount(
        func,
        call_ctx.reloc_enabled,
        call_ctx.import_ids,
        call_ctx.locals.refcount(),
        molt_codegen_abi::inline_refcount::RefcountOperation::Release,
    );
    if let Some(out) = call_ctx.locals.bound_op_result_slot(op) {
        call_ctx.const_cache.emit_none(func);
        func.instruction(&Instruction::LocalSet(out));
    }
}
