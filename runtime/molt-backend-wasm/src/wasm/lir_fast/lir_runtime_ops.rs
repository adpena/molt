use super::lir_context::LirLowerCtx;
use super::lir_scalar::emit_get_boxed_for_repr;
use super::runtime_calls::LirRuntimeCall;
use crate::wasm::body::WasmLirFallbackReason;
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

fn emit_lir_boxed_operands_runtime_call(
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
