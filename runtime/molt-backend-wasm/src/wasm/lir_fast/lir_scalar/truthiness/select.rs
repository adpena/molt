use super::super::super::lir_context::LirLowerCtx;
use super::super::super::runtime_calls::LirRuntimeCall;
use super::super::boxing::emit_get_boxed_for_repr;
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::{BlockType, Instruction, ValType};

pub(in crate::wasm::lir_fast) fn emit_lir_bool_select(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    is_and: bool,
) {
    let tir_op = &op.tir_op;
    if tir_op.operands.len() < 2 || op.result_values.is_empty() {
        return;
    }
    let lhs = tir_op.operands[0];
    let rhs = tir_op.operands[1];
    let result = &op.result_values[0];
    let dst = result.id;
    if ctx.repr_of(lhs) == LirRepr::Bool1
        && ctx.repr_of(rhs) == LirRepr::Bool1
        && result.repr == LirRepr::Bool1
    {
        ctx.emit_get(lhs);
        ctx.emit_get(rhs);
        ctx.instructions.push(if is_and {
            Instruction::I32And
        } else {
            Instruction::I32Or
        });
        ctx.emit_set(dst);
        return;
    }

    assert!(
        matches!(result.repr, LirRepr::DynBox | LirRepr::Ref64),
        "boxed Python boolean selection must produce a boxed result, got {:?}",
        result.repr
    );
    assert!(
        crate::tir::op_kinds_generated::opcode_result_mints_owned_selected_operand_table(
            tir_op.opcode
        ),
        "boxed Python boolean selection must mint an owned selected operand"
    );

    emit_get_boxed_for_repr(ctx, lhs);
    ctx.emit_runtime_call(LirRuntimeCall::IsTruthy);
    ctx.instructions.push(Instruction::I64Const(0));
    ctx.instructions.push(Instruction::I64Ne);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I64)));
    if is_and {
        emit_get_boxed_for_repr(ctx, rhs);
    } else {
        emit_get_boxed_for_repr(ctx, lhs);
    }
    ctx.instructions.push(Instruction::Else);
    if is_and {
        emit_get_boxed_for_repr(ctx, lhs);
    } else {
        emit_get_boxed_for_repr(ctx, rhs);
    }
    ctx.instructions.push(Instruction::End);
    ctx.instructions
        .push(Instruction::LocalTee(ctx.get_local(dst)));
    ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
}
