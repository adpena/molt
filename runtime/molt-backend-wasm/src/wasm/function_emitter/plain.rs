use crate::FunctionIR;
use crate::wasm::control_flow::ControlKind;
use crate::wasm::op_loop::WasmFunctionEmitContext;
use std::collections::{BTreeMap, BTreeSet};
use wasm_encoder::{BlockType, Function, Instruction};

pub(super) fn emit_plain_function_body(
    func_ir: &FunctionIR,
    func: &mut Function,
    op_emitter: &mut WasmFunctionEmitContext<'_, '_>,
) {
    let mut control_stack: Vec<ControlKind> = Vec::new();
    let mut try_stack: Vec<usize> = Vec::new();
    let mut label_stack: Vec<i64> = Vec::new();
    let mut label_depths: BTreeMap<i64, usize> = BTreeMap::new();

    let mut branch_target_labels: BTreeSet<i64> = BTreeSet::new();
    let mut label_order: Vec<i64> = Vec::new();
    for op in &func_ir.ops {
        match op.kind.as_str() {
            "jump" | "br_if" | "check_exception" | "async_work_poll" => {
                if let Some(label_id) = op.value {
                    branch_target_labels.insert(label_id);
                }
            }
            "label" => {
                if let Some(label_id) = op.value {
                    label_order.push(label_id);
                }
            }
            _ => {}
        }
    }
    let label_ids: Vec<i64> = label_order
        .into_iter()
        .filter(|label_id| branch_target_labels.contains(label_id))
        .collect();
    if !label_ids.is_empty() {
        for label_id in label_ids.iter().rev() {
            func.instruction(&Instruction::Block(BlockType::Empty));
            control_stack.push(ControlKind::Block);
            label_depths.insert(*label_id, control_stack.len() - 1);
            label_stack.push(*label_id);
        }
    }
    op_emitter.emit_ops(
        func,
        &func_ir.ops,
        &mut control_stack,
        &mut try_stack,
        &mut label_stack,
        &mut label_depths,
        0,
    );
    while !label_stack.is_empty() {
        label_stack.pop();
        func.instruction(&Instruction::End);
        control_stack.pop();
    }
    op_emitter
        .frame
        .emit_implicit_return(func, op_emitter.reloc_enabled, op_emitter.import_ids);
}
