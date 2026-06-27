use super::super::super::result_sink::store_result_or_drop;
use super::super::super::*;
use super::AggregateRuntimeContext;

pub(super) fn emit_container_query_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "contains" => {
            let args = op.args.as_ref().unwrap();
            let container = locals[&args[0]];
            let item = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(container));
            func.instruction(&Instruction::LocalGet(item));
            let import_key =
                wasm_specialized_container_import(ctx.scalar_plan, ctx.op_idx, "contains", op)
                    .unwrap_or("contains");
            let import_id =
                selected_import_id(import_ids, import_key, &ctx.func_ir.name, op.kind.as_str());
            emit_call(func, reloc_enabled, import_id);
            store_result_or_drop(func, op, locals);
        }
        "len" => {
            let args = op.args.as_ref().unwrap();
            let arg = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(arg));
            // Dispatch to specialized fast-path len when container
            // type is known, skipping the 18-type dispatch.
            let import_key =
                wasm_specialized_container_import(ctx.scalar_plan, ctx.op_idx, "len", op)
                    .unwrap_or("len");
            let import_id =
                selected_import_id(import_ids, import_key, &ctx.func_ir.name, op.kind.as_str());
            emit_call(func, reloc_enabled, import_id);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
