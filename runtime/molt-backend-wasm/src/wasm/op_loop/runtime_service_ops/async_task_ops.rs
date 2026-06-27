use super::super::result_sink::store_result_or_drop;
use super::*;

pub(super) fn emit_async_task_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let func_map = context.func_map;
    let table_base = context.table_base;
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "alloc_task" => {
            let total = op.value.unwrap_or(0);
            let task_kind = op.task_kind.as_deref().unwrap_or("future");
            let (kind_bits, payload_base) = match task_kind {
                "generator" => (TASK_KIND_GENERATOR, GEN_CONTROL_SIZE),
                "future" => (TASK_KIND_FUTURE, 0),
                "coroutine" => (TASK_KIND_COROUTINE, 0),
                _ => panic!("unknown task kind: {task_kind}"),
            };
            let target_name = op.s_value.as_ref().expect("alloc_task target missing");
            let table_slot = *func_map
                .get(target_name)
                .unwrap_or_else(|| panic!("alloc_task table target not found: {target_name}"));
            let table_idx = table_base + table_slot;
            emit_table_index_i64(func, reloc_enabled, table_idx);
            func.instruction(&Instruction::I64Const(total));
            func.instruction(&Instruction::I64Const(kind_bits));
            emit_call(func, reloc_enabled, import_ids["task_new"]);
            let res = if let Some(out) = op.out.as_ref() {
                let r = locals[out];
                func.instruction(&Instruction::LocalSet(r));
                r
            } else {
                func.instruction(&Instruction::Drop);
                0
            };
            // Resolve the task handle pointer once when we need to
            // materialize closure/argument payload slots after the
            // runtime-owned control block.
            let has_args = op.args.as_ref().is_some_and(|a| !a.is_empty());
            if has_args {
                let resolve_local = locals["__wasm_alloc_resolve"];
                func.instruction(&Instruction::LocalGet(res));
                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                func.instruction(&Instruction::LocalSet(resolve_local));
            }
            if let Some(args) = op.args.as_ref()
                && !args.is_empty()
            {
                let resolve_local = locals["__wasm_alloc_resolve"];
                for (i, name) in args.iter().enumerate() {
                    let arg_local = locals[name];
                    func.instruction(&Instruction::LocalGet(resolve_local));
                    func.instruction(&Instruction::I32Const(payload_base + (i as i32) * 8));
                    func.instruction(&Instruction::I32Add);
                    func.instruction(&Instruction::LocalGet(arg_local));
                    func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                        align: 3,
                        offset: 0,
                        memory_index: 0,
                    }));
                    func.instruction(&Instruction::LocalGet(arg_local));
                    emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
                }
            }
            if matches!(task_kind, "future" | "coroutine") {
                func.instruction(&Instruction::LocalGet(res));
                emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
                emit_call(func, reloc_enabled, import_ids["task_register_token_owned"]);
                func.instruction(&Instruction::Drop);
            }
        }
        "state_yield" => {
            let args = op.args.as_ref().unwrap();
            func.instruction(&Instruction::LocalGet(0));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Const(op.value.unwrap()));
            emit_call(func, reloc_enabled, import_ids["obj_set_state"]);
            let pair = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(pair));
            emit_call(func, reloc_enabled, import_ids["inc_ref_obj"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalGet(pair));
                func.instruction(&Instruction::LocalSet(locals[out]));
                func.instruction(&Instruction::LocalGet(locals[out]));
            } else {
                func.instruction(&Instruction::LocalGet(pair));
            }
            func.instruction(&Instruction::Return);
        }
        "cancel_token_new" => {
            let args = op.args.as_ref().unwrap();
            let parent = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(parent));
            emit_call(func, reloc_enabled, import_ids["cancel_token_new"]);
            store_result_or_drop(func, op, locals);
        }
        "cancel_token_clone" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_clone"]);
            store_result_or_drop(func, op, locals);
        }
        "cancel_token_drop" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_drop"]);
            store_result_or_drop(func, op, locals);
        }
        "cancel_token_cancel" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_cancel"]);
            store_result_or_drop(func, op, locals);
        }
        "future_cancel" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(future));
            emit_call(func, reloc_enabled, import_ids["future_cancel"]);
            store_result_or_drop(func, op, locals);
        }
        "future_cancel_msg" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            let msg = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(future));
            func.instruction(&Instruction::LocalGet(msg));
            emit_call(func, reloc_enabled, import_ids["future_cancel_msg"]);
            store_result_or_drop(func, op, locals);
        }
        "future_cancel_clear" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(future));
            emit_call(func, reloc_enabled, import_ids["future_cancel_clear"]);
            store_result_or_drop(func, op, locals);
        }
        "promise_new" => {
            emit_call(func, reloc_enabled, import_ids["promise_new"]);
            store_result_or_drop(func, op, locals);
        }
        "promise_set_result" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            let result = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(future));
            func.instruction(&Instruction::LocalGet(result));
            emit_call(func, reloc_enabled, import_ids["promise_set_result"]);
            store_result_or_drop(func, op, locals);
        }
        "promise_set_exception" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            let exc = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(future));
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["promise_set_exception"]);
            store_result_or_drop(func, op, locals);
        }
        "thread_submit" => {
            let args = op.args.as_ref().unwrap();
            let callable = locals[&args[0]];
            let call_args = locals[&args[1]];
            let call_kwargs = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(callable));
            func.instruction(&Instruction::LocalGet(call_args));
            func.instruction(&Instruction::LocalGet(call_kwargs));
            emit_call(func, reloc_enabled, import_ids["thread_submit"]);
            store_result_or_drop(func, op, locals);
        }
        "task_register_token_owned" => {
            let args = op.args.as_ref().unwrap();
            let task = locals[&args[0]];
            let token = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(task));
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["task_register_token_owned"]);
            store_result_or_drop(func, op, locals);
        }
        "spawn" => {
            let args = op.args.as_ref().unwrap();
            func.instruction(&Instruction::LocalGet(locals[&args[0]]));
            emit_call(func, reloc_enabled, import_ids["spawn"]);
        }
        "cancel_token_is_cancelled" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_is_cancelled"]);
            store_result_or_drop(func, op, locals);
        }
        "cancel_token_set_current" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_set_current"]);
            store_result_or_drop(func, op, locals);
        }
        "cancel_token_get_current" => {
            emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
            store_result_or_drop(func, op, locals);
        }
        "cancelled" => {
            emit_call(func, reloc_enabled, import_ids["cancelled"]);
            store_result_or_drop(func, op, locals);
        }
        "cancel_current" => {
            emit_call(func, reloc_enabled, import_ids["cancel_current"]);
            store_result_or_drop(func, op, locals);
        }
        "block_on" => {
            let args = op.args.as_ref().unwrap();
            func.instruction(&Instruction::LocalGet(locals[&args[0]]));
            emit_call(func, reloc_enabled, import_ids["block_on"]);
            store_result_or_drop(func, op, locals);
        }
        _ => return false,
    }
    true
}
