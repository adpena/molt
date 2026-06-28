use super::lir_context::LirLowerCtx;
use super::lir_scalar::emit_get_boxed_for_repr;
use super::runtime_calls::LirRuntimeCall;
use crate::wasm::body::WasmLirFallbackReason;
use molt_codegen_abi::{QNAN_TAG_BOOL_I64, box_int_bits, box_none_bits, stable_ic_site_id};
use molt_tir::tir::lir::{LirOp, LirRepr};
use molt_tir::tir::ops::{AttrValue, OpCode};
use molt_tir::tir::types::TirType;
use molt_tir::tir::values::ValueId;
use std::sync::Arc;
use wasm_encoder::Instruction;

#[derive(Clone)]
enum LirRuntimeArg {
    BoxedOperand(ValueId),
    DataPtrI32(Arc<[u8]>),
    I64Const(i64),
    ResolvedPtr32(ValueId),
}

#[derive(Clone, Copy)]
pub(super) enum LirSequenceBuilderFinish {
    List,
    Tuple,
}

impl LirSequenceBuilderFinish {
    const fn finish_call(self) -> LirRuntimeCall {
        match self {
            Self::List => LirRuntimeCall::ListBuilderFinish,
            Self::Tuple => LirRuntimeCall::TupleBuilderFinish,
        }
    }
}

impl LirRuntimeArg {
    fn emit(&self, ctx: &mut LirLowerCtx) {
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
        LirRuntimeArg::BoxedOperand(operand).emit(ctx);
    }
    emit_lir_runtime_call_with_result(ctx, op, runtime_call);
}

pub(super) fn emit_lir_alloc(ctx: &mut LirLowerCtx, op: &LirOp) {
    if matches!(
        op.tir_op.attrs.get("arena_eligible"),
        Some(AttrValue::Bool(true))
    ) {
        ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
        emit_lir_runtime_result(ctx, op);
        return;
    }
    let size = required_i64_attr(op, "value", "Alloc");
    emit_lir_runtime_call_with_args_and_result(
        ctx,
        op,
        LirRuntimeCall::Alloc,
        &[LirRuntimeArg::I64Const(size)],
    );
}

pub(super) fn emit_lir_sequence_builder(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    finish: LirSequenceBuilderFinish,
) {
    let Some(result) = op.result_values.first() else {
        panic!("sequence builder op requires result");
    };
    let out = result.id;
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::ListBuilderNew,
        &[LirRuntimeArg::I64Const(box_int_bits(
            op.tir_op.operands.len() as i64,
        ))],
    );
    ctx.emit_set(out);

    for &operand in &op.tir_op.operands {
        ctx.emit_get(out);
        LirRuntimeArg::BoxedOperand(operand).emit(ctx);
        ctx.emit_runtime_call(LirRuntimeCall::ListBuilderAppend);
    }

    ctx.emit_get(out);
    emit_lir_runtime_call_with_result(ctx, op, finish.finish_call());
}

pub(super) fn emit_lir_build_dict(ctx: &mut LirLowerCtx, op: &LirOp) {
    if op.tir_op.operands.len() % 2 != 0 {
        panic!("BuildDict requires an even key/value operand count");
    }
    let Some(result) = op.result_values.first() else {
        panic!("BuildDict requires result");
    };
    let out = result.id;
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::DictNew,
        &[LirRuntimeArg::I64Const(
            (op.tir_op.operands.len() / 2) as i64,
        )],
    );
    ctx.emit_set(out);

    for pair in op.tir_op.operands.chunks(2) {
        ctx.emit_get(out);
        LirRuntimeArg::BoxedOperand(pair[0]).emit(ctx);
        LirRuntimeArg::BoxedOperand(pair[1]).emit(ctx);
        ctx.emit_runtime_call(LirRuntimeCall::DictSet);
        ctx.emit_set(out);
    }
}

pub(super) fn emit_lir_build_set(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(result) = op.result_values.first() else {
        panic!("BuildSet requires result");
    };
    let out = result.id;
    emit_lir_runtime_call_with_args(
        ctx,
        LirRuntimeCall::SetNew,
        &[LirRuntimeArg::I64Const(op.tir_op.operands.len() as i64)],
    );
    ctx.emit_set(out);

    for &operand in &op.tir_op.operands {
        ctx.emit_get(out);
        LirRuntimeArg::BoxedOperand(operand).emit(ctx);
        ctx.emit_runtime_call(LirRuntimeCall::SetAdd);
        ctx.instructions.push(Instruction::Drop);
    }
}

