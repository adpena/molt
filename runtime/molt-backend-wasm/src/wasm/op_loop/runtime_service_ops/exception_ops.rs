use super::*;

pub(super) fn emit_exception_runtime_op(
    context: &RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let import_ids = context.import_ids;
    let locals = context.locals;
    let const_cache = context.const_cache;
    let reloc_enabled = context.reloc_enabled;
    let native_eh_enabled = context.native_eh_enabled;

    match op.kind.as_str() {
        "context_null" => {
            let args = op.args.as_ref().unwrap();
            let payload = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(payload));
            emit_call(func, reloc_enabled, import_ids["context_null"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "context_enter" => {
            let args = op.args.as_ref().unwrap();
            let ctx = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(ctx));
            emit_call(func, reloc_enabled, import_ids["context_enter"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "context_exit" => {
            let args = op.args.as_ref().unwrap();
            let ctx = locals[&args[0]];
            let exc = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(ctx));
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["context_exit"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "context_unwind" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["context_unwind"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "context_depth" => {
            emit_call(func, reloc_enabled, import_ids["context_depth"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "context_unwind_to" => {
            let args = op.args.as_ref().unwrap();
            let depth = locals[&args[0]];
            let exc = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(depth));
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["context_unwind_to"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "context_closing" => {
            let args = op.args.as_ref().unwrap();
            let payload = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(payload));
            emit_call(func, reloc_enabled, import_ids["context_closing"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_push" => {
            if native_eh_enabled {
                // Native EH: no-op; WASM runtime manages handler stack.
                const_cache.emit_none(func);
            } else {
                emit_call(func, reloc_enabled, import_ids["exception_push"]);
            }
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_pop" => {
            if native_eh_enabled {
                const_cache.emit_none(func);
            } else {
                emit_call(func, reloc_enabled, import_ids["exception_pop"]);
            }
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_stack_clear" => {
            emit_call(func, reloc_enabled, import_ids["exception_stack_clear"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_last" => {
            emit_call(func, reloc_enabled, import_ids["exception_last"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_last_pending" | "exception_finally_pending_observer" => {
            emit_call(func, reloc_enabled, import_ids["exception_last_pending"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_active" => {
            emit_call(func, reloc_enabled, import_ids["exception_active"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_current" => {
            emit_call(func, reloc_enabled, import_ids["exception_current"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_enter_handler" => {
            let args = op.args.as_ref().unwrap();
            let captured = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(captured));
            emit_call(func, reloc_enabled, import_ids["exception_enter_handler"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_resolve_captured" => {
            let args = op.args.as_ref().unwrap();
            let captured = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(captured));
            emit_call(
                func,
                reloc_enabled,
                import_ids["exception_resolve_captured"],
            );
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_new" => {
            let args = op.args.as_ref().unwrap();
            let kind = locals[&args[0]];
            let args_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(kind));
            func.instruction(&Instruction::LocalGet(args_bits));
            emit_call(func, reloc_enabled, import_ids["exception_new"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_new_builtin" => {
            let args = op.args.as_ref().unwrap();
            let tag = op.value.expect("exception_new_builtin missing tag value");
            let args_bits = locals[&args[0]];
            func.instruction(&Instruction::I64Const(tag));
            func.instruction(&Instruction::LocalGet(args_bits));
            emit_call(func, reloc_enabled, import_ids["exception_new_builtin"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_new_builtin_empty" => {
            let tag = op
                .value
                .expect("exception_new_builtin_empty missing tag value");
            func.instruction(&Instruction::I64Const(tag));
            emit_call(
                func,
                reloc_enabled,
                import_ids["exception_new_builtin_empty"],
            );
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_new_builtin_one" => {
            let args = op.args.as_ref().unwrap();
            let tag = op
                .value
                .expect("exception_new_builtin_one missing tag value");
            let arg_bits = locals[&args[0]];
            func.instruction(&Instruction::I64Const(tag));
            func.instruction(&Instruction::LocalGet(arg_bits));
            emit_call(func, reloc_enabled, import_ids["exception_new_builtin_one"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_new_from_class" => {
            let args = op.args.as_ref().unwrap();
            let class_bits = locals[&args[0]];
            let args_bits = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(class_bits));
            func.instruction(&Instruction::LocalGet(args_bits));
            emit_call(func, reloc_enabled, import_ids["exception_new_from_class"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exceptiongroup_match" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            let matcher = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(exc));
            func.instruction(&Instruction::LocalGet(matcher));
            emit_call(func, reloc_enabled, import_ids["exceptiongroup_match"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exceptiongroup_combine" => {
            let args = op.args.as_ref().unwrap();
            let items = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(items));
            emit_call(func, reloc_enabled, import_ids["exceptiongroup_combine"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_clear" => {
            emit_call(func, reloc_enabled, import_ids["exception_clear"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_kind" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["exception_kind"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_class" => {
            let args = op.args.as_ref().unwrap();
            let kind = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(kind));
            emit_call(func, reloc_enabled, import_ids["exception_class"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_message" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["exception_message"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_set_cause" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            let cause = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(exc));
            func.instruction(&Instruction::LocalGet(cause));
            emit_call(func, reloc_enabled, import_ids["exception_set_cause"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_set_value" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            let value = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(exc));
            func.instruction(&Instruction::LocalGet(value));
            emit_call(func, reloc_enabled, import_ids["exception_set_value"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_context_set" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["exception_context_set"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "exception_set_last" => {
            let args = op.args.as_ref().unwrap();
            let exc = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["exception_set_last"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
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
                if let Some(ref out) = op.out {
                    func.instruction(&Instruction::LocalSet(locals[out]));
                } else {
                    // raise with no output — drop the result from the stack
                    func.instruction(&Instruction::Drop);
                }
            }
        }
        _ => return false,
    }
    true
}
