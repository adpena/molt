use super::super::lir_context::LirLowerCtx;
use super::super::runtime_calls::LirRuntimeCall;
use super::call_abi::{
    LirRuntimeArg, emit_lir_boxed_operands_runtime_call,
    emit_lir_runtime_call_with_args_and_result, original_kind, required_name_bytes,
    required_operand, required_source_op_index,
};
use super::fallback_ops::emit_lir_unsupported_marker;
use molt_codegen_abi::{box_int_bits, stable_ic_site_id};
use molt_tir::tir::lir::LirOp;
use molt_tir::tir::ops::OpCode;

pub(in crate::wasm::lir_fast) fn emit_lir_attr(ctx: &mut LirLowerCtx, op: &LirOp) {
    let original_kind = original_kind(op);
    match (op.tir_op.opcode, original_kind) {
        (OpCode::LoadAttr, Some(molt_ir::OpKind::GetAttrName)) => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::GetAttrName)
        }
        (OpCode::StoreAttr, Some(molt_ir::OpKind::SetAttrName)) => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::SetAttrName)
        }
        (OpCode::DelAttr, Some(molt_ir::OpKind::DelAttrName)) => {
            emit_lir_boxed_operands_runtime_call(ctx, op, LirRuntimeCall::DelAttrName)
        }
        (
            OpCode::LoadAttr,
            Some(kind @ (molt_ir::OpKind::GetAttrGenericPtr | molt_ir::OpKind::GetAttrGenericObj)),
        ) => {
            let obj = required_operand(op, 0, kind);
            let name = required_name_bytes(op, kind);
            let name_len = name.len() as i64;
            let source_op_idx = required_source_op_index(op, kind);
            let site_bits = box_int_bits(stable_ic_site_id(
                ctx.func.name.as_str(),
                source_op_idx,
                kind.as_str(),
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
        (OpCode::LoadAttr, Some(molt_ir::OpKind::GetAttrSpecialObj)) => {
            let obj = required_operand(op, 0, molt_ir::OpKind::GetAttrSpecialObj);
            let name = required_name_bytes(op, molt_ir::OpKind::GetAttrSpecialObj);
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
        (
            OpCode::StoreAttr,
            Some(kind @ (molt_ir::OpKind::SetAttrGenericPtr | molt_ir::OpKind::SetAttrGenericObj)),
        ) => {
            let obj = required_operand(op, 0, kind);
            let value = required_operand(op, 1, kind);
            let name = required_name_bytes(op, kind);
            let name_len = name.len() as i64;
            let source_op_idx = required_source_op_index(op, kind);
            let site_bits = box_int_bits(stable_ic_site_id(
                ctx.func.name.as_str(),
                source_op_idx,
                kind.as_str(),
            ));
            emit_lir_runtime_call_with_args_and_result(
                ctx,
                op,
                LirRuntimeCall::SetAttrObjectIc,
                &[
                    LirRuntimeArg::BoxedOperand(obj),
                    LirRuntimeArg::DataPtrI32(name),
                    LirRuntimeArg::I64Const(name_len),
                    LirRuntimeArg::BoxedOperand(value),
                    LirRuntimeArg::I64Const(site_bits),
                ],
            );
        }
        (
            OpCode::DelAttr,
            Some(kind @ (molt_ir::OpKind::DelAttrGenericPtr | molt_ir::OpKind::DelAttrGenericObj)),
        ) => {
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
