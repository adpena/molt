use super::super::super::lir_context::LirLowerCtx;
use super::super::boxing::emit_get_boxed_for_repr;
use crate::wasm::body::WasmLirFallbackReason;
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::values::ValueId;
use wasm_encoder::{BlockType, Instruction, ValType};

pub(in crate::wasm::lir_fast) fn emit_lir_checked_add(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    assert!(
        tir_op.operands.len() >= 2 && op.result_values.len() >= 2,
        "checked_add requires 2 operands and 2 results"
    );
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let sum = op.result_values[0].id;
    let flag = op.result_values[1].id;
    if ctx.repr_of(lhs) == LirRepr::I64 && ctx.repr_of(rhs) == LirRepr::I64 {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(Instruction::I64Add);
        ctx.emit_set(sum);
        ctx.emit_get(lhs);
        ctx.emit_get(sum);
        ctx.instructions.push(Instruction::I64Xor);
        ctx.emit_get(rhs);
        ctx.emit_get(sum);
        ctx.instructions.push(Instruction::I64Xor);
        ctx.instructions.push(Instruction::I64And);
        ctx.instructions.push(Instruction::I64Const(0));
        ctx.instructions.push(Instruction::I64LtS);
        ctx.emit_set(flag);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
        emit_get_boxed_for_repr(ctx, rhs);
        ctx.emit_bail_to_generic_path(WasmLirFallbackReason::BoxedCheckedArithmetic);
        ctx.emit_set(sum);
        ctx.instructions.push(Instruction::I32Const(0));
        ctx.emit_set(flag);
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_checked_mul(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    assert!(
        tir_op.operands.len() >= 2 && op.result_values.len() >= 2,
        "checked_mul requires 2 operands and 2 results"
    );
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let product = op.result_values[0].id;
    let flag = op.result_values[1].id;
    if ctx.repr_of(lhs) == LirRepr::I64
        && ctx.repr_of(rhs) == LirRepr::I64
        && op.result_values[0].repr == LirRepr::I64
    {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(Instruction::I64Mul);
        ctx.emit_set(product);
        emit_checked_mul_overflow_flag(ctx, lhs, rhs, product);
        ctx.emit_set(flag);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
        emit_get_boxed_for_repr(ctx, rhs);
        ctx.emit_bail_to_generic_path(WasmLirFallbackReason::BoxedCheckedArithmetic);
        ctx.emit_set(product);
        ctx.instructions.push(Instruction::I32Const(0));
        ctx.emit_set(flag);
    }
}

fn emit_checked_mul_overflow_flag(
    ctx: &mut LirLowerCtx,
    lhs: ValueId,
    rhs: ValueId,
    product: ValueId,
) {
    ctx.emit_get(lhs);
    ctx.instructions.push(Instruction::I64Const(0));
    ctx.instructions.push(Instruction::I64Eq);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I32)));
    ctx.instructions.push(Instruction::I32Const(0));
    ctx.instructions.push(Instruction::Else);

    ctx.emit_get(lhs);
    ctx.instructions.push(Instruction::I64Const(-1));
    ctx.instructions.push(Instruction::I64Eq);
    ctx.emit_get(rhs);
    ctx.instructions.push(Instruction::I64Const(i64::MIN));
    ctx.instructions.push(Instruction::I64Eq);
    ctx.instructions.push(Instruction::I32And);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I32)));
    ctx.instructions.push(Instruction::I32Const(1));
    ctx.instructions.push(Instruction::Else);
    ctx.emit_get(product);
    ctx.emit_get(lhs);
    ctx.instructions.push(Instruction::I64DivS);
    ctx.emit_get(rhs);
    ctx.instructions.push(Instruction::I64Ne);
    ctx.instructions.push(Instruction::End);

    ctx.instructions.push(Instruction::End);
}
