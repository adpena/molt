use super::super::super::lir_context::LirLowerCtx;
use super::super::super::runtime_calls::numeric_lir_runtime_call;
use super::super::boxing::{emit_box_inline_i64, emit_box_none, emit_get_boxed_for_repr};
use crate::wasm_abi_generated::{WasmNumericOpLoopKind, WasmNumericRuntimeSelection};
use molt_codegen_abi::{INT_MAX_INLINE as INLINE_INT_MAX, INT_MIN_INLINE as INLINE_INT_MIN};
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::ops::AttrValue;
use wasm_encoder::{BlockType, Instruction, ValType};

fn raw_i64_arith_instruction(op_loop_kind: WasmNumericOpLoopKind) -> Instruction<'static> {
    match op_loop_kind {
        WasmNumericOpLoopKind::Add => Instruction::I64Add,
        WasmNumericOpLoopKind::Sub => Instruction::I64Sub,
        WasmNumericOpLoopKind::Mul => Instruction::I64Mul,
        WasmNumericOpLoopKind::TrueDiv | WasmNumericOpLoopKind::FloorDiv => Instruction::I64DivS,
        WasmNumericOpLoopKind::Mod => Instruction::I64RemS,
        _ => unreachable!("non-arithmetic numeric selector routed to arithmetic emitter"),
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_binary_arith(
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
    if matches!(
        tir_op.attrs.get("lir.checked_overflow"),
        Some(AttrValue::Bool(true))
    ) {
        let main = op.result_values[0].id;
        let overflow_box = op.result_values[1].id;
        let overflow_flag = op.result_values[2].id;

        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions
            .push(raw_i64_arith_instruction(selection.op_loop_kind));
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
        ctx.emit_runtime_call(numeric_lir_runtime_call(selection));
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
            ctx.instructions
                .push(raw_i64_arith_instruction(selection.op_loop_kind));
        }
        (LirRepr::F64, LirRepr::F64) => {
            ctx.emit_get(lhs);
            ctx.emit_get(rhs);
            match selection.op_loop_kind {
                WasmNumericOpLoopKind::Add => ctx.instructions.push(Instruction::F64Add),
                WasmNumericOpLoopKind::Sub => ctx.instructions.push(Instruction::F64Sub),
                WasmNumericOpLoopKind::Mul => ctx.instructions.push(Instruction::F64Mul),
                WasmNumericOpLoopKind::TrueDiv => ctx.instructions.push(Instruction::F64Div),
                WasmNumericOpLoopKind::FloorDiv => {
                    ctx.instructions.push(Instruction::F64Div);
                    ctx.instructions.push(Instruction::F64Floor);
                }
                WasmNumericOpLoopKind::Mod => {
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
                _ => unreachable!("non-arithmetic numeric selector routed to arithmetic emitter"),
            }
        }
        _ => {
            emit_get_boxed_for_repr(ctx, lhs);
            emit_get_boxed_for_repr(ctx, rhs);
            ctx.emit_runtime_call(numeric_lir_runtime_call(selection));
            ctx.emit_set(dst);
            return;
        }
    }
    ctx.emit_set(dst);
}
