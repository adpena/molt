use super::super::lir_context::LirLowerCtx;
use crate::wasm::lir_fast::LirRuntimeCall;
use molt_tir::tir::lir::{LirOp, LirRepr};

pub(super) fn emit_lir_refcount_op(ctx: &mut LirLowerCtx, op: &LirOp, call: LirRuntimeCall) {
    if let Some(&operand) = op.tir_op.operands.first()
        && matches!(ctx.repr_of(operand), LirRepr::DynBox | LirRepr::Ref64)
    {
        ctx.emit_get(operand);
        ctx.emit_runtime_call(call);
    }
}
