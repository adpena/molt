use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::{LirRuntimeCall, numeric_lir_runtime_call};
use super::boxing::emit_get_boxed_for_repr;
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::Instruction;

pub(in crate::wasm::lir_fast) fn emit_lir_comparison(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    selection: WasmNumericRuntimeSelection,
) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let dst = op.result_values[0].id;
    match (ctx.repr_of(lhs), ctx.repr_of(rhs)) {
        (LirRepr::I64, LirRepr::I64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            ctx.instructions.push(match selection.op_loop_kind {
                WasmNumericOpLoopKind::Eq => Instruction::I64Eq,
                WasmNumericOpLoopKind::Ne => Instruction::I64Ne,
                WasmNumericOpLoopKind::Lt => Instruction::I64LtS,
                WasmNumericOpLoopKind::Le => Instruction::I64LeS,
                WasmNumericOpLoopKind::Gt => Instruction::I64GtS,
                WasmNumericOpLoopKind::Ge => Instruction::I64GeS,
                _ => unreachable!("non-comparison numeric selector routed to LIR compare"),
            });
        }
        (LirRepr::F64, LirRepr::F64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            ctx.instructions.push(match selection.op_loop_kind {
                WasmNumericOpLoopKind::Eq => Instruction::F64Eq,
                WasmNumericOpLoopKind::Ne => Instruction::F64Ne,
                WasmNumericOpLoopKind::Lt => Instruction::F64Lt,
                WasmNumericOpLoopKind::Le => Instruction::F64Le,
                WasmNumericOpLoopKind::Gt => Instruction::F64Gt,
                WasmNumericOpLoopKind::Ge => Instruction::F64Ge,
                _ => unreachable!("non-comparison numeric selector routed to LIR compare"),
            });
        }
        _ => {
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.emit_runtime_call(numeric_lir_runtime_call(selection));
            if op.result_values[0].repr == LirRepr::Bool1 {
                ctx.instructions.push(Instruction::I64Const(1));
                ctx.instructions.push(Instruction::I64And);
                ctx.instructions.push(Instruction::I32WrapI64);
            }
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}

pub(in crate::wasm::lir_fast) fn emit_lir_identity_comparison(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    invert: bool,
) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let result = &op.result_values[0];

    emit_get_boxed_for_repr(ctx, lhs);
    emit_get_boxed_for_repr(ctx, rhs);
    ctx.emit_runtime_call(LirRuntimeCall::Is);

    match result.repr {
        LirRepr::Bool1 => {
            ctx.instructions.push(Instruction::I64Const(1));
            ctx.instructions.push(Instruction::I64And);
            ctx.instructions.push(Instruction::I32WrapI64);
            if invert {
                ctx.instructions.push(Instruction::I32Eqz);
            }
        }
        LirRepr::DynBox | LirRepr::Ref64 | LirRepr::I64 => {
            if invert {
                ctx.emit_runtime_call(LirRuntimeCall::Not);
            }
        }
        LirRepr::F64 => {
            panic!("identity comparison cannot materialize an f64 result");
        }
    }
    ctx.emit_set(result.id);
}
