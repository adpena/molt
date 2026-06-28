use super::super::lir_context::LirLowerCtx;
use crate::wasm::body::WasmLirFallbackReason;
use molt_tir::tir::lir::LirOp;

pub(in crate::wasm::lir_fast) fn emit_lir_unsupported_marker(ctx: &mut LirLowerCtx, op: &LirOp) {
    for &operand in &op.tir_op.operands {
        ctx.emit_get(operand);
    }
    ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
    if let Some(result) = op.result_values.first() {
        ctx.emit_set(result.id);
    }
}
