use super::super::super::lir_context::LirLowerCtx;
use super::super::super::runtime_calls::LirRuntimeCall;
use super::super::boxing::emit_get_boxed_for_repr;
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::Instruction;

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
