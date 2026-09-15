use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use crate::wasm_values::push_f64_to_i64_canonical;
use molt_codegen_abi::{
    INLINE_INT_BIAS, INLINE_INT_LIMIT, INT_MASK, INT_SHIFT, QNAN_TAG_BOOL_I64, QNAN_TAG_INT_I64,
    QNAN_TAG_MASK_I64, box_none_bits,
};
use molt_tir::tir::lir::LirRepr;
use molt_tir::tir::values::ValueId;
use wasm_encoder::{BlockType, Instruction, ValType};

/// Push operand `v` onto the WASM stack in NaN-boxed form, ready for a runtime
/// helper call. Raw-i64 carriers use the overflow-safe path because they may be
/// full-i64 `RawI64FullDeopt` values.
pub(in crate::wasm::lir_fast) fn emit_get_boxed_for_repr(ctx: &mut LirLowerCtx, v: ValueId) {
    match ctx.repr_of(v) {
        LirRepr::I64 => {
            let local = ctx.boxed_operand_local(v);
            ctx.instructions.push(Instruction::LocalGet(local));
            ctx.instructions
                .push(Instruction::I64Const(box_none_bits()));
            ctx.instructions.push(Instruction::I64Eq);
            ctx.instructions.push(Instruction::If(BlockType::Empty));
            emit_box_i64_overflow_safe(ctx, v);
            ctx.instructions.push(Instruction::LocalTee(local));
            ctx.instructions
                .push(Instruction::I64Const(box_none_bits()));
            ctx.instructions.push(Instruction::I64Eq);
            ctx.branch_to_operation_cleanup_if();
            ctx.instructions.push(Instruction::End);
            ctx.instructions.push(Instruction::LocalGet(local));
        }
        LirRepr::Bool1 => {
            ctx.emit_get(v);
            ctx.instructions.push(Instruction::I64ExtendI32U);
            ctx.instructions
                .push(Instruction::I64Const(QNAN_TAG_BOOL_I64));
            ctx.instructions.push(Instruction::I64Or);
        }
        LirRepr::F64 => {
            ctx.emit_get(v);
            let scratch = ctx.alloc_scratch_local(ValType::I64);
            push_f64_to_i64_canonical(|instruction| ctx.instructions.push(instruction), scratch);
        }
        LirRepr::DynBox | LirRepr::Ref64 => ctx.emit_get(v),
    }
}

pub(super) fn emit_box_inline_i64(ctx: &mut LirLowerCtx, src: ValueId) {
    ctx.emit_get(src);
    ctx.instructions
        .push(Instruction::I64Const(INT_MASK as i64));
    ctx.instructions.push(Instruction::I64And);
    ctx.instructions
        .push(Instruction::I64Const(QNAN_TAG_INT_I64));
    ctx.instructions.push(Instruction::I64Or);
}

/// Box a raw-i64 carrier overflow-safely: inline-47 fast path with a cold
/// `int_from_i64` runtime call for values outside `[-2^46, 2^46)`.
pub(super) fn emit_box_i64_overflow_safe(ctx: &mut LirLowerCtx, src: ValueId) {
    ctx.emit_get(src);
    ctx.instructions
        .push(Instruction::I64Const(INLINE_INT_BIAS));
    ctx.instructions.push(Instruction::I64Add);
    ctx.instructions
        .push(Instruction::I64Const(INLINE_INT_LIMIT));
    ctx.instructions.push(Instruction::I64LtU);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I64)));
    emit_box_inline_i64(ctx, src);
    ctx.instructions.push(Instruction::Else);
    ctx.emit_get(src);
    ctx.emit_runtime_call(LirRuntimeCall::IntFromI64);
    ctx.instructions.push(Instruction::End);
}

/// Typed extraction shared by explicit UnboxVal and the boxed function ABI.
/// The caller proves an integer representable in i64, not necessarily inline.
/// Keep the inline path local; only heap integers call the runtime extractor.
pub(in crate::wasm::lir_fast) fn emit_unbox_i64(ctx: &mut LirLowerCtx, local: u32) {
    ctx.instructions.push(Instruction::LocalGet(local));
    ctx.instructions
        .push(Instruction::I64Const(QNAN_TAG_MASK_I64));
    ctx.instructions.push(Instruction::I64And);
    ctx.instructions
        .push(Instruction::I64Const(QNAN_TAG_INT_I64));
    ctx.instructions.push(Instruction::I64Eq);
    ctx.instructions
        .push(Instruction::If(BlockType::Result(ValType::I64)));
    ctx.instructions.push(Instruction::LocalGet(local));
    ctx.instructions.push(Instruction::I64Const(INT_SHIFT));
    ctx.instructions.push(Instruction::I64Shl);
    ctx.instructions.push(Instruction::I64Const(INT_SHIFT));
    ctx.instructions.push(Instruction::I64ShrS);
    ctx.instructions.push(Instruction::Else);
    ctx.instructions.push(Instruction::LocalGet(local));
    ctx.emit_runtime_call(LirRuntimeCall::IntAsI64);
    ctx.instructions.push(Instruction::End);
}

pub(in crate::wasm::lir_fast) fn emit_box_none(ctx: &mut LirLowerCtx) {
    ctx.instructions
        .push(Instruction::I64Const(box_none_bits()));
}

pub(in crate::wasm::lir_fast) fn emit_return_boxed_i64(ctx: &mut LirLowerCtx, value: ValueId) {
    if ctx.repr_of(value) == LirRepr::I64 {
        // Unlike a borrowed call argument, a return transfers the freshly
        // materialized owner to the caller (or None with pending MemoryError).
        emit_box_i64_overflow_safe(ctx, value);
    } else {
        emit_get_boxed_for_repr(ctx, value);
    }
}
