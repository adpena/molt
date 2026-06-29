use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use super::call_abi::{
    LirRuntimeArg, emit_lir_runtime_call_with_args_and_result, emit_lir_runtime_result,
    positive_i64_attr, required_i64_attr,
};
use crate::wasm::body::WasmLirFallbackReason;
use molt_tir::tir::lir::LirOp;
use molt_tir::tir::ops::AttrValue;

pub(in crate::wasm::lir_fast) fn emit_lir_alloc(ctx: &mut LirLowerCtx, op: &LirOp) {
    if matches!(
        op.tir_op.attrs.get("arena_eligible"),
        Some(AttrValue::Bool(true))
    ) {
        ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
        emit_lir_runtime_result(ctx, op);
        return;
    }
    let size = required_i64_attr(op, "value", "Alloc");
    emit_lir_runtime_call_with_args_and_result(
        ctx,
        op,
        LirRuntimeCall::Alloc,
        &[LirRuntimeArg::I64Const(size)],
    );
}

pub(in crate::wasm::lir_fast) fn emit_lir_object_new_bound(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(&class_ref) = op.tir_op.operands.first() else {
        panic!("ObjectNewBound requires class operand");
    };
    if let Some(payload_size) = positive_i64_attr(op, "value") {
        emit_lir_runtime_call_with_args_and_result(
            ctx,
            op,
            LirRuntimeCall::ObjectNewBoundSized,
            &[
                LirRuntimeArg::BoxedOperand(class_ref),
                LirRuntimeArg::I64Const(payload_size),
            ],
        );
    } else {
        emit_lir_runtime_call_with_args_and_result(
            ctx,
            op,
            LirRuntimeCall::ObjectNewBound,
            &[LirRuntimeArg::BoxedOperand(class_ref)],
        );
    }
}
