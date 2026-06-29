use super::site::{
    build_positional_callargs, collect_live_object_locals_for_call, emit_call_site_id,
    emit_pending_exception_return, release_live_object_locals, retain_live_object_locals,
    spill_call_args,
};
use super::{CallOpContext, CallOpEmission};
use crate::OpIR;
use crate::wasm::WasmFrameSyntheticLocal;
use crate::wasm_binary::{emit_call, emit_table_index_i64};
use crate::wasm_values::box_bool;
use wasm_encoder::{BlockType, Function, Instruction};

pub(super) fn emit_dynamic_call_op(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let func_ir = call_ctx.func_ir;
    let call_site_abi = call_ctx.call_site_abi;
    let import_ids = call_ctx.import_ids;
    let runtime_lookup_only_vars = call_ctx.runtime_lookup_only_vars;
    let locals = call_ctx.locals;
    let const_cache = call_ctx.const_cache;
    let reloc_enabled = call_ctx.reloc_enabled;
    let last_use_local = call_ctx.last_use_local;
    let rel_idx = call_ctx.rel_idx;
    let op_idx = call_ctx.op_idx;
    match op.kind.as_str() {
        "call_guarded" => {
            let target_name = op.s_value.as_ref().unwrap();
            let args_names = op.args.as_ref().unwrap();
            let callee_bits = locals[&args_names[0]];
            let out = locals[op.out.as_ref().unwrap()];
            let callargs_tmp = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            let tmp_ptr = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp1);
            let arity = args_names.len().saturating_sub(1);
            let escaped_target = call_site_abi.is_escaped_callable(target_name);
            let func_idx = call_site_abi.function_index(target_name, "call_guarded");
            let table_idx = call_site_abi.table_index(target_name, "call_guarded");
            if escaped_target {
                func.instruction(&Instruction::LocalGet(callee_bits));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsFunctionObj],
                );
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
                );
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(BlockType::Empty));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::RecursionGuardEnter],
                );
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(BlockType::Empty));
                let code_id = op.value.unwrap_or(0);
                func.instruction(&Instruction::I64Const(code_id));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TraceEnterSlot],
                );
                func.instruction(&Instruction::Drop);
                let spill_base = call_site_abi.call_func_spill_offset();
                spill_call_args(func, locals, spill_base, &args_names[1..]);
                func.instruction(&Instruction::LocalGet(callee_bits));
                func.instruction(&Instruction::I64Const(spill_base as i64));
                func.instruction(&Instruction::I64Const(arity as i64));
                func.instruction(&Instruction::I64Const(code_id));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallFuncDispatch],
                );
                func.instruction(&Instruction::LocalSet(out));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TraceExit],
                );
                func.instruction(&Instruction::Drop);
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::RecursionGuardExit],
                );
                func.instruction(&Instruction::Else);
                // Recursion guard failed — exception is already pending.
                // Return immediately so the pending RecursionError
                // propagates to the caller instead of being silently
                // swallowed as None (which caused TypeError downstream).
                emit_pending_exception_return(func, const_cache);
                func.instruction(&Instruction::End);
                func.instruction(&Instruction::Else);
                build_positional_callargs(
                    func,
                    import_ids,
                    reloc_enabled,
                    locals,
                    callargs_tmp,
                    &args_names[1..],
                );
                emit_call_site_id(func, func_ir.name.as_str(), op_idx, "call_guarded_nonfunc");
                func.instruction(&Instruction::LocalGet(callee_bits));
                func.instruction(&Instruction::LocalGet(callargs_tmp));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallBindIc],
                );
                func.instruction(&Instruction::LocalSet(out));
                func.instruction(&Instruction::End);
                return CallOpEmission::Handled;
            }
            func.instruction(&Instruction::LocalGet(callee_bits));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsFunctionObj],
            );
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::IsTruthy],
            );
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(BlockType::Empty));

            // callee is a function object: resolve and compare against expected target
            func.instruction(&Instruction::LocalGet(callee_bits));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::HandleResolve],
            );
            func.instruction(&Instruction::I64ExtendI32U);
            func.instruction(&Instruction::LocalSet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            func.instruction(&Instruction::I32WrapI64);
            func.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
                align: 3,
                offset: 0,
                memory_index: 0,
            }));
            func.instruction(&Instruction::LocalSet(tmp_ptr));
            func.instruction(&Instruction::LocalGet(tmp_ptr));
            emit_table_index_i64(func, reloc_enabled, table_idx);
            func.instruction(&Instruction::I64Eq);
            func.instruction(&Instruction::If(BlockType::Empty));

            // fast path: callee matches expected target
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::RecursionGuardEnter],
            );
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(BlockType::Empty));
            let code_id = op.value.unwrap_or(0);
            func.instruction(&Instruction::I64Const(code_id));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TraceEnterSlot],
            );
            func.instruction(&Instruction::Drop);
            // For closure functions, extract the closure environment
            // from the callee object and push it as the leading arg.
            // The WASM signature of closure functions is
            //   (closure_env, arg1, arg2, …) → i64
            // so we must prepend the env before the user arguments.
            if call_site_abi.is_closure_function(target_name) {
                func.instruction(&Instruction::LocalGet(callee_bits));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::FunctionClosureBits],
                );
            }
            for arg_name in &args_names[1..] {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(arg));
            }
            emit_call(func, reloc_enabled, func_idx);
            func.instruction(&Instruction::LocalSet(out));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::TraceExit],
            );
            func.instruction(&Instruction::Drop);
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::RecursionGuardExit],
            );
            func.instruction(&Instruction::Else);
            // Recursion guard failed — exception is already pending.
            // Return immediately so the pending RecursionError
            // propagates to the caller instead of being silently
            // swallowed as None (which caused TypeError downstream).
            emit_pending_exception_return(func, const_cache);
            func.instruction(&Instruction::End);

            // slow path: function object does not match expected target
            func.instruction(&Instruction::Else);
            build_positional_callargs(
                func,
                import_ids,
                reloc_enabled,
                locals,
                callargs_tmp,
                &args_names[1..],
            );
            emit_call_site_id(
                func,
                func_ir.name.as_str(),
                op_idx,
                "call_guarded_slow_match_miss",
            );
            func.instruction(&Instruction::LocalGet(callee_bits));
            func.instruction(&Instruction::LocalGet(callargs_tmp));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallBindIc],
            );
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::End);

            // not a function object: fallback to call_bind
            func.instruction(&Instruction::Else);
            build_positional_callargs(
                func,
                import_ids,
                reloc_enabled,
                locals,
                callargs_tmp,
                &args_names[1..],
            );
            emit_call_site_id(func, func_ir.name.as_str(), op_idx, "call_guarded_nonfunc");
            func.instruction(&Instruction::LocalGet(callee_bits));
            func.instruction(&Instruction::LocalGet(callargs_tmp));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallBindIc],
            );
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::End);
        }
        "call_func" => {
            let args_names = op.args.as_ref().unwrap();
            let live_object_locals = collect_live_object_locals_for_call(
                locals,
                last_use_local,
                rel_idx,
                op.out.as_ref(),
            );
            retain_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
            if args_names.len() == 3 && runtime_lookup_only_vars.contains(&args_names[0]) {
                let name_bits = locals[&args_names[1]];
                let namespace_bits = locals[&args_names[2]];
                let out = locals[op.out.as_ref().unwrap()];
                func.instruction(&Instruction::LocalGet(name_bits));
                func.instruction(&Instruction::LocalGet(namespace_bits));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids
                        [crate::wasm_abi_generated::WasmRuntimeImport::RequireIntrinsicRuntime],
                );
                func.instruction(&Instruction::LocalSet(out));
                release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
                return CallOpEmission::Handled;
            }
            // Outlined: spill args to linear memory, then delegate
            // to molt_call_func_dispatch runtime helper.
            let func_bits = locals[&args_names[0]];
            let out = locals[op.out.as_ref().unwrap()];
            let nargs = args_names.len().saturating_sub(1);
            let spill_base = call_site_abi.call_func_spill_offset();

            spill_call_args(func, locals, spill_base, &args_names[1..]);

            // Push args: func_bits, args_ptr, nargs, code_id
            func.instruction(&Instruction::LocalGet(func_bits));
            func.instruction(&Instruction::I64Const(spill_base as i64));
            func.instruction(&Instruction::I64Const(nargs as i64));
            let code_id = op.value.unwrap_or(0);
            func.instruction(&Instruction::I64Const(code_id));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallFuncDispatch],
            );
            func.instruction(&Instruction::LocalSet(out));
            release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
        }
        "invoke_ffi" => {
            if let Some(export_name) = op.native_callable_export.as_deref() {
                let binding = op.native_callable_binding.as_deref().unwrap_or("<missing>");
                let abi = op.native_callable_abi.as_deref().unwrap_or("<missing>");
                let symbol = op
                    .native_callable_symbol
                    .as_deref()
                    .unwrap_or("<module-attr>");
                panic!(
                    "native callable export `{export_name}` reached wasm backend without executable native ABI dispatch table: binding={binding} abi={abi} symbol={symbol}"
                );
            }
            let args_names = op.args.as_ref().unwrap();
            let live_object_locals = collect_live_object_locals_for_call(
                locals,
                last_use_local,
                rel_idx,
                op.out.as_ref(),
            );
            retain_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
            let func_bits = locals[&args_names[0]];
            let out = locals[op.out.as_ref().unwrap()];
            let callargs_tmp = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
            build_positional_callargs(
                func,
                import_ids,
                reloc_enabled,
                locals,
                callargs_tmp,
                &args_names[1..],
            );
            let invoke_bridge_lane = op.s_value.as_deref() == Some("bridge");
            let call_site_label = if invoke_bridge_lane {
                "invoke_ffi_bridge"
            } else {
                "invoke_ffi_deopt"
            };
            emit_call_site_id(func, func_ir.name.as_str(), op_idx, call_site_label);
            func.instruction(&Instruction::LocalGet(func_bits));
            func.instruction(&Instruction::LocalGet(callargs_tmp));
            let require_bridge_cap = if invoke_bridge_lane { 1 } else { 0 };
            func.instruction(&Instruction::I64Const(box_bool(require_bridge_cap)));
            emit_call(
                func,
                reloc_enabled,
                import_ids[crate::wasm_abi_generated::WasmRuntimeImport::InvokeFfiIc],
            );
            func.instruction(&Instruction::LocalSet(out));
            release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
        }
        "call_bind" | "call_indirect" => {
            let args_names = op.args.as_ref().unwrap();
            let func_bits = locals[&args_names[0]];
            let builder_ptr = locals[&args_names[1]];
            let out = op.out.as_ref().and_then(|name| locals.get(name).copied());
            let live_object_locals = collect_live_object_locals_for_call(
                locals,
                last_use_local,
                rel_idx,
                op.out.as_ref(),
            );
            retain_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
            let call_site_label = if op.kind == "call_indirect" {
                "call_indirect"
            } else {
                "call_bind"
            };
            emit_call_site_id(func, func_ir.name.as_str(), op_idx, call_site_label);
            func.instruction(&Instruction::LocalGet(func_bits));
            func.instruction(&Instruction::LocalGet(builder_ptr));
            if op.kind == "call_indirect" {
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallIndirectIc],
                );
            } else {
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallBindIc],
                );
            }
            if let Some(out_local) = out {
                func.instruction(&Instruction::LocalSet(out_local));
            } else {
                func.instruction(&Instruction::Drop);
            }
            release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
        }
        "call_method" => {
            let args_names = op.args.as_ref().unwrap();
            let method_bits = locals[&args_names[0]];
            let out = locals[op.out.as_ref().unwrap()];
            let live_object_locals = collect_live_object_locals_for_call(
                locals,
                last_use_local,
                rel_idx,
                op.out.as_ref(),
            );
            retain_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);

            // Fast-path: dispatch known bound-method patterns
            // directly without callargs allocation or IC lookup.
            let fast_dispatched = if let Some(sv) = op.s_value.as_deref() {
                let arity = args_names.len().saturating_sub(1);
                match sv {
                    "BoundMethod:list:append" if arity == 1 => {
                        let arg = locals[&args_names[1]];
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids
                                [crate::wasm_abi_generated::WasmRuntimeImport::FastListAppend],
                        );
                        true
                    }
                    "BoundMethod:str:join" if arity == 1 => {
                        let arg = locals[&args_names[1]];
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::FastStrJoin],
                        );
                        true
                    }
                    "BoundMethod:dict:get" if arity == 2 => {
                        let key = locals[&args_names[1]];
                        let default = locals[&args_names[2]];
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::FastDictGet],
                        );
                        true
                    }
                    "BoundMethod:str:startswith" if arity == 1 => {
                        let arg = locals[&args_names[1]];
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids
                                [crate::wasm_abi_generated::WasmRuntimeImport::FastStrStartswith],
                        );
                        true
                    }
                    "BoundMethod:str:upper" if arity == 0 => {
                        func.instruction(&Instruction::LocalGet(method_bits));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::FastStrUpper],
                        );
                        true
                    }
                    "BoundMethod:str:lower" if arity == 0 => {
                        func.instruction(&Instruction::LocalGet(method_bits));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::FastStrLower],
                        );
                        true
                    }
                    "BoundMethod:str:strip" if arity == 0 => {
                        func.instruction(&Instruction::LocalGet(method_bits));
                        emit_call(
                            func,
                            reloc_enabled,
                            import_ids[crate::wasm_abi_generated::WasmRuntimeImport::FastStrStrip],
                        );
                        true
                    }
                    _ => false,
                }
            } else {
                false
            };

            if !fast_dispatched {
                // Generic path: allocate callargs and dispatch via IC.
                let callargs_tmp = locals.synthetic(WasmFrameSyntheticLocal::MoltTmp0);
                build_positional_callargs(
                    func,
                    import_ids,
                    reloc_enabled,
                    locals,
                    callargs_tmp,
                    &args_names[1..],
                );
                emit_call_site_id(func, func_ir.name.as_str(), op_idx, "call_method");
                func.instruction(&Instruction::LocalGet(method_bits));
                func.instruction(&Instruction::LocalGet(callargs_tmp));
                emit_call(
                    func,
                    reloc_enabled,
                    import_ids[crate::wasm_abi_generated::WasmRuntimeImport::CallBindIc],
                );
            }
            func.instruction(&Instruction::LocalSet(out));
            release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
        }
        _ => return CallOpEmission::NotHandled,
    }

    CallOpEmission::Handled
}
