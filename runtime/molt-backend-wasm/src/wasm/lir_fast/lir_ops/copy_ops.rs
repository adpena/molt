use super::super::lir_context::LirLowerCtx;
use super::super::lir_runtime_ops::{
    emit_lir_fixed_runtime_call, emit_lir_unsupported_marker, original_kind,
};
use super::super::lir_scalar::emit_get_boxed_for_repr;
use super::super::runtime_calls::lir_fixed_runtime_call;
use crate::wasm::lir_fast::LirRuntimeCall;
use molt_tir::tir::lir::LirOp;

pub(super) fn emit_lir_identity_copy(ctx: &mut LirLowerCtx, op: &LirOp) {
    if let (Some(&src), Some(result)) = (op.tir_op.operands.first(), op.result_values.first()) {
        ctx.emit_get(src);
        ctx.emit_set(result.id);
    }
}

pub(super) fn emit_lir_copy_or_original_kind(ctx: &mut LirLowerCtx, op: &LirOp) {
    match original_kind(op) {
        Some("binding_alias") => emit_lir_binding_alias(ctx, op),
        Some(kind)
            if crate::tir::op_kinds_generated::copy_kind_is_explicit_no_heap_move_table(kind) =>
        {
            emit_lir_identity_copy(ctx, op)
        }
        Some(kind) if let Some(runtime) = lir_fixed_runtime_call(kind) => {
            emit_lir_fixed_runtime_call(ctx, op, runtime)
        }
        Some(_) => emit_lir_unsupported_marker(ctx, op),
        None => emit_lir_identity_copy(ctx, op),
    }
}

fn emit_lir_binding_alias(ctx: &mut LirLowerCtx, op: &LirOp) {
    if let (Some(&src), Some(result)) = (op.tir_op.operands.first(), op.result_values.first()) {
        emit_get_boxed_for_repr(ctx, src);
        ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
        ctx.emit_get(src);
        ctx.emit_set(result.id);
    }
}
