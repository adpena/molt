use super::super::lir_context::LirLowerCtx;
use super::super::lir_runtime_ops::emit_lir_runtime_result;
use super::super::runtime_calls::{LirRuntimeCall, numeric_lir_runtime_call};
use super::boxing::emit_get_boxed_for_repr;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::Instruction;

pub(in crate::wasm::lir_fast) fn emit_lir_bitwise(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    selection: WasmNumericRuntimeSelection,
) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let (i64_instr, bool_instr) = match selection.op_loop_kind {
        WasmNumericOpLoopKind::BitAnd => (Instruction::I64And, Instruction::I32And),
        WasmNumericOpLoopKind::BitOr => (Instruction::I64Or, Instruction::I32Or),
        WasmNumericOpLoopKind::BitXor => (Instruction::I64Xor, Instruction::I32Xor),
        _ => unreachable!("non-bitwise numeric selector routed to LIR bitwise emitter"),
    };
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let result = &op.result_values[0];
    if ctx.repr_of(lhs) == LirRepr::Bool1
        && ctx.repr_of(rhs) == LirRepr::Bool1
        && result.repr == LirRepr::Bool1
    {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(bool_instr);
        ctx.emit_set(result.id);
        return;
    }
    emit_lir_i64_binary_or_boxed(ctx, op, i64_instr, numeric_lir_runtime_call(selection));
}

pub(in crate::wasm::lir_fast) fn emit_lir_bit_not(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    selection: WasmNumericRuntimeSelection,
) {
    let tir_op = &op.tir_op;
    if let (Some(&src), Some(result)) = (tir_op.operands.first(), op.result_values.first()) {
        if ctx.repr_of(src) == LirRepr::I64 && result.repr == LirRepr::I64 {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I64Const(-1));
            ctx.instructions.push(Instruction::I64Xor);
            ctx.emit_set(result.id);
        } else {
            emit_get_boxed_for_repr(ctx, src);
            ctx.emit_runtime_call(numeric_lir_runtime_call(selection));
            emit_lir_runtime_result(ctx, op);
        }
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_shift(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    selection: WasmNumericRuntimeSelection,
) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let instruction = match selection.op_loop_kind {
        WasmNumericOpLoopKind::LShift => Instruction::I64Shl,
        WasmNumericOpLoopKind::RShift => Instruction::I64ShrS,
        _ => unreachable!("non-shift numeric selector routed to LIR shift emitter"),
    };
    // Shifts require raw-result proof: WASM masks counts mod 64 and Python does
    // not. Any unproven count or result routes through the BigInt-correct helper.
    emit_lir_i64_binary_or_boxed(ctx, op, instruction, numeric_lir_runtime_call(selection));
}

fn emit_lir_i64_binary_or_boxed(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    bare_i64_instr: Instruction<'static>,
    boxed_runtime_call: LirRuntimeCall,
) {
    let lhs = op.tir_op.operands[0];
    let rhs = op.tir_op.operands[1];
    let result = &op.result_values[0];
    let raw_lane_ok = ctx.repr_of(lhs) == LirRepr::I64
        && ctx.repr_of(rhs) == LirRepr::I64
        && result.repr == LirRepr::I64;
    if raw_lane_ok {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(bare_i64_instr);
        ctx.emit_set(result.id);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
        emit_get_boxed_for_repr(ctx, rhs);
        ctx.emit_runtime_call(boxed_runtime_call);
        emit_lir_runtime_result(ctx, op);
    }
}
