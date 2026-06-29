use super::super::lir_context::LirLowerCtx;
use super::super::lir_scalar::emit_get_boxed_for_repr;
use super::super::runtime_calls::LirRuntimeCall;
use super::call_abi::{emit_lir_boxed_operands_runtime_call, emit_lir_runtime_result};
use crate::wasm::body::WasmLirFallbackReason;
use crate::wasm::container_runtime_select::selected_lir_container_runtime_call;
use crate::wasm_abi_generated::WasmContainerRuntimeOp;
use molt_codegen_abi::box_none_bits;
use molt_tir::tir::lir::{LirOp, LirRepr};
use wasm_encoder::Instruction;

pub(in crate::wasm::lir_fast) fn emit_lir_index(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(&container) = op.tir_op.operands.first() else {
        return;
    };
    let runtime_call = selected_lir_container_runtime_call(
        WasmContainerRuntimeOp::Index,
        ctx.has_flat_list_int_storage(container),
        ctx.type_of(container),
    )
    .unwrap_or(LirRuntimeCall::Index);
    emit_lir_boxed_operands_runtime_call(ctx, op, runtime_call);
}

pub(in crate::wasm::lir_fast) fn emit_lir_store_index(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(&container) = op.tir_op.operands.first() else {
        return;
    };
    let runtime_call = selected_lir_container_runtime_call(
        WasmContainerRuntimeOp::StoreIndex,
        ctx.has_flat_list_int_storage(container),
        ctx.type_of(container),
    )
    .unwrap_or(LirRuntimeCall::StoreIndex);
    emit_lir_boxed_operands_runtime_call(ctx, op, runtime_call);
}

pub(in crate::wasm::lir_fast) fn emit_lir_del_index(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::DelIndex);
}

pub(in crate::wasm::lir_fast) fn emit_lir_get_iter(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::Iter);
}

pub(in crate::wasm::lir_fast) fn emit_lir_iter_next(ctx: &mut LirLowerCtx, op: &LirOp) {
    emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::IterNext);
}

pub(in crate::wasm::lir_fast) fn emit_lir_build_slice(ctx: &mut LirLowerCtx, op: &LirOp) {
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

pub(in crate::wasm::lir_fast) fn emit_lir_membership(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    invert: bool,
) {
    if op.tir_op.operands.len() < 2 {
        return;
    }
    let container = op.tir_op.operands[0];
    emit_get_boxed_for_repr(ctx, container);
    emit_get_boxed_for_repr(ctx, op.tir_op.operands[1]);
    let runtime_call = selected_lir_container_runtime_call(
        WasmContainerRuntimeOp::Contains,
        false,
        ctx.type_of(container),
    )
    .unwrap_or(LirRuntimeCall::Contains);
    ctx.emit_runtime_call(runtime_call);
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
