use super::super::lir_context::LirLowerCtx;
use super::super::lir_runtime_ops::{
    emit_lir_fixed_runtime_call, emit_lir_unsupported_marker, original_kind,
};
use super::super::lir_scalar::{emit_get_boxed_for_repr, emit_unbox_i64};
use super::super::runtime_calls::lir_fixed_runtime_call;
use crate::wasm::lir_fast::LirRuntimeCall;
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::Instruction;

pub(super) fn emit_lir_box_value(ctx: &mut LirLowerCtx, op: &LirOp) {
    if op.result_values.is_empty() {
        if let Some(&src) = op.tir_op.operands.first()
            && ctx.repr_of(src) == LirRepr::I64
        {
            // A discarded full-width box can still fail. Keep materialization
            // and let the operation-owner scope retire its temporary credit.
            emit_get_boxed_for_repr(ctx, src);
            ctx.instructions.push(Instruction::Drop);
        }
        return;
    }
    if let (Some(&src), Some(result)) = (op.tir_op.operands.first(), op.result_values.first()) {
        assert!(matches!(result.repr, LirRepr::DynBox | LirRepr::Ref64));
        emit_get_boxed_for_repr(ctx, src);
        ctx.emit_set(result.id);
        if matches!(ctx.repr_of(src), LirRepr::DynBox | LirRepr::Ref64) {
            // BoxVal is not a transparent alias in the shared ownership
            // authority: its result owns a reference distinct from its input.
            ctx.emit_get(result.id);
            ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
        } else if ctx.repr_of(src) == LirRepr::I64 {
            // BoxVal owns the physical materialization, unlike a borrowed
            // runtime operand. Its SSA drop/return now controls that owner.
            let owner = ctx.boxed_operand_local(src);
            ctx.forget_operation_owner(owner);
        }
    }
}

/// Unbox is a representation extraction with a typed IR precondition, not a
/// Python conversion. Preserve the entire signed i64 range, including integers
/// represented by a BigInt; an inline-payload mask alone truncates heap bits.
pub(super) fn emit_lir_unbox_value(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(result) = op.result_values.first() else {
        // Representation extraction is pure and creates no owner when unused.
        return;
    };
    let &src = op
        .tir_op
        .operands
        .first()
        .expect("UnboxVal requires operand");
    assert!(matches!(ctx.repr_of(src), LirRepr::DynBox | LirRepr::Ref64));
    match result.repr {
        LirRepr::I64 => {
            let local = ctx.get_local(src);
            emit_unbox_i64(ctx, local);
        }
        LirRepr::Bool1 => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::I32WrapI64);
            ctx.instructions.push(Instruction::I32Const(1));
            ctx.instructions.push(Instruction::I32And);
        }
        LirRepr::F64 => {
            ctx.emit_get(src);
            ctx.instructions.push(Instruction::F64ReinterpretI64);
        }
        LirRepr::DynBox | LirRepr::Ref64 => {
            // Unwrapping a semantic Box type does not override the shared
            // physical carrier plan. A still-boxed result owns its reference.
            ctx.emit_get(src);
            ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
            ctx.emit_get(src);
        }
    }
    ctx.emit_set(result.id);
}

pub(super) fn emit_lir_identity_copy(ctx: &mut LirLowerCtx, op: &LirOp) {
    if let (Some(&src), Some(result)) = (op.tir_op.operands.first(), op.result_values.first()) {
        ctx.emit_get(src);
        ctx.emit_set(result.id);
    }
}

pub(super) fn emit_lir_copy_or_original_kind(ctx: &mut LirLowerCtx, op: &LirOp) {
    match original_kind(op) {
        Some("binding_alias") => emit_lir_binding_alias(ctx, op),
        Some(kind)
            if crate::tir::op_kinds_generated::copy_kind_is_explicit_no_heap_move_table(kind) =>
        {
            emit_lir_identity_copy(ctx, op)
        }
        Some(kind) if let Some(runtime) = lir_fixed_runtime_call(kind) => {
            emit_lir_fixed_runtime_call(ctx, op, runtime)
        }
        Some(_) => emit_lir_unsupported_marker(ctx, op),
        None => emit_lir_identity_copy(ctx, op),
    }
}

fn emit_lir_binding_alias(ctx: &mut LirLowerCtx, op: &LirOp) {
    if let (Some(&src), Some(result)) = (op.tir_op.operands.first(), op.result_values.first()) {
        if matches!(result.repr, LirRepr::DynBox | LirRepr::Ref64) {
            emit_get_boxed_for_repr(ctx, src);
            ctx.instructions
                .push(Instruction::LocalTee(ctx.get_local(result.id)));
            ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
        } else {
            assert_eq!(
                ctx.repr_of(src),
                result.repr,
                "raw binding alias changes carrier"
            );
            ctx.emit_get(src);
            ctx.emit_set(result.id);
        }
    }
}
