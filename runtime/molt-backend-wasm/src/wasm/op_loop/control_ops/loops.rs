use super::{ControlKind, ControlOpContext};
use crate::OpIR;
use crate::wasm_binary::emit_call;
use crate::wasm_values::emit_branch_truthiness_i32;
use wasm_encoder::{BlockType, Function, Instruction};

pub(super) fn emit_loop_control_op(
    context: &mut ControlOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        "loop_start" => emit_loop_start(context, func),
        "loop_index_start" | "loop_index_next" => emit_loop_index_move(context, func, op),
        "loop_break_if_true" => emit_loop_break_if_truthy(context, func, op, false),
        "loop_break_if_false" => emit_loop_break_if_truthy(context, func, op, true),
        "loop_break_if_exception" => emit_loop_break_if_exception(context, func),
        "loop_break" => emit_loop_break(context, func),
        "loop_continue" => emit_loop_continue(context, func),
        "loop_end" => emit_loop_end(context, func),
        _ => return false,
    }
    true
}

fn emit_loop_start(context: &mut ControlOpContext<'_>, func: &mut Function) {
    func.instruction(&Instruction::Block(BlockType::Empty));
    func.instruction(&Instruction::Loop(BlockType::Empty));
    context.control_stack.push(ControlKind::Block);
    context.control_stack.push(ControlKind::Loop);
}

fn emit_loop_index_move(context: &ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().unwrap();
    let source = context.locals[&args[0]];
    let out = context.locals[op.out.as_ref().unwrap()];
    func.instruction(&Instruction::LocalGet(source));
    func.instruction(&Instruction::LocalSet(out));
}

fn emit_loop_break_if_truthy(
    context: &ControlOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
    invert: bool,
) {
    let args = op.args.as_ref().unwrap();
    let cond = context.locals[&args[0]];
    emit_branch_truthiness_i32(
        func,
        cond,
        context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
        context.reloc_enabled,
    );
    if invert {
        func.instruction(&Instruction::I32Eqz);
    }
    emit_loop_break_if(context, func, op.kind.as_str());
}

fn emit_loop_break_if_exception(context: &ControlOpContext<'_>, func: &mut Function) {
    emit_call(
        func,
        context.reloc_enabled,
        context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::ExceptionPending],
    );
    func.instruction(&Instruction::I64Const(0));
    func.instruction(&Instruction::I64Ne);
    emit_loop_break_if(context, func, "loop_break_if_exception");
}

fn emit_loop_break(context: &ControlOpContext<'_>, func: &mut Function) {
    let depth = loop_break_depth(context).unwrap_or_else(|| {
        super::dispatch_control_panic(
            &context.func_ir.name,
            context.op_idx,
            "loop_break without loop",
        )
    });
    func.instruction(&Instruction::Br(depth));
}

fn emit_loop_continue(context: &ControlOpContext<'_>, func: &mut Function) {
    let depth = loop_continue_depth(context).unwrap_or_else(|| {
        super::dispatch_control_panic(
            &context.func_ir.name,
            context.op_idx,
            "loop_continue without loop",
        )
    });
    func.instruction(&Instruction::Br(depth));
}

fn emit_loop_end(context: &mut ControlOpContext<'_>, func: &mut Function) {
    func.instruction(&Instruction::End);
    func.instruction(&Instruction::End);
    context.control_stack.pop();
    context.control_stack.pop();
}

fn emit_loop_break_if(context: &ControlOpContext<'_>, func: &mut Function, kind: &str) {
    let depth = loop_break_depth(context).unwrap_or_else(|| {
        super::dispatch_control_panic(
            &context.func_ir.name,
            context.op_idx,
            format_args!("{kind} without loop"),
        )
    });
    func.instruction(&Instruction::BrIf(depth));
}

fn loop_break_depth(context: &ControlOpContext<'_>) -> Option<u32> {
    super::super::super::control_flow::loop_break_depth(context.control_stack)
}

fn loop_continue_depth(context: &ControlOpContext<'_>) -> Option<u32> {
    super::super::super::control_flow::loop_continue_depth(context.control_stack)
}
