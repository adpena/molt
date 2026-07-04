use super::super::super::lir_context::LirLowerCtx;
use super::super::super::runtime_calls::LirRuntimeCall;
use super::super::boxing::emit_get_boxed_for_repr;
use super::predicate::emit_lir_truthiness_i32;
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::Instruction;

pub(in crate::wasm::lir_fast) fn emit_lir_not(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first()) {
        if result.repr == LirRepr::Bool1 {
            emit_lir_truthiness_i32(ctx, src);
            ctx.instructions.push(Instruction::I32Eqz);
        } else {
            emit_get_boxed_for_repr(ctx, src);
            ctx.emit_runtime_call(LirRuntimeCall::Not);
        }
        ctx.emit_set(result.id);
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_bool(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first()) {
        emit_lir_truthiness_i32(ctx, src);
        ctx.emit_set(result.id);
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_truthy_cond_builtin(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_bool(ctx, op);
}
