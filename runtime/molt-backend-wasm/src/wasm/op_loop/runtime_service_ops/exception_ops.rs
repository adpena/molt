use super::super::result_sink::store_result_or_drop;
use super::RuntimeServiceOpContext;
use super::call_emit::{
    RuntimeServiceArg::{Local, OpValueI64},
    RuntimeServiceCall, emit_runtime_service_call,
};
use crate::OpIR;
use crate::wasm_abi::TAG_EXCEPTION_INDEX;
use crate::wasm_binary::emit_call;
use wasm_encoder::{Function, Instruction};

pub(super) fn emit_exception_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    if let Some(call) = exception_plain_runtime_call(op.kind.as_str()) {
        emit_runtime_service_call(context, func, op, call);
        return true;
    }

    let import_ids = context.import_ids;
    let locals = context.locals;
    let const_cache = context.const_cache;
    let reloc_enabled = context.reloc_enabled;
    let native_eh_enabled = context.native_eh_enabled;

    match op.kind.as_str() {
        "exception_push" => {
            if native_eh_enabled {
                // Native EH: no-op; WASM runtime manages handler stack.
                const_cache.emit_none(func);
            } else {
                emit_call(func, reloc_enabled, import_ids["exception_push"]);
            }
            store_result_or_drop(func, op, locals);
        }
        "exception_pop" => {
            if native_eh_enabled {
                const_cache.emit_none(func);
            } else {
                emit_call(func, reloc_enabled, import_ids["exception_pop"]);
            }
            store_result_or_drop(func, op, locals);
        }
        "raise" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(exc));
            if native_eh_enabled {
                // Native EH: call host raise to register the exception
                // (traceback, __context__), then throw via WASM EH.
                emit_call(func, reloc_enabled, import_ids["raise"]);
                func.instruction(&Instruction::Drop);
                func.instruction(&Instruction::LocalGet(exc));
                func.instruction(&Instruction::Throw(TAG_EXCEPTION_INDEX));
            } else {
                emit_call(func, reloc_enabled, import_ids["raise"]);
                store_result_or_drop(func, op, locals);
            }
        }
        _ => return false,
    }
    true
}

fn exception_plain_runtime_call(kind: &str) -> Option<RuntimeServiceCall<'static>> {
    Some(match kind {
        "context_null" => RuntimeServiceCall::result("context_null", &[Local(0)]),
        "context_enter" => RuntimeServiceCall::result("context_enter", &[Local(0)]),
        "context_exit" => RuntimeServiceCall::result("context_exit", &[Local(0), Local(1)]),
        "context_unwind" => RuntimeServiceCall::result("context_unwind", &[Local(0)]),
        "context_depth" => RuntimeServiceCall::result("context_depth", &[]),
        "context_unwind_to" => {
            RuntimeServiceCall::result("context_unwind_to", &[Local(0), Local(1)])
        }
        "context_closing" => RuntimeServiceCall::result("context_closing", &[Local(0)]),
        "exception_stack_clear" => RuntimeServiceCall::result("exception_stack_clear", &[]),
        // Runtime exception-handler-stack depth bookkeeping mirrors the native
        // Cranelift lowering: direct runtime imports with i64 result sinking.
        "exception_stack_enter" => RuntimeServiceCall::result("exception_stack_enter", &[]),
        "exception_stack_depth" => RuntimeServiceCall::result("exception_stack_depth", &[]),
        "exception_stack_exit" => RuntimeServiceCall::result("exception_stack_exit", &[Local(0)]),
        "exception_stack_set_depth" => {
            RuntimeServiceCall::result("exception_stack_set_depth", &[Local(0)])
        }
        "exception_last" => RuntimeServiceCall::result("exception_last", &[]),
        "exception_last_pending" | "exception_finally_pending_observer" => {
            RuntimeServiceCall::result("exception_last_pending", &[])
        }
        "exception_active" => RuntimeServiceCall::result("exception_active", &[]),
        "exception_current" => RuntimeServiceCall::result("exception_current", &[]),
        "exception_enter_handler" => {
            RuntimeServiceCall::result("exception_enter_handler", &[Local(0)])
        }
        "exception_resolve_captured" => {
            RuntimeServiceCall::result("exception_resolve_captured", &[Local(0)])
        }
        "exception_new" => RuntimeServiceCall::result("exception_new", &[Local(0), Local(1)]),
        "exception_new_builtin" => RuntimeServiceCall::result(
            "exception_new_builtin",
            &[
                OpValueI64("exception_new_builtin missing tag value"),
                Local(0),
            ],
        ),
        "exception_new_builtin_empty" => RuntimeServiceCall::result(
            "exception_new_builtin_empty",
            &[OpValueI64("exception_new_builtin_empty missing tag value")],
        ),
        "exception_new_builtin_one" => RuntimeServiceCall::result(
            "exception_new_builtin_one",
            &[
                OpValueI64("exception_new_builtin_one missing tag value"),
                Local(0),
            ],
        ),
        "exception_new_from_class" => {
            RuntimeServiceCall::result("exception_new_from_class", &[Local(0), Local(1)])
        }
        "exceptiongroup_match" => {
            RuntimeServiceCall::result("exceptiongroup_match", &[Local(0), Local(1)])
        }
        "exceptiongroup_combine" => {
            RuntimeServiceCall::result("exceptiongroup_combine", &[Local(0)])
        }
        "exception_clear" => RuntimeServiceCall::result("exception_clear", &[]),
        "exception_kind" => RuntimeServiceCall::result("exception_kind", &[Local(0)]),
        "exception_class" => RuntimeServiceCall::result("exception_class", &[Local(0)]),
        "exception_message" => RuntimeServiceCall::result("exception_message", &[Local(0)]),
        "exception_set_cause" => {
            RuntimeServiceCall::result("exception_set_cause", &[Local(0), Local(1)])
        }
        "exception_set_value" => {
            RuntimeServiceCall::result("exception_set_value", &[Local(0), Local(1)])
        }
        "exception_context_set" => RuntimeServiceCall::result("exception_context_set", &[Local(0)]),
        "exception_set_last" => RuntimeServiceCall::result("exception_set_last", &[Local(0)]),
        _ => return None,
    })
}
