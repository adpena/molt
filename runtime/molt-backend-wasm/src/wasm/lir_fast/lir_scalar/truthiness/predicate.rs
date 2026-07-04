use super::super::super::lir_context::LirLowerCtx;
use super::super::super::runtime_calls::LirRuntimeCall;
use molt_codegen_abi::{QNAN, QNAN_TAG_MASK_I64, TAG_BOOL};
use molt_tir::tir::lir::LirRepr;
use molt_tir::tir::values::ValueId;
use wasm_encoder::{BlockType, Instruction, ValType};

pub(in crate::wasm::lir_fast) fn emit_lir_truthiness_i32(ctx: &mut LirLowerCtx, src: ValueId) {
    match ctx.repr_of(src) {
        LirRepr::Bool1 => ctx.emit_get(src),
        LirRepr::I64 => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I64Const(0));
            ctx.instructions.push(Instruction::I64Ne);
        }
        LirRepr::F64 => {
            ctx.emit_get(src);
            ctx.instructions
                .push(Instruction::F64Const(wasm_encoder::Ieee64::from(0.0)));
            ctx.instructions.push(Instruction::F64Ne);
        }
        LirRepr::DynBox | LirRepr::Ref64 => {
            ctx.emit_get(src);
            ctx.instructions
                .push(Instruction::I64Const(QNAN_TAG_MASK_I64));
            ctx.instructions.push(Instruction::I64And);
            ctx.instructions
                .push(Instruction::I64Const((QNAN | TAG_BOOL) as i64));
            ctx.instructions.push(Instruction::I64Eq);
            ctx.instructions
                .push(Instruction::If(BlockType::Result(ValType::I32)));
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I32WrapI64);
            ctx.instructions.push(Instruction::I32Const(1));
            ctx.instructions.push(Instruction::I32And);
            ctx.instructions.push(Instruction::Else);
            ctx.emit_get(src);
            ctx.emit_runtime_call(LirRuntimeCall::IsTruthy);
            ctx.instructions.push(Instruction::I64Const(0));
            ctx.instructions.push(Instruction::I64Ne);
            ctx.instructions.push(Instruction::End);
        }
    }
}
