use super::ControlOpContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_return_control_op(
    context: &ControlOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        "ret" => emit_ret(context, func, op),
        "ret_void" => {
            emit_arena_free(context, func);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::Return);
        }
        "unreachable" => {
            func.instruction(&Instruction::Unreachable);
        }
        _ => return false,
    }
    true
}

fn emit_ret(context: &ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    let ret_var = op.var.as_ref();
    let callee_value_locals = context.multi_return.callee_value_locals();
    if ret_var.is_some_and(|v| context.multi_return.is_callee_tuple_var(v))
        && !callee_value_locals.is_empty()
    {
        for &local_idx in callee_value_locals {
            func.instruction(&Instruction::LocalGet(local_idx));
        }
    } else {
        let ret_local = ret_var.and_then(|name| context.locals.get(name).copied());
        if let Some(local_idx) = ret_local {
            func.instruction(&Instruction::LocalGet(local_idx));
        } else {
            super::dispatch_control_panic(
                &context.func_ir.name,
                context.op_idx,
                format_args!("ret target local {:?} is not present", op.var),
            );
        }
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
