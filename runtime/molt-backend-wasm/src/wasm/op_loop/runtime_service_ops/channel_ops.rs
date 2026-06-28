use super::super::result_sink::store_result_or_drop;
use super::RuntimeServiceOpContext;
use crate::OpIR;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_channel_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "chan_new" => {
            let args = op.args.as_ref().unwrap();
            let cap = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(cap));
            emit_call(func, reloc_enabled, import_ids["chan_new"]);
            store_result_or_drop(func, op, locals);
        }
        "chan_drop" => {
            let args = op.args.as_ref().unwrap();
            let chan = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(chan));
            emit_call(func, reloc_enabled, import_ids["chan_drop"]);
            func.instruction(&Instruction::Drop);
        }
        _ => return false,
    }
    true
}
