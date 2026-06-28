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
