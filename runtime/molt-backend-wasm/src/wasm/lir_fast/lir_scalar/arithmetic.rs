use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use super::boxing::{emit_box_inline_i64, emit_box_none, emit_get_boxed_for_repr};
use crate::wasm::body::WasmLirFallbackReason;
use molt_codegen_abi::{INT_MAX_INLINE as INLINE_INT_MAX, INT_MIN_INLINE as INLINE_INT_MIN};
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::ops::AttrValue;
use molt_tir::tir::values::ValueId;
use wasm_encoder::{BlockType, Instruction, ValType};

#[derive(Clone, Copy)]
pub(in crate::wasm::lir_fast) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
}

#[derive(Clone, Copy)]
pub(in crate::wasm::lir_fast) enum UnaryOp {
    Neg,
}

fn raw_i64_arith_instruction(arith: ArithOp) -> Instruction<'static> {
    match arith {
        ArithOp::Add => Instruction::I64Add,
        ArithOp::Sub => Instruction::I64Sub,
        ArithOp::Mul => Instruction::I64Mul,
        ArithOp::Div | ArithOp::FloorDiv => Instruction::I64DivS,
        ArithOp::Mod => Instruction::I64RemS,
    }
}

fn boxed_arith_runtime_call(arith: ArithOp) -> LirRuntimeCall {
    match arith {
        ArithOp::Add => LirRuntimeCall::Add,
        ArithOp::Sub => LirRuntimeCall::Sub,
        ArithOp::Mul => LirRuntimeCall::Mul,
        ArithOp::Div => LirRuntimeCall::Div,
        ArithOp::FloorDiv => LirRuntimeCall::FloorDiv,
        ArithOp::Mod => LirRuntimeCall::Mod,
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_binary_arith(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    arith: ArithOp,
) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let dst = op.result_values[0].id;
    if matches!(
        tir_op.attrs.get("lir.checked_overflow"),
        Some(AttrValue::Bool(true))
    ) {
        let main = op.result_values[0].id;
        let overflow_box = op.result_values[1].id;
        let overflow_flag = op.result_values[2].id;

        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(raw_i64_arith_instruction(arith));
        ctx.emit_set(main);

        ctx.emit_get(main);
        ctx.instructions.push(Instruction::I64Const(INLINE_INT_MIN));
        ctx.instructions.push(Instruction::I64GeS);
        ctx.emit_get(main);
        ctx.instructions.push(Instruction::I64Const(INLINE_INT_MAX));
        ctx.instructions.push(Instruction::I64LeS);
        ctx.instructions.push(Instruction::I32And);
        ctx.instructions.push(Instruction::If(BlockType::Empty));
        emit_box_none(ctx);
        ctx.emit_set(overflow_box);
        ctx.instructions.push(Instruction::I32Const(0));
        ctx.emit_set(overflow_flag);
        ctx.instructions.push(Instruction::Else);
        // Inline boxing is sound here because the checked-triple gate only fires
        // when both operands are proven inside the 47-bit inline window.
        emit_box_inline_i64(ctx, lhs);
        emit_box_inline_i64(ctx, rhs);
        ctx.emit_runtime_call(boxed_arith_runtime_call(arith));
        ctx.emit_set(overflow_box);
        ctx.instructions.push(Instruction::I32Const(1));
        ctx.emit_set(overflow_flag);
        ctx.instructions.push(Instruction::End);
        return;
    }
    let lhs_repr = ctx.repr_of(lhs);
    let rhs_repr = ctx.repr_of(rhs);
    let boxed_dispatch = matches!(
        tir_op.attrs.get("lir.boxed_dispatch"),
        Some(AttrValue::Bool(true))
    );
    let result_repr = op.result_values[0].repr;
    match (lhs_repr, rhs_repr) {
        (LirRepr::I64, LirRepr::I64) if result_repr == LirRepr::I64 && !boxed_dispatch => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            ctx.instructions.push(raw_i64_arith_instruction(arith));
        }
        (LirRepr::F64, LirRepr::F64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            match arith {
                ArithOp::Add => ctx.instructions.push(Instruction::F64Add),
                ArithOp::Sub => ctx.instructions.push(Instruction::F64Sub),
                ArithOp::Mul => ctx.instructions.push(Instruction::F64Mul),
                ArithOp::Div => ctx.instructions.push(Instruction::F64Div),
                ArithOp::FloorDiv => {
                    ctx.instructions.push(Instruction::F64Div);
                    ctx.instructions.push(Instruction::F64Floor);
                }
                ArithOp::Mod => {
                    let scratch_a = ctx.alloc_scratch_local(ValType::F64);
                    let scratch_b = ctx.alloc_scratch_local(ValType::F64);
                    ctx.instructions.push(Instruction::LocalSet(scratch_b));
                    ctx.instructions.push(Instruction::LocalSet(scratch_a));
                    ctx.instructions.push(Instruction::LocalGet(scratch_a));
                    ctx.instructions.push(Instruction::LocalGet(scratch_a));
                    ctx.instructions.push(Instruction::LocalGet(scratch_b));
                    ctx.instructions.push(Instruction::F64Div);
                    ctx.instructions.push(Instruction::F64Floor);
                    ctx.instructions.push(Instruction::LocalGet(scratch_b));
                    ctx.instructions.push(Instruction::F64Mul);
                    ctx.instructions.push(Instruction::F64Sub);
                }
            }
        }
        _ => {
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.emit_runtime_call(boxed_arith_runtime_call(arith));
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

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

pub(in crate::wasm::lir_fast) fn emit_lir_unary_arith(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    _unary: UnaryOp,
) {
    let tir_op = &op.tir_op;
    if tir_op.operands.is_empty() || op.result_values.is_empty() {
        return;
    }
    let src = tir_op.operands[0];
    let dst = op.result_values[0].id;
    match ctx.repr_of(src) {
        LirRepr::I64 => {
            ctx.instructions.push(Instruction::I64Const(0));
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I64Sub);
        }
        LirRepr::F64 => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::F64Neg);
        }
        _ => {
            emit_get_boxed_for_repr(ctx, src);
            ctx.emit_runtime_call(LirRuntimeCall::Neg);
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

pub(in crate::wasm::lir_fast) fn emit_lir_unary_pos(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.is_empty() || op.result_values.is_empty() {
        return;
    }
    let src = tir_op.operands[0];
    let dst = op.result_values[0].id;
    match (ctx.repr_of(src), op.result_values[0].repr) {
        (LirRepr::I64, LirRepr::I64) | (LirRepr::F64, LirRepr::F64) => ctx.emit_get(src),
        _ => {
            emit_get_boxed_for_repr(ctx, src);
            ctx.emit_runtime_call(LirRuntimeCall::Pos);
        }
    }
    ctx.emit_set(dst);
}
