use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use super::call_abi::{
    LirRuntimeArg, emit_lir_runtime_call_with_args, emit_lir_runtime_call_with_result,
};
use molt_codegen_abi::box_int_bits;
use molt_tir::tir::lir::LirOp;
use wasm_encoder::Instruction;

#[derive(Clone, Copy)]
pub(in crate::wasm::lir_fast) enum LirSequenceBuilderFinish {
    List,
    Tuple,
}

impl LirSequenceBuilderFinish {
    const fn finish_call(self) -> LirRuntimeCall {
        match self {
            Self::List => LirRuntimeCall::ListBuilderFinish,
            Self::Tuple => LirRuntimeCall::TupleBuilderFinish,
        }
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_sequence_builder(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    finish: LirSequenceBuilderFinish,
) {
    let Some(result) = op.result_values.first() else {
        panic!("sequence builder op requires result");
    };
    let out = result.id;
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::ListBuilderNew,
        &[LirRuntimeArg::I64Const(box_int_bits(
            op.tir_op.operands.len() as i64,
        ))],
    );
    ctx.emit_set(out);

    for &operand in &op.tir_op.operands {
        ctx.emit_get(out);
        LirRuntimeArg::BoxedOperand(operand).emit(ctx);
        ctx.emit_runtime_call(LirRuntimeCall::ListBuilderAppend);
    }

    ctx.emit_get(out);
    emit_lir_runtime_call_with_result(ctx, op, finish.finish_call());
}

pub(in crate::wasm::lir_fast) fn emit_lir_build_dict(ctx: &mut LirLowerCtx, op: &LirOp) {
    if op.tir_op.operands.len() % 2 != 0 {
        panic!("BuildDict requires an even key/value operand count");
    }
    let Some(result) = op.result_values.first() else {
        panic!("BuildDict requires result");
    };
    let out = result.id;
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::DictNew,
        &[LirRuntimeArg::I64Const(
            (op.tir_op.operands.len() / 2) as i64,
        )],
    );
    ctx.emit_set(out);

    for pair in op.tir_op.operands.chunks(2) {
        ctx.emit_get(out);
        LirRuntimeArg::BoxedOperand(pair[0]).emit(ctx);
        LirRuntimeArg::BoxedOperand(pair[1]).emit(ctx);
        ctx.emit_runtime_call(LirRuntimeCall::DictSet);
        ctx.emit_set(out);
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_build_set(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(result) = op.result_values.first() else {
        panic!("BuildSet requires result");
    };
    let out = result.id;
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::SetNew,
        &[LirRuntimeArg::I64Const(op.tir_op.operands.len() as i64)],
    );
    ctx.emit_set(out);

    for &operand in &op.tir_op.operands {
        ctx.emit_get(out);
        LirRuntimeArg::BoxedOperand(operand).emit(ctx);
        ctx.emit_runtime_call(LirRuntimeCall::SetAdd);
        ctx.instructions.push(Instruction::Drop);
    }
}
