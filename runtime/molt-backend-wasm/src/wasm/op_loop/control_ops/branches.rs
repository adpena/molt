use super::{ControlKind, ControlOpContext};
use crate::OpIR;
use crate::wasm_plan::wasm_scalar_truthiness_fast_path_for_name;
use crate::wasm_values::emit_branch_truthiness_i32;
use wasm_encoder::{BlockType, Function, Instruction};

pub(super) fn emit_branch_control_op(
    context: &mut ControlOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    match op.kind.as_str() {
        "jump" => emit_jump(context, func, op),
        "br_if" => emit_br_if(context, func, op),
        "if" => emit_if(context, func, op),
        "label" => emit_label(context, func, op),
        "else" => {
            func.instruction(&Instruction::Else);
        }
        "end_if" => {
            func.instruction(&Instruction::End);
            context.control_stack.pop();
        }
        _ => return false,
    }
    true
}

fn emit_jump(context: &ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    let target = op.value.expect("jump missing label");
    let depth = label_branch_depth(context, target, "jump");
    func.instruction(&Instruction::Br(depth));
}

fn emit_br_if(context: &ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().unwrap();
    let cond = context.locals[&args[0]];
    let target = op.value.expect("br_if missing label");
    let depth = label_branch_depth(context, target, "br_if");
    emit_branch_truthiness_i32(
        func,
        cond,
        context.import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
        context.reloc_enabled,
    );
    func.instruction(&Instruction::BrIf(depth));
}

fn emit_if(context: &mut ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    let args = op.args.as_ref().unwrap();
    let cond = context.locals[&args[0]];
    let truthy_import = if wasm_scalar_truthiness_fast_path_for_name(context.scalar_plan, &args[0])
    {
        crate::wasm_abi_generated::WasmRuntimeImport::IsTruthyInt
    } else {
        crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy
    };
    emit_branch_truthiness_i32(
        func,
        cond,
        context.import_ids[truthy_import],
        context.reloc_enabled,
    );
    func.instruction(&Instruction::If(BlockType::Empty));
    context.control_stack.push(ControlKind::If);
}

fn emit_label(context: &mut ControlOpContext<'_>, func: &mut Function, op: &OpIR) {
    if let Some(label_id) = op.value
        && let Some(top) = context.label_stack.last().copied()
        && top == label_id
    {
        context.label_stack.pop();
        context.label_depths.remove(&label_id);
        func.instruction(&Instruction::End);
        context.control_stack.pop();
    }
}

pub(super) fn label_branch_depth(context: &ControlOpContext<'_>, target: i64, kind: &str) -> u32 {
    context
        .label_depths
        .get(&target)
        .map(|idx| context.control_stack.len().saturating_sub(1 + idx) as u32)
        .unwrap_or_else(|| panic!("{kind} target {} missing label block", target))
}
