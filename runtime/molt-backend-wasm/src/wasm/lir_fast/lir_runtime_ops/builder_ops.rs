use super::super::lir_context::LirLowerCtx;
use super::super::lir_scalar::emit_get_boxed_for_repr;
use super::super::runtime_calls::LirRuntimeCall;
use super::call_abi::{
    LirRuntimeArg, emit_lir_runtime_call_with_args, emit_lir_runtime_discard,
    emit_lir_runtime_result,
};
use molt_codegen_abi::box_int_bits;
use molt_tir::tir::lir::LirOp;
use wasm_encoder::{Instruction, ValType};

/// Materialize borrowed element views before acquiring an aggregate resource.
/// They stay owned by the operation; successful append retains each element.
fn prepare_builder_operands(ctx: &mut LirLowerCtx, op: &LirOp) -> Vec<u32> {
    op.tir_op
        .operands
        .iter()
        .map(|&value| {
            emit_get_boxed_for_repr(ctx, value);
            let local = ctx.alloc_scratch_local(ValType::I64);
            ctx.instructions.push(Instruction::LocalSet(local));
            local
        })
        .collect()
}

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
    assert!(
        !op.result_values.is_empty(),
        "sequence builder op requires result"
    );
    let operands = prepare_builder_operands(ctx, op);
    let owner = ctx.alloc_operation_owner();
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::ListBuilderNew,
        &[LirRuntimeArg::I64Const(box_int_bits(
            op.tir_op.operands.len() as i64,
        ))],
    );
    ctx.instructions.push(Instruction::LocalSet(owner));
    ctx.guard_operation_exception();

    for operand in operands {
        ctx.instructions.push(Instruction::LocalGet(owner));
        ctx.instructions.push(Instruction::LocalGet(operand));
        ctx.emit_runtime_call(LirRuntimeCall::ListBuilderAppend);
        ctx.branch_to_operation_cleanup_if();
    }

    ctx.instructions.push(Instruction::LocalGet(owner));
    ctx.emit_runtime_call(finish.finish_call());
    // Finish consumes the builder on both success and failure.
    ctx.forget_operation_owner(owner);
    emit_lir_runtime_result(ctx, op, finish.finish_call());
}

pub(in crate::wasm::lir_fast) fn emit_lir_build_dict(ctx: &mut LirLowerCtx, op: &LirOp) {
    if !op.tir_op.operands.len().is_multiple_of(2) {
        panic!("BuildDict requires an even key/value operand count");
    }
    let Some(result) = op.result_values.first() else {
        panic!("BuildDict requires result");
    };
    let out = result.id;
    let operands = prepare_builder_operands(ctx, op);
    let owner = ctx.alloc_operation_owner();
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::DictNew,
        &[LirRuntimeArg::I64Const(
            (op.tir_op.operands.len() / 2) as i64,
        )],
    );
    ctx.instructions.push(Instruction::LocalSet(owner));
    ctx.guard_operation_exception();

    for pair in operands.chunks(2) {
        ctx.instructions.push(Instruction::LocalGet(owner));
        ctx.instructions.push(Instruction::LocalGet(pair[0]));
        ctx.instructions.push(Instruction::LocalGet(pair[1]));
        ctx.emit_runtime_call(LirRuntimeCall::DictSet);
        // DictSet borrows the dictionary and can return None on failure.
        // Never overwrite the only owner with its status/result word.
        emit_lir_runtime_discard(ctx, LirRuntimeCall::DictSet);
        ctx.guard_operation_exception();
    }
    ctx.instructions.push(Instruction::LocalGet(owner));
    ctx.emit_set(out);
    ctx.forget_operation_owner(owner);
}

pub(in crate::wasm::lir_fast) fn emit_lir_build_set(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(result) = op.result_values.first() else {
        panic!("BuildSet requires result");
    };
    let out = result.id;
    let operands = prepare_builder_operands(ctx, op);
    let owner = ctx.alloc_operation_owner();
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::SetNew,
        &[LirRuntimeArg::I64Const(op.tir_op.operands.len() as i64)],
    );
    ctx.instructions.push(Instruction::LocalSet(owner));
    ctx.guard_operation_exception();

    for operand in operands {
        ctx.instructions.push(Instruction::LocalGet(owner));
        ctx.instructions.push(Instruction::LocalGet(operand));
        ctx.emit_runtime_call(LirRuntimeCall::SetAdd);
        emit_lir_runtime_discard(ctx, LirRuntimeCall::SetAdd);
        ctx.guard_operation_exception();
    }
    ctx.instructions.push(Instruction::LocalGet(owner));
    ctx.emit_set(out);
    ctx.forget_operation_owner(owner);
}
