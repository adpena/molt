use super::super::super::super::result_sink::store_result_or_drop;
use super::super::super::super::*;
use super::super::AggregateRuntimeContext;

pub(super) fn emit_iterator_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "iter" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["iter"]);
            store_result_or_drop(func, op, locals);
        }
        "enumerate" => {
            let args = op.args.as_ref().unwrap();
            let iterable = locals[&args[0]];
            let start = locals[&args[1]];
            let has_start = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(iterable));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(has_start));
            emit_call(func, reloc_enabled, import_ids["enumerate"]);
            store_result_or_drop(func, op, locals);
        }
        "aiter" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["aiter"]);
            store_result_or_drop(func, op, locals);
        }
        "iter_next_unboxed" => {
            let args = op.args.as_ref().unwrap();
            let iter = locals[&args[0]];
            let pair = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(func, reloc_enabled, import_ids["iter_next"]);
            func.instruction(&Instruction::LocalSet(pair));
            if let Some(done_name) = op.out.as_ref()
                && done_name != "none"
            {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::I64Const(box_int(1)));
                emit_call(func, reloc_enabled, import_ids["index"]);
                func.instruction(&Instruction::LocalSet(locals[done_name]));
            }
            if let Some(val_name) = op.var.as_ref()
                && val_name != "none"
            {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::I64Const(box_int(0)));
                emit_call(func, reloc_enabled, import_ids["index"]);
                func.instruction(&Instruction::LocalSet(locals[val_name]));
            }
            func.instruction(&Instruction::LocalGet(pair));
            emit_call(func, reloc_enabled, import_ids["dec_ref_obj"]);
        }
        "iter_next" => {
            let args = op.args.as_ref().unwrap();
            let iter = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(func, reloc_enabled, import_ids["iter_next"]);
            store_result_or_drop(func, op, locals);
        }
        "anext" => {
            let args = op.args.as_ref().unwrap();
            let iter = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(iter));
            emit_call(func, reloc_enabled, import_ids["anext"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
