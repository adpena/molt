use super::super::lir_context::LirLowerCtx;
use super::super::lir_scalar::emit_get_boxed_for_repr;
use super::super::runtime_calls::{LirFixedRuntimeCall, LirRuntimeCall};
use crate::wasm::body::WasmLirFallbackReason;
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::ops::AttrValue;
use molt_tir::tir::values::ValueId;
use std::sync::Arc;
use wasm_encoder::Instruction;

#[derive(Clone)]
pub(in crate::wasm::lir_fast::lir_runtime_ops) enum LirRuntimeArg {
    BoxedOperand(ValueId),
    DataPtrI32(Arc<[u8]>),
    I64Const(i64),
    ResolvedPtr32(ValueId),
}

impl LirRuntimeArg {
    pub(in crate::wasm::lir_fast::lir_runtime_ops) fn emit(&self, ctx: &mut LirLowerCtx) {
        match self {
            Self::BoxedOperand(value) => emit_get_boxed_for_repr(ctx, *value),
            Self::DataPtrI32(bytes) => ctx.instructions.push_data_ptr_i32(bytes.clone()),
            Self::I64Const(value) => ctx.instructions.push(Instruction::I64Const(*value)),
            Self::ResolvedPtr32(value) => {
                emit_get_boxed_for_repr(ctx, *value);
                ctx.emit_runtime_call(LirRuntimeCall::HandleResolve);
            }
        }
    }
}

pub(in crate::wasm::lir_fast) fn original_kind(op: &LirOp) -> Option<&str> {
    match op.tir_op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_boxed_operands_runtime_call(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    runtime_call: LirRuntimeCall,
) {
    let operand_count = runtime_call.boxed_operand_count().unwrap_or_else(|| {
        panic!("WASM LIR runtime call {runtime_call:?} lacks ABI boxed_operand_count")
    });
    emit_lir_boxed_operands_runtime_call_counted(ctx, op, runtime_call, operand_count);
}

pub(in crate::wasm::lir_fast) fn emit_lir_fixed_runtime_call(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    runtime_call: LirFixedRuntimeCall,
) {
    emit_lir_boxed_operands_runtime_call_counted(
        ctx,
        op,
        runtime_call.call,
        runtime_call.operand_count,
    );
}

fn emit_lir_boxed_operands_runtime_call_counted(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    runtime_call: LirRuntimeCall,
    operand_count: usize,
) {
    if op.tir_op.operands.len() < operand_count {
        return;
    }
    for &operand in &op.tir_op.operands[..operand_count] {
        LirRuntimeArg::BoxedOperand(operand).emit(ctx);
    }
    emit_lir_runtime_call_with_result(ctx, op, runtime_call);
}

pub(in crate::wasm::lir_fast::lir_runtime_ops) fn emit_lir_runtime_call_with_args_and_result(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    runtime_call: LirRuntimeCall,
    args: &[LirRuntimeArg],
) {
    for arg in args {
        arg.emit(ctx);
    }
    emit_lir_runtime_call_with_result(ctx, op, runtime_call);
}

pub(in crate::wasm::lir_fast::lir_runtime_ops) fn emit_lir_runtime_call_with_args(
    ctx: &mut LirLowerCtx,
    runtime_call: LirRuntimeCall,
    args: &[LirRuntimeArg],
) {
    for arg in args {
        arg.emit(ctx);
    }
    ctx.emit_runtime_call(runtime_call);
}

pub(in crate::wasm::lir_fast::lir_runtime_ops) fn emit_lir_runtime_call_with_result(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    runtime_call: LirRuntimeCall,
) {
    ctx.emit_runtime_call(runtime_call);
    emit_lir_runtime_result(ctx, op);
}

pub(in crate::wasm::lir_fast::lir_runtime_ops) fn required_i64_attr(
    op: &LirOp,
    attr: &str,
    op_name: &str,
) -> i64 {
    match op.tir_op.attrs.get(attr) {
        Some(AttrValue::Int(value)) => *value,
        _ => panic!("{op_name} requires integer attr {attr}"),
    }
}

pub(in crate::wasm::lir_fast::lir_runtime_ops) fn required_name_bytes(
    op: &LirOp,
    op_name: &str,
) -> Arc<[u8]> {
    match op.tir_op.attrs.get("name") {
        Some(AttrValue::Str(name)) => Arc::from(name.as_bytes()),
        _ => panic!("{op_name} requires string attr name"),
    }
}

pub(in crate::wasm::lir_fast::lir_runtime_ops) fn required_operand(
    op: &LirOp,
    index: usize,
    op_name: &str,
) -> ValueId {
    op.tir_op
        .operands
        .get(index)
        .copied()
        .unwrap_or_else(|| panic!("{op_name} requires operand {index}"))
}

pub(in crate::wasm::lir_fast::lir_runtime_ops) fn required_source_op_index(
    op: &LirOp,
    op_name: &str,
) -> usize {
    op.tir_op
        .source_op_index()
        .unwrap_or_else(|| panic!("{op_name} requires source op index"))
}

pub(in crate::wasm::lir_fast::lir_runtime_ops) fn emit_lir_runtime_result(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
) {
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
