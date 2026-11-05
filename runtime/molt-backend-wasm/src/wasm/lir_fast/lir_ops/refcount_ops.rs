use super::super::lir_context::LirLowerCtx;
use crate::wasm::lir_fast::LirRuntimeCall;
use molt_tir::tir::lir::{LirOp, LirRepr};

pub(super) fn emit_lir_refcount_op(ctx: &mut LirLowerCtx, op: &LirOp, call: LirRuntimeCall) {
    if let Some(&operand) = op.tir_op.operands.first() {
        if matches!(ctx.repr_of(operand), LirRepr::DynBox | LirRepr::Ref64) {
            ctx.emit_get(operand);
            ctx.emit_runtime_call(call);
        }
        if let Some(result) = op.result_values.first() {
            if call == LirRuntimeCall::IncRefObj {
                if result.repr == LirRepr::Immortal64 {
                    super::super::lir_scalar::emit_get_boxed_for_repr(ctx, operand);
                } else {
                    ctx.emit_get(operand);
                }
            } else {
                super::super::lir_scalar::emit_box_none(ctx);
            }
            ctx.emit_set(result.id);
        }
    }
}
