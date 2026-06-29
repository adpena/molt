use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use super::boxing::emit_get_boxed_for_repr;
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::values::ValueId;
use wasm_encoder::Instruction;

#[derive(Clone, Copy)]
pub(in crate::wasm::lir_fast) enum BitwiseOp {
    And,
    Or,
    Xor,
}

#[derive(Clone, Copy)]
pub(in crate::wasm::lir_fast) enum ShiftOp {
    Left,
    Right,
}

pub(in crate::wasm::lir_fast) fn emit_lir_bitwise(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    bw: BitwiseOp,
) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let instr = match bw {
        BitwiseOp::And => Instruction::I64And,
        BitwiseOp::Or => Instruction::I64Or,
        BitwiseOp::Xor => Instruction::I64Xor,
    };
    let runtime_call = match bw {
        BitwiseOp::And => LirRuntimeCall::BitAnd,
        BitwiseOp::Or => LirRuntimeCall::BitOr,
        BitwiseOp::Xor => LirRuntimeCall::BitXor,
    };
    emit_lir_i64_binary_or_boxed(
        ctx,
        tir_op.operands[0],
        tir_op.operands[1],
        op.result_values[0].id,
        op.result_values[0].repr,
        instr,
        false,
        runtime_call,
    );
}

pub(in crate::wasm::lir_fast) fn emit_lir_bit_not(ctx: &mut LirLowerCtx, op: &LirOp) {
    let tir_op = &op.tir_op;
    if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first()) {
        if ctx.repr_of(src) == LirRepr::I64 {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I64Const(-1));
            ctx.instructions.push(Instruction::I64Xor);
        } else {
            emit_get_boxed_for_repr(ctx, src);
            ctx.emit_runtime_call(LirRuntimeCall::Invert);
        }
        ctx.emit_set(result.id);
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_shift(ctx: &mut LirLowerCtx, op: &LirOp, shift: ShiftOp) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 {
        return;
    }
    let Some(result) = op.result_values.first() else {
        return;
    };
    let (instruction, runtime_call) = match shift {
        ShiftOp::Left => (Instruction::I64Shl, LirRuntimeCall::LShift),
        ShiftOp::Right => (Instruction::I64ShrS, LirRuntimeCall::RShift),
    };
    // Shifts require raw-result proof: WASM masks counts mod 64 and Python does
    // not. Any unproven count or result routes through the BigInt-correct helper.
    emit_lir_i64_binary_or_boxed(
        ctx,
        tir_op.operands[0],
        tir_op.operands[1],
        result.id,
        result.repr,
        instruction,
        true,
        runtime_call,
    );
}

fn emit_lir_i64_binary_or_boxed(
    ctx: &mut LirLowerCtx,
    lhs: ValueId,
    rhs: ValueId,
    dst: ValueId,
    dst_repr: LirRepr,
    bare_i64_instr: Instruction<'static>,
    require_raw_result: bool,
    boxed_runtime_call: LirRuntimeCall,
) {
    let raw_lane_ok = ctx.repr_of(lhs) == LirRepr::I64
        && ctx.repr_of(rhs) == LirRepr::I64
        && (!require_raw_result || dst_repr == LirRepr::I64);
    if raw_lane_ok {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(bare_i64_instr);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
        emit_get_boxed_for_repr(ctx, rhs);
        ctx.emit_runtime_call(boxed_runtime_call);
    }
    ctx.emit_set(dst);
}
