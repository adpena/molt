use super::super::super::result_sink::store_result_or_drop;
use super::super::super::*;
use super::AggregateRuntimeContext;

pub(super) fn emit_callargs_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "callargs_new" => {
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Const(0));
            emit_call(func, reloc_enabled, import_ids["callargs_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "callargs_push_pos" => {
            let args = op.args.as_ref().unwrap();
            let builder_ptr = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(builder_ptr));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
            store_result_or_drop(func, op, locals);
        }
        "callargs_push_kw" => {
            let args = op.args.as_ref().unwrap();
            let builder_ptr = locals[&args[0]];
            let name = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(builder_ptr));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["callargs_push_kw"]);
            store_result_or_drop(func, op, locals);
        }
        "callargs_expand_star" => {
            let args = op.args.as_ref().unwrap();
            let builder_ptr = locals[&args[0]];
            let iterable = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(builder_ptr));
            func.instruction(&Instruction::LocalGet(iterable));
            emit_call(func, reloc_enabled, import_ids["callargs_expand_star"]);
            store_result_or_drop(func, op, locals);
        }
        "callargs_expand_kwstar" => {
            let args = op.args.as_ref().unwrap();
            let builder_ptr = locals[&args[0]];
            let mapping = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(builder_ptr));
            func.instruction(&Instruction::LocalGet(mapping));
            emit_call(func, reloc_enabled, import_ids["callargs_expand_kwstar"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
