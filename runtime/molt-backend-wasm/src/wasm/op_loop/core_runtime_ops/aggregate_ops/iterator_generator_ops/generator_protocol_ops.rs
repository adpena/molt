use super::super::super::super::result_sink::store_result_or_drop;
use super::super::super::super::*;
use super::super::AggregateRuntimeContext;

pub(super) fn emit_generator_protocol_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "asyncgen_new" => {
            let args = op.args.as_ref().unwrap();
            let gen_local = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(gen_local));
            emit_call(func, reloc_enabled, import_ids["asyncgen_new"]);
            store_result_or_drop(func, op, locals);
        }
        "asyncgen_shutdown" => {
            emit_call(func, reloc_enabled, import_ids["asyncgen_shutdown"]);
            store_result_or_drop(func, op, locals);
        }
        "gen_send" => {
            let args = op.args.as_ref().unwrap();
            let gen_local = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(gen_local));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["generator_send"]);
            store_result_or_drop(func, op, locals);
        }
        "gen_throw" => {
            let args = op.args.as_ref().unwrap();
            let gen_local = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(gen_local));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["generator_throw"]);
            store_result_or_drop(func, op, locals);
        }
        "gen_close" => {
            let args = op.args.as_ref().unwrap();
            let gen_local = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(gen_local));
            emit_call(func, reloc_enabled, import_ids["generator_close"]);
            store_result_or_drop(func, op, locals);
        }
        "is_generator" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["is_generator"]);
            store_result_or_drop(func, op, locals);
        }
        "is_callable" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["is_callable"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
