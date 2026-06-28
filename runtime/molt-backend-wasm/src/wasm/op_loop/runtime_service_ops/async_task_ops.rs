use super::RuntimeServiceOpContext;
use super::call_emit::{RuntimeServiceArg::Local, RuntimeServiceCall, emit_runtime_service_call};
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm_abi::{
    GEN_CONTROL_SIZE, TASK_KIND_COROUTINE, TASK_KIND_FUTURE, TASK_KIND_GENERATOR,
};
use crate::wasm_binary::{emit_call, emit_table_index_i64};
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_async_task_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if let Some(call) = async_task_runtime_call(op.kind.as_str()) {
        emit_runtime_service_call(context, func, op, call);
        return true;
    }

    let call_site_abi = context.call_site_abi;
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
            let table_idx = call_site_abi.table_index(target_name, "alloc_task");
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
                let resolve_local = locals.synthetic(WasmFrameSyntheticLocal::WasmAllocResolve);
                func.instruction(&Instruction::LocalGet(res));
                emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
                func.instruction(&Instruction::LocalSet(resolve_local));
            }
            if let Some(args) = op.args.as_ref()
                && !args.is_empty()
            {
                let resolve_local = locals.synthetic(WasmFrameSyntheticLocal::WasmAllocResolve);
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
        _ => return false,
    }
    true
}

fn async_task_runtime_call(kind: &str) -> Option<RuntimeServiceCall<'static>> {
    Some(match kind {
        "cancel_token_new" => RuntimeServiceCall::result("cancel_token_new", &[Local(0)]),
        "cancel_token_clone" => RuntimeServiceCall::result("cancel_token_clone", &[Local(0)]),
        "cancel_token_drop" => RuntimeServiceCall::result("cancel_token_drop", &[Local(0)]),
        "cancel_token_cancel" => RuntimeServiceCall::result("cancel_token_cancel", &[Local(0)]),
        "future_cancel" => RuntimeServiceCall::result("future_cancel", &[Local(0)]),
        "future_cancel_msg" => {
            RuntimeServiceCall::result("future_cancel_msg", &[Local(0), Local(1)])
        }
        "future_cancel_clear" => RuntimeServiceCall::result("future_cancel_clear", &[Local(0)]),
        "promise_new" => RuntimeServiceCall::result("promise_new", &[]),
        "promise_set_result" => {
            RuntimeServiceCall::result("promise_set_result", &[Local(0), Local(1)])
        }
        "promise_set_exception" => {
            RuntimeServiceCall::result("promise_set_exception", &[Local(0), Local(1)])
        }
        "thread_submit" => {
            RuntimeServiceCall::result("thread_submit", &[Local(0), Local(1), Local(2)])
        }
        "task_register_token_owned" => {
            RuntimeServiceCall::result("task_register_token_owned", &[Local(0), Local(1)])
        }
        "spawn" => RuntimeServiceCall::no_result("spawn", &[Local(0)]),
        "cancel_token_is_cancelled" => {
            RuntimeServiceCall::result("cancel_token_is_cancelled", &[Local(0)])
        }
        "cancel_token_set_current" => {
            RuntimeServiceCall::result("cancel_token_set_current", &[Local(0)])
        }
        "cancel_token_get_current" => RuntimeServiceCall::result("cancel_token_get_current", &[]),
        "cancelled" => RuntimeServiceCall::result("cancelled", &[]),
        "cancel_current" => RuntimeServiceCall::result("cancel_current", &[]),
        "block_on" => RuntimeServiceCall::result("block_on", &[Local(0)]),
        _ => return None,
    })
}
