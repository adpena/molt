use super::super::lir_context::LirLowerCtx;
use super::super::lir_runtime_ops::emit_lir_fixed_runtime_call;
use super::super::runtime_calls::lir_fixed_runtime_call;
use molt_tir::tir::lir::LirOp;

pub(super) fn emit_lir_generated_fixed_runtime_call(ctx: &mut LirLowerCtx, op: &LirOp) {
    let kind = crate::tir::op_kinds_generated::opcode_canonical_kind_table(op.tir_op.opcode);
    let runtime = lir_fixed_runtime_call(kind)
        .unwrap_or_else(|| panic!("missing generated WASM LIR fixed runtime call for {kind}"));
    assert!(
        op.tir_op.operands.len() >= runtime.operand_count,
        "generated WASM LIR fixed runtime call for {kind} needs {} operands, got {}",
        runtime.operand_count,
        op.tir_op.operands.len()
    );
    emit_lir_fixed_runtime_call(ctx, op, runtime);
}