pub(super) fn emit_lir_attr(ctx: &mut LirLowerCtx, op: &LirOp) {
    let original_kind = original_kind(op);
    match (op.tir_op.opcode, original_kind) {
        (OpCode::LoadAttr, Some("get_attr_name")) => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::GetAttrName, 2)
        }
        (OpCode::StoreAttr, Some("set_attr_name")) => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::SetAttrName, 3)
        }
        (OpCode::DelAttr, Some("del_attr_name")) => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::DelAttrName, 2)
        }
        (OpCode::LoadAttr, Some("get_attr_generic_ptr")) => {
            let obj = required_operand(op, 0, "get_attr_generic_ptr");
            let name = required_name_bytes(op, "get_attr_generic_ptr");
            let name_len = name.len() as i64;
            emit_lir_runtime_call_with_args_and_result(
                ctx,
                op,
                LirRuntimeCall::GetAttrPtr,
                &[
                    LirRuntimeArg::ResolvedPtr32(obj),
                    LirRuntimeArg::DataPtrI32(name),
                    LirRuntimeArg::I64Const(name_len),
                ],
            );
        }
        (OpCode::LoadAttr, Some("get_attr_generic_obj")) => {
            let obj = required_operand(op, 0, "get_attr_generic_obj");
            let name = required_name_bytes(op, "get_attr_generic_obj");
            let name_len = name.len() as i64;
            let source_op_idx = required_source_op_index(op, "get_attr_generic_obj");
            let site_bits = box_int_bits(stable_ic_site_id(
                ctx.func.name.as_str(),
                source_op_idx,
                "get_attr_generic_obj",
            ));
            emit_lir_runtime_call_with_args_and_result(
                ctx,
                op,
                LirRuntimeCall::GetAttrObjectIc,
                &[
                    LirRuntimeArg::BoxedOperand(obj),
                    LirRuntimeArg::DataPtrI32(name),
                    LirRuntimeArg::I64Const(name_len),
                    LirRuntimeArg::I64Const(site_bits),
                ],
            );
        }
        (OpCode::LoadAttr, Some("get_attr_special_obj")) => {
            let obj = required_operand(op, 0, "get_attr_special_obj");
            let name = required_name_bytes(op, "get_attr_special_obj");
            let name_len = name.len() as i64;
            emit_lir_runtime_call_with_args_and_result(
                ctx,
                op,
                LirRuntimeCall::GetAttrSpecial,
                &[
                    LirRuntimeArg::BoxedOperand(obj),
                    LirRuntimeArg::DataPtrI32(name),
                    LirRuntimeArg::I64Const(name_len),
                ],
            );
        }
        (OpCode::StoreAttr, Some(kind @ ("set_attr_generic_ptr" | "set_attr_generic_obj"))) => {
            let obj = required_operand(op, 0, kind);
            let value = required_operand(op, 1, kind);
            let name = required_name_bytes(op, kind);
            let name_len = name.len() as i64;
            emit_lir_runtime_call_with_args_and_result(
                ctx,
                op,
                LirRuntimeCall::SetAttrObject,
                &[
                    LirRuntimeArg::BoxedOperand(obj),
                    LirRuntimeArg::DataPtrI32(name),
                    LirRuntimeArg::I64Const(name_len),
                    LirRuntimeArg::BoxedOperand(value),
                ],
            );
        }
        (OpCode::DelAttr, Some(kind @ ("del_attr_generic_ptr" | "del_attr_generic_obj"))) => {
            let obj = required_operand(op, 0, kind);
            let name = required_name_bytes(op, kind);
            let name_len = name.len() as i64;
            emit_lir_runtime_call_with_args_and_result(
                ctx,
                op,
                LirRuntimeCall::DelAttrObject,
                &[
                    LirRuntimeArg::BoxedOperand(obj),
                    LirRuntimeArg::DataPtrI32(name),
                    LirRuntimeArg::I64Const(name_len),
                ],
            );
        }
        _ => emit_lir_unsupported_marker(ctx, op),
    }
}

