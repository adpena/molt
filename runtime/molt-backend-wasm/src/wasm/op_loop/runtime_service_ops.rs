use super::*;

mod exception_ops;

pub(super) struct RuntimeServiceOpContext<'a> {
    pub(super) func_map: &'a BTreeMap<String, u32>,
    pub(super) table_base: u32,
    pub(super) import_ids: &'a TrackedImportIds,
    pub(super) locals: &'a BTreeMap<String, u32>,
    pub(super) const_cache: &'a ConstantCache,
    pub(super) reloc_enabled: bool,
    pub(super) native_eh_enabled: bool,
}

pub(super) fn emit_runtime_service_op(
    context: RuntimeServiceOpContext<'_>,
    func: &mut Function,
    op: &OpIR,
) -> bool {
    let func_map = context.func_map;
    let table_base = context.table_base;
    let import_ids = context.import_ids;
    let locals = context.locals;
    let reloc_enabled = context.reloc_enabled;

    match op.kind.as_str() {
        "chan_new" => {
            let args = op.args.as_ref().unwrap();
            let cap = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(cap));
            emit_call(func, reloc_enabled, import_ids["chan_new"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "chan_drop" => {
            let args = op.args.as_ref().unwrap();
            let chan = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(chan));
            emit_call(func, reloc_enabled, import_ids["chan_drop"]);
            func.instruction(&Instruction::Drop);
        }
        "module_new" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_new"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_cache_get" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_cache_get"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_import" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_import"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_cache_set" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            let module = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(module));
            emit_call(func, reloc_enabled, import_ids["module_cache_set"]);
            if let Some(out) = op.out.as_ref() {
                if out != "none" {
                    let res = locals[out];
                    func.instruction(&Instruction::LocalSet(res));
                } else {
                    func.instruction(&Instruction::Drop);
                }
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_cache_del" => {
            let args = op.args.as_ref().unwrap();
            let name = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_cache_del"]);
            if let Some(out) = op.out.as_ref() {
                if out != "none" {
                    let res = locals[out];
                    func.instruction(&Instruction::LocalSet(res));
                } else {
                    func.instruction(&Instruction::Drop);
                }
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_get_attr" | "module_import_from" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            // `from M import name` uses CPython IMPORT_FROM semantics
            // (ImportError on miss + sys.modules submodule fallback);
            // plain `M.name` raises AttributeError.
            let import_symbol = if op.kind == "module_import_from" {
                "module_import_from"
            } else {
                "module_get_attr"
            };
            emit_call(func, reloc_enabled, import_ids[import_symbol]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_get_global" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_get_global"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_del_global" | "module_del_global_if_present" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids[op.kind.as_str()]);
            if let Some(out) = op.out.as_ref() {
                if out != "none" {
                    let res = locals[out];
                    func.instruction(&Instruction::LocalSet(res));
                } else {
                    func.instruction(&Instruction::Drop);
                }
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_get_name" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            emit_call(func, reloc_enabled, import_ids["module_get_name"]);
            if let Some(out) = op.out.as_ref() {
                let res = locals[out];
                func.instruction(&Instruction::LocalSet(res));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_set_attr" => {
            let args = op.args.as_ref().unwrap();
            let module = locals[&args[0]];
            let name = locals[&args[1]];
            let val = locals[&args[2]];
            func.instruction(&Instruction::LocalGet(module));
            func.instruction(&Instruction::LocalGet(name));
            func.instruction(&Instruction::LocalGet(val));
            emit_call(func, reloc_enabled, import_ids["module_set_attr"]);
            if let Some(out) = op.out.as_ref() {
                if out != "none" {
                    func.instruction(&Instruction::LocalSet(locals[out]));
                } else {
                    func.instruction(&Instruction::Drop);
                }
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "module_import_star" => {
            let args = op.args.as_ref().unwrap();
            let src = locals[&args[0]];
            let dst = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(src));
            func.instruction(&Instruction::LocalGet(dst));
            emit_call(func, reloc_enabled, import_ids["module_import_star"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
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
        "context_null"
        | "context_enter"
        | "context_exit"
        | "context_unwind"
        | "context_depth"
        | "context_unwind_to"
        | "context_closing"
        | "exception_push"
        | "exception_pop"
        | "exception_stack_clear"
        | "exception_last"
        | "exception_last_pending"
        | "exception_finally_pending_observer"
        | "exception_active"
        | "exception_current"
        | "exception_enter_handler"
        | "exception_resolve_captured"
        | "exception_new"
        | "exception_new_builtin"
        | "exception_new_builtin_empty"
        | "exception_new_builtin_one"
        | "exception_new_from_class"
        | "exceptiongroup_match"
        | "exceptiongroup_combine"
        | "exception_clear"
        | "exception_kind"
        | "exception_class"
        | "exception_message"
        | "exception_set_cause"
        | "exception_set_value"
        | "exception_context_set"
        | "exception_set_last"
        | "raise" => {
            return exception_ops::emit_exception_runtime_op(&context, func, op);
        }
        "bridge_unavailable" => {
            let args = op.args.as_ref().unwrap();
            let msg = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(msg));
            emit_call(func, reloc_enabled, import_ids["bridge_unavailable"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "file_open" => {
            let args = op.args.as_ref().unwrap();
            let path = locals[&args[0]];
            let mode = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(path));
            func.instruction(&Instruction::LocalGet(mode));
            emit_call(func, reloc_enabled, import_ids["file_open"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "file_read" => {
            let args = op.args.as_ref().unwrap();
            let handle = locals[&args[0]];
            let size = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(handle));
            func.instruction(&Instruction::LocalGet(size));
            emit_call(func, reloc_enabled, import_ids["file_read"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "file_write" => {
            let args = op.args.as_ref().unwrap();
            let handle = locals[&args[0]];
            let data = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(handle));
            func.instruction(&Instruction::LocalGet(data));
            emit_call(func, reloc_enabled, import_ids["file_write"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "file_close" => {
            let args = op.args.as_ref().unwrap();
            let handle = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(handle));
            emit_call(func, reloc_enabled, import_ids["file_close"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "file_flush" => {
            let args = op.args.as_ref().unwrap();
            let handle = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(handle));
            emit_call(func, reloc_enabled, import_ids["file_flush"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "cancel_token_new" => {
            let args = op.args.as_ref().unwrap();
            let parent = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(parent));
            emit_call(func, reloc_enabled, import_ids["cancel_token_new"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "cancel_token_clone" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_clone"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "cancel_token_drop" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_drop"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "cancel_token_cancel" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_cancel"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "future_cancel" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(future));
            emit_call(func, reloc_enabled, import_ids["future_cancel"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "future_cancel_msg" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            let msg = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(future));
            func.instruction(&Instruction::LocalGet(msg));
            emit_call(func, reloc_enabled, import_ids["future_cancel_msg"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "future_cancel_clear" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(future));
            emit_call(func, reloc_enabled, import_ids["future_cancel_clear"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "promise_new" => {
            emit_call(func, reloc_enabled, import_ids["promise_new"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "promise_set_result" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            let result = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(future));
            func.instruction(&Instruction::LocalGet(result));
            emit_call(func, reloc_enabled, import_ids["promise_set_result"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "promise_set_exception" => {
            let args = op.args.as_ref().unwrap();
            let future = locals[&args[0]];
            let exc = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(future));
            func.instruction(&Instruction::LocalGet(exc));
            emit_call(func, reloc_enabled, import_ids["promise_set_exception"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
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
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "task_register_token_owned" => {
            let args = op.args.as_ref().unwrap();
            let task = locals[&args[0]];
            let token = locals[&args[1]];
            func.instruction(&Instruction::LocalGet(task));
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["task_register_token_owned"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
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
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "cancel_token_set_current" => {
            let args = op.args.as_ref().unwrap();
            let token = locals[&args[0]];
            func.instruction(&Instruction::LocalGet(token));
            emit_call(func, reloc_enabled, import_ids["cancel_token_set_current"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "cancel_token_get_current" => {
            emit_call(func, reloc_enabled, import_ids["cancel_token_get_current"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "cancelled" => {
            emit_call(func, reloc_enabled, import_ids["cancelled"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "cancel_current" => {
            emit_call(func, reloc_enabled, import_ids["cancel_current"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        "block_on" => {
            let args = op.args.as_ref().unwrap();
            func.instruction(&Instruction::LocalGet(locals[&args[0]]));
            emit_call(func, reloc_enabled, import_ids["block_on"]);
            if let Some(out) = op.out.as_ref() {
                func.instruction(&Instruction::LocalSet(locals[out]));
            } else {
                func.instruction(&Instruction::Drop);
            }
        }
        // ---------------------------------------------------------------
        // memory_copy: bulk linear-memory copy (WASM 2.0 bulk-memory op)
        //
        // IR signature:  memory_copy(dst, src, len)
        //   dst, src  – i64 boxed integers holding i32 linear-memory byte
        //               offsets (e.g. from handle_resolve)
        //   len       – i64 boxed integer holding the byte count
        //
        // Emits:  memory.copy  (dst_mem=0, src_mem=0)
        //         stack: [dst:i32, src:i32, len:i32]
        //
        // This intrinsic enables the IR to emit efficient buffer-to-buffer
        // copies without round-tripping through host imports.  See
        // WASM_OPTIMIZATION_PLAN.md Section 3.3.
        // ---------------------------------------------------------------
        "memory_copy" => {
            let args = op.args.as_ref().unwrap();
            debug_assert!(
                args.len() == 3,
                "memory_copy requires exactly 3 args (dst, src, len)"
            );
            let dst = locals[&args[0]];
            let src = locals[&args[1]];
            let len = locals[&args[2]];
            // Unbox each i64 value to i32 for the memory.copy instruction.
            func.instruction(&Instruction::LocalGet(dst));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(src));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            });
        }
        // ---------------------------------------------------------------
        // memory_fill: bulk linear-memory fill (WASM 2.0 bulk-memory op)
        //
        // IR signature:  memory_fill(dst, val, len)
        //   dst  – i64 boxed integer holding i32 linear-memory byte offset
        //   val  – i64 boxed integer holding the fill byte (0-255)
        //   len  – i64 boxed integer holding the byte count
        //
        // Emits:  memory.fill  (mem=0)
        //         stack: [dst:i32, val:i32, len:i32]
        //
        // Enables efficient zero-init and constant-fill of linear memory
        // regions without round-tripping through host imports or byte loops.
        // ---------------------------------------------------------------
        "memory_fill" => {
            let args = op.args.as_ref().unwrap();
            debug_assert!(
                args.len() == 3,
                "memory_fill requires exactly 3 args (dst, val, len)"
            );
            let dst = locals[&args[0]];
            let val = locals[&args[1]];
            let len = locals[&args[2]];
            // Unbox each i64 value to i32 for the memory.fill instruction.
            func.instruction(&Instruction::LocalGet(dst));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(val));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::LocalGet(len));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::MemoryFill(0));
        }
        _ => return false,
    }
    true
}
