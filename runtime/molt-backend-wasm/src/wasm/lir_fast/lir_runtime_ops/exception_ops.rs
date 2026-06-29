use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use crate::wasm::body::WasmLirFallbackReason;
use molt_codegen_abi::QNAN_TAG_BOOL_I64;
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::Instruction;

pub(in crate::wasm::lir_fast) fn emit_lir_exception_pending(ctx: &mut LirLowerCtx, op: &LirOp) {
    ctx.emit_runtime_call(LirRuntimeCall::ExceptionPending);
    ctx.instructions.push(Instruction::I64Const(0));
    ctx.instructions.push(Instruction::I64Ne);
    let Some(result) = op.result_values.first() else {
        ctx.instructions.push(Instruction::Drop);
        return;
    };
    match result.repr {
        LirRepr::Bool1 => ctx.emit_set(result.id),
        LirRepr::DynBox | LirRepr::Ref64 => {
            ctx.instructions.push(Instruction::I64ExtendI32U);
            ctx.instructions
                .push(Instruction::I64Const(QNAN_TAG_BOOL_I64));
            ctx.instructions.push(Instruction::I64Or);
            ctx.emit_set(result.id);
        }
        LirRepr::I64 => {
            ctx.instructions.push(Instruction::I64ExtendI32U);
            ctx.emit_set(result.id);
        }
        LirRepr::F64 => {
            ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
            ctx.emit_set(result.id);
        }
    }
}
