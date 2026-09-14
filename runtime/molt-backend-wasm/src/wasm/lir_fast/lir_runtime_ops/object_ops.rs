use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use super::call_abi::{
    LirRuntimeArg, emit_lir_runtime_call_with_args_and_result, required_i64_attr,
};
use molt_tir::tir::lir::LirOp;

pub(in crate::wasm::lir_fast) fn emit_lir_alloc(ctx: &mut LirLowerCtx, op: &LirOp) {
    if op.tir_op.attrs.contains_key("arena_eligible") {
        panic!(
            "{}",
            crate::tir::target_info::COMPILER_ARENA_PLACEMENT_UNSUPPORTED
        );
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
    emit_lir_runtime_call_with_args_and_result(
        ctx,
        op,
        LirRuntimeCall::ObjectNewBound,
        &[LirRuntimeArg::BoxedOperand(class_ref)],
    );
}
