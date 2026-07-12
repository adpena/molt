use super::super::lir_context::LirLowerCtx;
use super::super::lir_scalar::emit_get_boxed_for_repr;
use crate::wasm::lir_fast::LirRuntimeCall;
use molt_tir::tir::lir::LirOp;

pub(super) fn emit_lir_refcount_op(ctx: &mut LirLowerCtx, op: &LirOp, call: LirRuntimeCall) {
    if let Some(&operand) = op.tir_op.operands.first() {
        emit_get_boxed_for_repr(ctx, operand);
        ctx.emit_runtime_call(call);
    }
}
