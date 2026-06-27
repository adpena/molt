use super::super::super::result_sink::store_result_or_drop;
use super::super::super::*;
use super::AggregateRuntimeContext;

pub(super) fn emit_dict_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "dict_new" => {
            let empty_args_dn: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_dn);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::I64Const((args.len() / 2) as i64));
            emit_call(func, reloc_enabled, import_ids["dict_new"]);
            func.instruction(&Instruction::LocalSet(out));
            for pair in args.chunks(2) {
                let key = locals[&pair[0]];
                let val = locals[&pair[1]];
                func.instruction(&Instruction::LocalGet(out));
                func.instruction(&Instruction::LocalGet(key));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(func, reloc_enabled, import_ids["dict_set"]);
                func.instruction(&Instruction::LocalSet(out));
            }
        }
        "dict_from_obj" => {
            let args = op.args.as_ref().unwrap();
            let obj = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(obj));
            emit_call(func, reloc_enabled, import_ids["dict_from_obj"]);
            let out = locals[op.out.as_ref().unwrap()];
            func.instruction(&Instruction::LocalSet(out));
        }
        "dict_get" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let default = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(default));
            emit_call(func, reloc_enabled, import_ids["dict_get"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_inc" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let delta = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(delta));
            emit_call(func, reloc_enabled, import_ids["dict_inc"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_str_int_inc" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let delta = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(delta));
            emit_call(func, reloc_enabled, import_ids["dict_str_int_inc"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_pop" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let default = locals[&args[2]];
            let has_default = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(default));
            func.instruction(&Instruction::LocalGet(has_default));
            emit_call(func, reloc_enabled, import_ids["dict_pop"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_setdefault" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            let default = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            func.instruction(&Instruction::LocalGet(default));
            emit_call(func, reloc_enabled, import_ids["dict_setdefault"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_setdefault_empty_list" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let key = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(key));
            emit_call(
                func,
                reloc_enabled,
                import_ids["dict_setdefault_empty_list"],
            );
            store_result_or_drop(func, op, locals);
        }
        "dict_update" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["dict_update"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_clear" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_clear"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_copy" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_copy"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_popitem" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_popitem"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_update_kwstar" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(dict));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["dict_update_kwstar"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_keys" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_keys"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_values" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_values"]);
            store_result_or_drop(func, op, locals);
        }
        "dict_items" => {
            let args = op.args.as_ref().unwrap();
            let dict = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(dict));
            emit_call(func, reloc_enabled, import_ids["dict_items"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
