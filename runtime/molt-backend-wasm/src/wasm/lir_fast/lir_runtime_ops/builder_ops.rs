use super::super::lir_context::LirLowerCtx;
use super::super::lir_scalar::emit_get_boxed_for_repr;
use super::super::runtime_calls::LirRuntimeCall;
use super::call_abi::{
    LirRuntimeArg, emit_lir_runtime_call_with_args, emit_lir_runtime_discard,
    emit_lir_runtime_result,
};
use molt_tir::tir::lir::LirOp;
use wasm_encoder::{Instruction, MemArg, ValType};

const I64_ALIGN_EXPONENT: u32 = 3;

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

pub(in crate::wasm::lir_fast) fn emit_lir_build_list(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_fixed_sequence(ctx, op, LirRuntimeCall::ListFromValues);
}

pub(in crate::wasm::lir_fast) fn emit_lir_build_tuple(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_fixed_sequence(ctx, op, LirRuntimeCall::TupleFromValues);
}

/// Fixed-arity tuples pass their operand words to the one runtime constructor.
/// The range is a scratch allocation private to this operation: the runtime
/// copies and retains every word before it is freed, so there is no builder
/// owner, per-element failure branch or shared buffer.
fn emit_lir_fixed_sequence(ctx: &mut LirLowerCtx, op: &LirOp, constructor: LirRuntimeCall) {
    assert!(
        !op.result_values.is_empty(),
        "fixed sequence requires result"
    );
    // A fallible physical box must precede the scratch acquisition. Otherwise
    // its exception branch would strand the range.
    let operands = prepare_builder_operands(ctx, op);
    let count = i64::try_from(operands.len()).expect("sequence arity exceeds the i64 ABI");
    if operands.is_empty() {
        emit_lir_runtime_call_with_args(
            ctx,
            constructor,
            &[LirRuntimeArg::I64Const(0), LirRuntimeArg::I64Const(0)],
        );
        emit_lir_runtime_result(ctx, op, constructor);
        return;
    }
    let bytes = count
        .checked_mul(8)
        .expect("sequence operand range exceeds the i64 scratch ABI");
    let scratch = ctx.alloc_scratch_local(ValType::I64);
    ctx.instructions.push(Instruction::I64Const(bytes));
    ctx.emit_runtime_call(LirRuntimeCall::ScratchAlloc);
    ctx.instructions.push(Instruction::LocalTee(scratch));
    ctx.instructions.push(Instruction::I64Eqz);
    // ScratchAlloc owns MemoryError publication; the result stays None.
    ctx.branch_to_operation_cleanup_if();
    for (index, operand) in operands.into_iter().enumerate() {
        ctx.instructions.push(Instruction::LocalGet(scratch));
        ctx.instructions.push(Instruction::I32WrapI64);
        ctx.instructions.push(Instruction::LocalGet(operand));
        ctx.instructions
            .push(Instruction::I64Store(word_memarg(index)));
    }
    ctx.instructions.push(Instruction::LocalGet(scratch));
    ctx.instructions.push(Instruction::I64Const(count));
    ctx.emit_runtime_call(constructor);
    // The runtime copied the range; free it while the sequence waits on the stack.
    ctx.instructions.push(Instruction::LocalGet(scratch));
    ctx.instructions.push(Instruction::I64Const(bytes));
    ctx.emit_runtime_call(LirRuntimeCall::ScratchFree);
    emit_lir_runtime_result(ctx, op, constructor);
}

fn word_memarg(index: usize) -> MemArg {
    MemArg {
        offset: (index as u64) * 8,
        align: I64_ALIGN_EXPONENT,
        memory_index: 0,
    }
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
