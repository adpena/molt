use super::super::result_sink::store_result_or_drop;
use super::RuntimeServiceOpContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_file_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "file_open" => {
            let args = op.args.as_ref().unwrap();
            let path = locals[&args[0]];
            let mode = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(path));
            func.instruction(&Instruction::LocalGet(mode));
            emit_call(func, reloc_enabled, import_ids["file_open"]);
            store_result_or_drop(func, op, locals);
        }
        "file_read" => {
            let args = op.args.as_ref().unwrap();
            let handle = locals[&args[0]];
            let size = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(handle));
            func.instruction(&Instruction::LocalGet(size));
            emit_call(func, reloc_enabled, import_ids["file_read"]);
            store_result_or_drop(func, op, locals);
        }
        "file_write" => {
            let args = op.args.as_ref().unwrap();
            let handle = locals[&args[0]];
            let data = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(handle));
            func.instruction(&Instruction::LocalGet(data));
            emit_call(func, reloc_enabled, import_ids["file_write"]);
            store_result_or_drop(func, op, locals);
        }
        "file_close" => {
            let args = op.args.as_ref().unwrap();
            let handle = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(handle));
            emit_call(func, reloc_enabled, import_ids["file_close"]);
            store_result_or_drop(func, op, locals);
        }
        "file_flush" => {
            let args = op.args.as_ref().unwrap();
            let handle = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(handle));
            emit_call(func, reloc_enabled, import_ids["file_flush"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
