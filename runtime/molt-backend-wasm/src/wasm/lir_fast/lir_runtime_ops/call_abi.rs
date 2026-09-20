use super::super::lir_context::LirLowerCtx;
use super::super::lir_scalar::emit_get_boxed_for_repr;
use super::super::runtime_calls::{LirFixedRuntimeCall, LirRuntimeCall};
use crate::wasm::body::WasmLirFallbackReason;
use crate::wasm_abi_generated::{STATIC_FUNC_TYPES, WasmRuntimeReturn};
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::ops::AttrValue;
use molt_tir::tir::values::ValueId;
use std::sync::Arc;
use wasm_encoder::{Instruction, ValType};

#[derive(Clone)]
pub(in crate::wasm::lir_fast::lir_runtime_ops) enum LirRuntimeArg {
    BoxedOperand(ValueId),
    DataPtrI32(Arc<[u8]>),
    I64Const(i64),
    ResolvedPtr32(ValueId),
    ResolvedPtrBits64(ValueId),
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
            Self::ResolvedPtrBits64(value) => {
                emit_get_boxed_for_repr(ctx, *value);
                ctx.emit_runtime_call(LirRuntimeCall::HandleResolve);
                ctx.instructions.push(Instruction::I64ExtendI32U);
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
    emit_lir_runtime_result(ctx, op, runtime_call);
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

/// Discard the import's actual result, not merely its machine carrier.
pub(in crate::wasm::lir_fast) fn emit_lir_runtime_discard(
    ctx: &mut LirLowerCtx,
    call: LirRuntimeCall,
) {
    match call.import().return_contract() {
        WasmRuntimeReturn::OwnedObject | WasmRuntimeReturn::PollResult => {
            ctx.emit_runtime_call(LirRuntimeCall::DecRefObj);
        }
        WasmRuntimeReturn::BorrowedObject | WasmRuntimeReturn::RawBits => {
            ctx.instructions.push(Instruction::Drop);
        }
        WasmRuntimeReturn::Void => {}
        contract => panic!(
            "WASM LIR import {:?} requires dedicated {contract:?} lifetime custody",
            call.import()
        ),
    }
}

pub(in crate::wasm::lir_fast) fn emit_lir_runtime_result(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    call: LirRuntimeCall,
) {
    assert!(
        op.result_values.len() <= 1,
        "runtime import result is not a multi-result unpack"
    );
    let contract = call.import().return_contract();
    let Some(result) = op.result_values.first() else {
        emit_lir_runtime_discard(ctx, call);
        return;
    };
    if contract == WasmRuntimeReturn::RawBits {
        let signature = &STATIC_FUNC_TYPES[call.import().type_idx() as usize];
        let raw_type = match result.repr {
            LirRepr::I64 => Some(ValType::I64),
            LirRepr::F64 => Some(ValType::F64),
            LirRepr::Bool1 => Some(ValType::I32),
            LirRepr::DynBox | LirRepr::Ref64 => None,
        };
        if raw_type.is_some_and(|ty| signature.results == [ty]) {
            ctx.emit_set(result.id);
        } else {
            ctx.instructions.push(Instruction::Drop);
            ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
        }
        return;
    }
    let owned = match contract {
        WasmRuntimeReturn::OwnedObject | WasmRuntimeReturn::PollResult => true,
        WasmRuntimeReturn::BorrowedObject => false,
        WasmRuntimeReturn::Void => panic!(
            "void WASM LIR import {:?} cannot bind a result",
            call.import()
        ),
        contract => panic!(
            "WASM LIR import {:?} requires dedicated {contract:?} lifetime custody",
            call.import()
        ),
    };
    match result.repr {
        LirRepr::DynBox | LirRepr::Ref64 => {
            ctx.emit_set(result.id);
            if !owned {
                // Retain before operation-owner cleanup can release a boxed
                // operand that aliases this borrowed result.
                ctx.emit_get(result.id);
                ctx.emit_runtime_call(LirRuntimeCall::IncRefObj);
            }
        }
        LirRepr::Bool1 => {
            let owner = owned.then(|| {
                let local = ctx.alloc_scratch_local(ValType::I64);
                ctx.instructions.push(Instruction::LocalTee(local));
                local
            });
            ctx.instructions.push(Instruction::I64Const(1));
            ctx.instructions.push(Instruction::I64And);
            ctx.instructions.push(Instruction::I32WrapI64);
            ctx.emit_set(result.id);
            if let Some(owner) = owner {
                ctx.instructions.push(Instruction::LocalGet(owner));
                ctx.emit_runtime_call(LirRuntimeCall::DecRefObj);
            }
        }
        LirRepr::I64 | LirRepr::F64 => {
            emit_lir_runtime_discard(ctx, call);
            ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
        }
    }
}
