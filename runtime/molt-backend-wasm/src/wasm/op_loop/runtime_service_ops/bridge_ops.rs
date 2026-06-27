use super::super::result_sink::store_result_or_drop;
use super::*;

pub(super) fn emit_bridge_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "bridge_unavailable" => {
            let args = op.args.as_ref().unwrap();
            let msg = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(msg));
            emit_call(func, reloc_enabled, import_ids["bridge_unavailable"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
