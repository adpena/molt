use super::lir_context::LirLowerCtx;
use super::lir_scalar::emit_get_boxed_for_repr;
use super::runtime_calls::LirRuntimeCall;
use crate::wasm::body::WasmLirFallbackReason;
use molt_codegen_abi::{QNAN_TAG_BOOL_I64, box_none_bits};
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::types::TirType;
use wasm_encoder::Instruction;

pub(super) fn emit_lir_boxed_binary_runtime_call(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    runtime_call: LirRuntimeCall,
) {
    emit_lir_boxed_operands_runtime_call(ctx, op, runtime_call, 2);
}

pub(super) fn emit_lir_index(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(&container) = op.tir_op.operands.first() else {
        return;
    };
    let runtime_call = if ctx.has_flat_list_int_storage(container) {
        LirRuntimeCall::ListIntGetitem
    } else {
        match ctx.type_of(container) {
            Some(TirType::Dict(_, _)) => LirRuntimeCall::DictGetitem,
            Some(TirType::Tuple(_)) => LirRuntimeCall::TupleGetitem,
            _ => LirRuntimeCall::Index,
        }
    };
    emit_lir_boxed_operands_runtime_call(ctx, op, runtime_call, 2);
}

pub(super) fn emit_lir_store_index(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(&container) = op.tir_op.operands.first() else {
        return;
    };
    let runtime_call = if ctx.has_flat_list_int_storage(container) {
        LirRuntimeCall::ListIntSetitem
    } else {
        match ctx.type_of(container) {
            Some(TirType::Dict(_, _)) => LirRuntimeCall::DictSetitem,
            _ => LirRuntimeCall::StoreIndex,
        }
    };
    emit_lir_boxed_operands_runtime_call(ctx, op, runtime_call, 3);
}

pub(super) fn emit_lir_del_index(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::DelIndex, 2);
}

pub(super) fn emit_lir_get_iter(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::Iter, 1);
}

pub(super) fn emit_lir_iter_next(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::IterNext, 1);
}

pub(super) fn emit_lir_build_slice(ctx: &mut LirLowerCtx, op: &LirOp) {
    for idx in 0..3 {
        if let Some(&operand) = op.tir_op.operands.get(idx) {
            emit_get_boxed_for_repr(ctx, operand);
        } else {
            ctx.instructions
                .push(Instruction::I64Const(box_none_bits()));
        }
    }
    ctx.emit_runtime_call(LirRuntimeCall::SliceNew);
    emit_lir_runtime_result(ctx, op);
}

pub(super) fn emit_lir_membership(ctx: &mut LirLowerCtx, op: &LirOp, invert: bool) {
    if op.tir_op.operands.len() < 2 {
        return;
    }
    let container = op.tir_op.operands[0];
    emit_get_boxed_for_repr(ctx, container);
    emit_get_boxed_for_repr(ctx, op.tir_op.operands[1]);
    ctx.emit_runtime_call(lir_contains_call_for_container(ctx.type_of(container)));
    if !invert {
        emit_lir_runtime_result(ctx, op);
        return;
    }
    let Some(result) = op.result_values.first() else {
        ctx.instructions.push(Instruction::Drop);
        return;
    };
    match result.repr {
        LirRepr::Bool1 => {
            ctx.instructions.push(Instruction::I64Const(1));
            ctx.instructions.push(Instruction::I64And);
            ctx.instructions.push(Instruction::I32WrapI64);
            ctx.instructions.push(Instruction::I32Eqz);
            ctx.emit_set(result.id);
        }
        LirRepr::DynBox | LirRepr::Ref64 => {
            ctx.emit_runtime_call(LirRuntimeCall::Not);
            emit_lir_runtime_result(ctx, op);
        }
        LirRepr::I64 | LirRepr::F64 => {
            ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
            ctx.emit_set(result.id);
        }
    }
}

pub(super) fn emit_lir_boxed_operands_runtime_call(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    runtime_call: LirRuntimeCall,
    operand_count: usize,
) {
    if op.tir_op.operands.len() < operand_count {
        return;
    }
    for &operand in &op.tir_op.operands[..operand_count] {
        emit_get_boxed_for_repr(ctx, operand);
    }
    ctx.emit_runtime_call(runtime_call);
    emit_lir_runtime_result(ctx, op);
}

fn lir_contains_call_for_container(container_ty: Option<&TirType>) -> LirRuntimeCall {
    match container_ty {
        Some(TirType::Dict(_, _)) => LirRuntimeCall::DictContains,
        Some(TirType::List(_)) => LirRuntimeCall::ListContains,
        Some(TirType::Set(_)) => LirRuntimeCall::SetContains,
        Some(TirType::Str) => LirRuntimeCall::StrContains,
        _ => LirRuntimeCall::Contains,
    }
}

pub(super) fn emit_lir_exception_pending(ctx: &mut LirLowerCtx, op: &LirOp) {
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

fn emit_lir_runtime_result(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(result) = op.result_values.first() else {
        ctx.instructions.push(Instruction::Drop);
        return;
    };
    match result.repr {
        LirRepr::DynBox | LirRepr::Ref64 => ctx.emit_set(result.id),
        LirRepr::Bool1 => {
            ctx.instructions.push(Instruction::I64Const(1));
            ctx.instructions.push(Instruction::I64And);
            ctx.instructions.push(Instruction::I32WrapI64);
            ctx.emit_set(result.id);
        }
        LirRepr::I64 | LirRepr::F64 => {
            ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
            ctx.emit_set(result.id);
        }
    }
}