pub(super) fn emit_lir_object_new_bound(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(&class_ref) = op.tir_op.operands.first() else {
        panic!("ObjectNewBound requires class operand");
    };
    if let Some(payload_size) = positive_i64_attr(op, "value") {
        emit_lir_runtime_call_with_args_and_result(
            ctx,
            op,
            LirRuntimeCall::ObjectNewBoundSized,
            &[
                LirRuntimeArg::BoxedOperand(class_ref),
                LirRuntimeArg::I64Const(payload_size),
            ],
        );
    } else {
        emit_lir_runtime_call_with_args_and_result(
            ctx,
            op,
            LirRuntimeCall::ObjectNewBound,
            &[LirRuntimeArg::BoxedOperand(class_ref)],
        );
    }
}

pub(super) fn emit_lir_closure_load(ctx: &mut LirLowerCtx, op: &LirOp) {
    let Some(&closure) = op.tir_op.operands.first() else {
        panic!("ClosureLoad requires closure operand");
    };
    let offset = required_i64_attr(op, "value", "ClosureLoad");
    emit_lir_runtime_call_with_args_and_result(
        ctx,
        op,
        LirRuntimeCall::ClosureLoad,
        &[
            LirRuntimeArg::ResolvedPtr32(closure),
            LirRuntimeArg::I64Const(offset),
        ],
    );
}

pub(super) fn emit_lir_closure_store(ctx: &mut LirLowerCtx, op: &LirOp) {
    if op.tir_op.operands.len() < 2 {
        panic!("ClosureStore requires closure and value operands");
    }
    let offset = required_i64_attr(op, "value", "ClosureStore");
    emit_lir_runtime_call_with_args_and_result(
        ctx,
        op,
        LirRuntimeCall::ClosureStore,
        &[
            LirRuntimeArg::ResolvedPtr32(op.tir_op.operands[0]),
            LirRuntimeArg::I64Const(offset),
            LirRuntimeArg::BoxedOperand(op.tir_op.operands[1]),
        ],
    );
}

fn emit_lir_runtime_call_with_args_and_result(
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

fn emit_lir_runtime_call_with_args(
    ctx: &mut LirLowerCtx,
    runtime_call: LirRuntimeCall,
    args: &[LirRuntimeArg],
) {
    for arg in args {
        arg.emit(ctx);
    }
    ctx.emit_runtime_call(runtime_call);
}

fn emit_lir_runtime_call_with_result(
    ctx: &mut LirLowerCtx,
    op: &LirOp,
    runtime_call: LirRuntimeCall,
) {
    ctx.emit_runtime_call(runtime_call);
    emit_lir_runtime_result(ctx, op);
}

fn positive_i64_attr(op: &LirOp, attr: &str) -> Option<i64> {
    match op.tir_op.attrs.get(attr) {
        Some(AttrValue::Int(value)) if *value > 0 => Some(*value),
        _ => None,
    }
}

fn required_i64_attr(op: &LirOp, attr: &str, op_name: &str) -> i64 {
    match op.tir_op.attrs.get(attr) {
        Some(AttrValue::Int(value)) => *value,
        _ => panic!("{op_name} requires integer attr {attr}"),
    }
}

fn required_name_bytes(op: &LirOp, op_name: &str) -> Arc<[u8]> {
    match op.tir_op.attrs.get("name") {
        Some(AttrValue::Str(name)) => Arc::from(name.as_bytes()),
        _ => panic!("{op_name} requires string attr name"),
    }
}

fn required_operand(op: &LirOp, index: usize, op_name: &str) -> ValueId {
    op.tir_op
        .operands
        .get(index)
        .copied()
        .unwrap_or_else(|| panic!("{op_name} requires operand {index}"))
}

fn required_source_op_index(op: &LirOp, op_name: &str) -> usize {
    op.tir_op
        .source_op_index()
        .unwrap_or_else(|| panic!("{op_name} requires source op index"))
}

fn original_kind(op: &LirOp) -> Option<&str> {
    match op.tir_op.attrs.get("_original_kind") {
        Some(AttrValue::Str(kind)) => Some(kind.as_str()),
        _ => None,
    }
}

pub(super) fn emit_lir_unsupported_marker(ctx: &mut LirLowerCtx, op: &LirOp) {
    for &operand in &op.tir_op.operands {
        ctx.emit_get(operand);
    }
    ctx.emit_bail_to_generic_path(WasmLirFallbackReason::UnsupportedOperation);
    if let Some(result) = op.result_values.first() {
        ctx.emit_set(result.id);
    }
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
