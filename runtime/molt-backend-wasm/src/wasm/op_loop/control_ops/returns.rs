use super::ControlOpContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_return_control_op(
    context: &ControlOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    match molt_ir::tir::op_kinds_generated::simpleir_return_shape(op.kind.as_str()) {
        molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::Value => emit_ret(context, func, op),
        molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::Void => {
            emit_arena_free(context, func);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::Return);
        }
        molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::NotReturn
            if op.kind == "unreachable" =>
        {
            func.instruction(&Instruction::Unreachable);
        }
        molt_ir::tir::op_kinds_generated::SimpleIrReturnShape::NotReturn => return false,
    }
    true
}

fn emit_ret(context: &ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    let ret_var = op.args.as_ref().and_then(|args| args.first());
    let ret_local = ret_var.and_then(|name| context.locals.get(name).copied());
    if let Some(local_idx) = ret_local {
        func.instruction(&Instruction::LocalGet(local_idx));
    } else {
        super::dispatch_control_panic(
            &context.func_ir.name,
            context.op_idx,
            format_args!("ret target args {:?} are not present", op.args),
        );
    }
    emit_arena_free(context, func);
    func.instruction(&Instruction::Return);
}

fn emit_arena_free(context: &ControlOpContext<'_>, func: &mut Function) {
    if let Some(arena_idx) = context.arena_local {
        func.instruction(&Instruction::LocalGet(arena_idx));
        emit_call(
            func,
            context.reloc_enabled,
            context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ArenaFree],
        );
    }
}
