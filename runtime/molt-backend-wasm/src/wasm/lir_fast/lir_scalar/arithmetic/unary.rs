use super::super::super::lir_context::LirLowerCtx;
use super::super::super::runtime_calls::numeric_lir_runtime_call;
use super::super::boxing::emit_get_boxed_for_repr;
use crate::wasm_abi_generated::WasmNumericRuntimeSelection;
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::Instruction;

pub(in crate::wasm::lir_fast) fn emit_lir_unary_arith(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    selection: WasmNumericRuntimeSelection,
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
            ctx.emit_runtime_call(numeric_lir_runtime_call(selection));
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

pub(in crate::wasm::lir_fast) fn emit_lir_unary_pos(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    selection: WasmNumericRuntimeSelection,
) {
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
            ctx.emit_runtime_call(numeric_lir_runtime_call(selection));
        }
    }
    ctx.emit_set(dst);
}
