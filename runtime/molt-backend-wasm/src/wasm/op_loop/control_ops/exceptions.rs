use super::{ControlKind, ControlOpContext};
use crate::OpIR;
use crate::wasm_abi::TAG_EXCEPTION_INDEX;
use crate::wasm_binary::emit_call;
use std::borrow::Cow;
use wasm_encoder::{BlockType, Catch, Function, Instruction, ValType};

pub(super) fn emit_exception_control_op(
    context: &mut ControlOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        "try_start" => emit_try_start(context, func),
        "try_end" => emit_try_end(context, func),
        "check_exception" => emit_check_exception(context, func),
        _ => return false,
    }
    true
}

fn emit_try_start(context: &mut ControlOpContext<'_>, func: &mut Function) {
    if context.native_eh_enabled {
        func.instruction(&Instruction::Block(BlockType::Result(ValType::I64)));
        context.control_stack.push(ControlKind::Block);
        func.instruction(&Instruction::TryTable(
            BlockType::Empty,
            Cow::Borrowed(&[Catch::One {
                tag: TAG_EXCEPTION_INDEX,
                label: 0,
            }]),
        ));
        context.control_stack.push(ControlKind::Try);
        context.try_stack.push(context.control_stack.len() - 1);
    } else {
        func.instruction(&Instruction::Block(BlockType::Empty));
        context.control_stack.push(ControlKind::Try);
        context.try_stack.push(context.control_stack.len() - 1);
    }
}

fn emit_try_end(context: &mut ControlOpContext<'_>, func: &mut Function) {
    if context.native_eh_enabled {
        func.instruction(&Instruction::End);
        context.control_stack.pop();
        context.try_stack.pop();
        context.const_cache.emit_none(func);
        func.instruction(&Instruction::End);
        context.control_stack.pop();
        func.instruction(&Instruction::Drop);
    } else {
        func.instruction(&Instruction::End);
        context.control_stack.pop();
        context.try_stack.pop();
    }
}

fn emit_check_exception(context: &ControlOpContext<'_>, func: &mut Function) {
    if context.native_eh_enabled
        || context
            .exception_handler_region_indices
            .contains(&context.op_idx)
    {
        return;
    }

    if let Some(&try_index) = context.try_stack.last() {
        emit_call(
            func,
            context.reloc_enabled,
            context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending],
        );
        func.instruction(&Instruction::I64Const(0));
        func.instruction(&Instruction::I64Ne);
        let depth = context.control_stack.len().saturating_sub(1 + try_index);
        func.instruction(&Instruction::BrIf(depth as u32));
    }
}
