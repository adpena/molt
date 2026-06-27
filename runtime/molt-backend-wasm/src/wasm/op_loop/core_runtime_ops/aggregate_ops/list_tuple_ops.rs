use super::super::super::builder_ops::{BuilderFinish, emit_sequence_builder_from_args};
use super::super::super::result_sink::store_result_or_drop;
use super::super::super::*;
use super::AggregateRuntimeContext;

pub(super) fn emit_list_tuple_op(
    func: &mut Function,
    op: &OpIR,
    ctx: &AggregateRuntimeContext<'_>,
) -> bool {
    let import_ids = ctx.import_ids;
    let locals = ctx.locals;
    let reloc_enabled = ctx.reloc_enabled;

    match op.kind.as_str() {
        "list_from_range" => {
            let args = op.args.as_ref().unwrap();
            let out = locals[op.out.as_ref().unwrap()];
            let start = locals[&args[0]];
            let stop = locals[&args[1]];
            let step = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(stop));
            func.instruction(&Instruction::LocalGet(step));
            emit_call(func, reloc_enabled, import_ids["list_from_range"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "build_list" | "list_new" => {
            let empty_args_ln: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args_ln);
            let out = locals[op.out.as_ref().unwrap()];
            emit_sequence_builder_from_args(
                func,
                args,
                out,
                import_ids,
                locals,
                reloc_enabled,
                BuilderFinish::List,
            );
        }
        "list_int_new" => {
            // Specialized flat i64 list: args = [count, fill_value]
            let args = op.args.as_ref().unwrap();
            let out = locals[op.out.as_ref().unwrap()];
            let count = locals[&args[0]];
            let fill = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(count));
            func.instruction(&Instruction::LocalGet(fill));
            emit_call(func, reloc_enabled, import_ids["list_int_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "list_fill_new" => {
            let args = op.args.as_ref().unwrap();
            let out = locals[op.out.as_ref().unwrap()];
            let count = locals[&args[0]];
            let fill = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(count));
            func.instruction(&Instruction::LocalGet(fill));
            emit_call(func, reloc_enabled, import_ids["list_fill_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "range_new" => {
            let args = op.args.as_ref().unwrap();
            let out = locals[op.out.as_ref().unwrap()];
            let start = locals[&args[0]];
            let stop = locals[&args[1]];
            let step = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(stop));
            func.instruction(&Instruction::LocalGet(step));
            emit_call(func, reloc_enabled, import_ids["range_new"]);
            func.instruction(&Instruction::LocalSet(out));
        }
        "tuple_new" => {
            let empty_args: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args);
            let out_name = op.out.as_ref().unwrap();
            let out = locals[out_name];
            // Multi-value return (Section 3.1): store elements
            // into __multi_ret_N locals instead of heap-allocating
            // when this tuple flows directly to a return in a
            // candidate function.
            let callee_value_locals = ctx.multi_return.callee_value_locals();
            if ctx.multi_return.is_callee_tuple_var(out_name)
                && args.len() == callee_value_locals.len()
            {
                for (k, arg_name) in args.iter().enumerate() {
                    let val = locals[arg_name];
                    func.instruction(&Instruction::LocalGet(val));
                    func.instruction(&Instruction::LocalSet(callee_value_locals[k]));
                }
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::LocalSet(out));
            } else {
                emit_sequence_builder_from_args(
                    func,
                    args,
                    out,
                    import_ids,
                    locals,
                    reloc_enabled,
                    BuilderFinish::Tuple,
                );
            }
        }
        "list_append" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_append"]);
            store_result_or_drop(func, op, locals);
        }
        "list_pop" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let idx = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(idx));
            emit_call(func, reloc_enabled, import_ids["list_pop"]);
            store_result_or_drop(func, op, locals);
        }
        "list_extend" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let other = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(other));
            emit_call(func, reloc_enabled, import_ids["list_extend"]);
            store_result_or_drop(func, op, locals);
        }
        "list_insert" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let idx = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(idx));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_insert"]);
            store_result_or_drop(func, op, locals);
        }
        "list_remove" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_remove"]);
            store_result_or_drop(func, op, locals);
        }
        "list_clear" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(list));
            emit_call(func, reloc_enabled, import_ids["list_clear"]);
            store_result_or_drop(func, op, locals);
        }
        "list_copy" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(list));
            emit_call(func, reloc_enabled, import_ids["list_copy"]);
            store_result_or_drop(func, op, locals);
        }
        "list_reverse" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(list));
            emit_call(func, reloc_enabled, import_ids["list_reverse"]);
            store_result_or_drop(func, op, locals);
        }
        "list_count" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_count"]);
            store_result_or_drop(func, op, locals);
        }
        "list_index" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["list_index"]);
            store_result_or_drop(func, op, locals);
        }
        "list_index_range" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            let val = locals[&args[1]];
            let start = locals[&args[2]];
            let stop = locals[&args[3]];
            func.instruction(&Instruction::LocalGet(list));
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::LocalGet(start));
            func.instruction(&Instruction::LocalGet(stop));
            emit_call(func, reloc_enabled, import_ids["list_index_range"]);
            store_result_or_drop(func, op, locals);
        }
        "tuple_from_list" => {
            let args = op.args.as_ref().unwrap();
            let list = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(list));
            emit_call(func, reloc_enabled, import_ids["tuple_from_list"]);
            store_result_or_drop(func, op, locals);
        }
        "tuple_count" => {
            let args = op.args.as_ref().unwrap();
            let tuple = locals[&args[0]];
            let val = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(tuple));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["tuple_count"]);
            store_result_or_drop(func, op, locals);
        }
        "tuple_index" => {
            let args = op.args.as_ref().unwrap();
            let tuple_var = &args[0];
            let res = locals[op.out.as_ref().unwrap()];
            // Multi-value return (Section 3.1): if the tuple was
            // produced by a promoted call_internal, the values
            // are already in dedicated locals.
            if ctx.multi_return.is_promoted_call_tuple(tuple_var) {
                let idx = op.value.unwrap_or(0);
                if let Some(src_local) = ctx.multi_return.promoted_call_value_local(tuple_var, idx)
                {
                    func.instruction(&Instruction::LocalGet(src_local));
                    func.instruction(&Instruction::LocalSet(res));
                } else {
                    let tuple = locals[tuple_var];
                    let val = locals[&args[1]];
                    func.instruction(&Instruction::LocalGet(tuple));
                    func.instruction(&Instruction::LocalGet(val));
                    emit_call(func, reloc_enabled, import_ids["tuple_index"]);
                    func.instruction(&Instruction::LocalSet(res));
                }
            } else {
                let tuple = locals[tuple_var];
                let val = locals[&args[1]];
                func.instruction(&Instruction::LocalGet(tuple));
                func.instruction(&Instruction::LocalGet(val));
                emit_call(func, reloc_enabled, import_ids["tuple_index"]);
                func.instruction(&Instruction::LocalSet(res));
            }
        }
        "unpack_sequence" => {
            // args[0] is the sequence, args[1..] are output variable names.
            // op.value holds the expected element count.
            // The sequence may be a list (from _emit_list_from_iter) or
            // a tuple, so use the general-purpose `index` import which
            // handles both via __getitem__.
            let args = op.args.as_ref().unwrap();
            let seq = locals[&args[0]];
            let expected_count = op.value.unwrap() as usize;
            for i in 0..expected_count {
                let out = locals[&args[1 + i]];
                func.instruction(&Instruction::LocalGet(seq));
                func.instruction(&Instruction::I64Const(box_int(i as i64)));
                emit_call(func, reloc_enabled, import_ids["index"]);
                func.instruction(&Instruction::LocalSet(out));
            }
        }
        _ => return false,
    }
    true
}
