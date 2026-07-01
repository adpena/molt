use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use super::call_abi::{
    LirRuntimeArg, emit_lir_runtime_call_with_args_and_result, required_i64_attr,
};
use molt_tir::tir::lir::LirOp;

pub(in crate::wasm::lir_fast) fn emit_lir_closure_load(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(&closure) = op.tir_op.operands.first() else {
        panic!("ClosureLoad requires closure operand");
    };
    let offset = required_i64_attr(op, "value", "ClosureLoad");
    emit_lir_runtime_call_with_args_and_result(
        ctx,
        op,
        LirRuntimeCall::ClosureLoad,
        &[
            LirRuntimeArg::ResolvedPtrBits64(closure),
            LirRuntimeArg::I64Const(offset),
        ],
    );
}

pub(in crate::wasm::lir_fast) fn emit_lir_closure_store(ctx: &mut LirLowerCtx, op: &LirOp) {
    if op.tir_op.operands.len() < 2 {
        panic!("ClosureStore requires closure and value operands");
    }
    let offset = required_i64_attr(op, "value", "ClosureStore");
    emit_lir_runtime_call_with_args_and_result(
        ctx,
        op,
        LirRuntimeCall::ClosureStore,
        &[
            LirRuntimeArg::ResolvedPtrBits64(op.tir_op.operands[0]),
            LirRuntimeArg::I64Const(offset),
            LirRuntimeArg::BoxedOperand(op.tir_op.operands[1]),
        ],
    );
}
