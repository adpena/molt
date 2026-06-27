use super::frame::{
    collect_live_object_locals_for_call, release_live_object_locals, retain_live_object_locals,
};
use super::*;

pub(super) fn emit_dynamic_call_op(
    call_ctx: &mut CallOpContext<'_, '_, '_>,
    func: &mut Function,
    op: &OpIR,
) -> CallOpEmission {
    let func_ir = call_ctx.func_ir;
    let ctx = call_ctx.ctx;
    let func_map = call_ctx.func_map;
    let func_indices = call_ctx.func_indices;
    let table_base = call_ctx.table_base;
    let import_ids = call_ctx.import_ids;
    let closure_functions = call_ctx.closure_functions;
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
            let escaped_target = ctx.escaped_callable_targets.contains(target_name);
            let func_idx = *func_indices
                .get(target_name)
                .expect("call_guarded target not found");
            let table_slot = func_map[target_name];
            let table_idx = table_base + table_slot;
            if escaped_target {
                func.instruction(&Instruction::LocalGet(callee_bits));
                emit_call(func, reloc_enabled, import_ids["is_function_obj"]);
                emit_call(func, reloc_enabled, import_ids["is_truthy"]);
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(BlockType::Empty));
                emit_call(func, reloc_enabled, import_ids["recursion_guard_enter"]);
                func.instruction(&Instruction::I64Const(0));
                func.instruction(&Instruction::I64Ne);
                func.instruction(&Instruction::If(BlockType::Empty));
                let code_id = op.value.unwrap_or(0);
                func.instruction(&Instruction::I64Const(code_id));
                emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
                func.instruction(&Instruction::Drop);
                let spill_base = ctx.call_func_spill_offset;
                for (i, arg_name) in args_names[1..].iter().enumerate() {
                    let arg = locals[arg_name];
                    func.instruction(&Instruction::I32Const((spill_base + (i as u32) * 8) as i32));
                    func.instruction(&Instruction::LocalGet(arg));
                    func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                        align: 3,
                        offset: 0,
                        memory_index: 0,
                    }));
                }
                func.instruction(&Instruction::LocalGet(callee_bits));
                func.instruction(&Instruction::I64Const(spill_base as i64));
                func.instruction(&Instruction::I64Const(arity as i64));
                func.instruction(&Instruction::I64Const(code_id));
                emit_call(func, reloc_enabled, import_ids["call_func_dispatch"]);
                func.instruction(&Instruction::LocalSet(out));
                emit_call(func, reloc_enabled, import_ids["trace_exit"]);
                func.instruction(&Instruction::Drop);
                emit_call(func, reloc_enabled, import_ids["recursion_guard_exit"]);
                func.instruction(&Instruction::Else);
                // Recursion guard failed — exception is already pending.
                // Return immediately so the pending RecursionError
                // propagates to the caller instead of being silently
                // swallowed as None (which caused TypeError downstream).
                const_cache.emit_none(func);
                func.instruction(&Instruction::Return);
                func.instruction(&Instruction::End);
                func.instruction(&Instruction::Else);
                func.instruction(&Instruction::I64Const(arity as i64));
                func.instruction(&Instruction::I64Const(0));
                emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                func.instruction(&Instruction::LocalSet(callargs_tmp));
                for arg_name in &args_names[1..] {
                    let arg = locals[arg_name];
                    func.instruction(&Instruction::LocalGet(callargs_tmp));
                    func.instruction(&Instruction::LocalGet(arg));
                    emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                    func.instruction(&Instruction::Drop);
                }
                let site_bits = box_int(stable_ic_site_id(
                    func_ir.name.as_str(),
                    op_idx,
                    "call_guarded_nonfunc",
                ));
                func.instruction(&Instruction::I64Const(site_bits));
                func.instruction(&Instruction::LocalGet(callee_bits));
                func.instruction(&Instruction::LocalGet(callargs_tmp));
                emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
                func.instruction(&Instruction::LocalSet(out));
                func.instruction(&Instruction::End);
                return CallOpEmission::Handled;
            }
            func.instruction(&Instruction::LocalGet(callee_bits));
            emit_call(func, reloc_enabled, import_ids["is_function_obj"]);
            emit_call(func, reloc_enabled, import_ids["is_truthy"]);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(BlockType::Empty));

            // callee is a function object: resolve and compare against expected target
            func.instruction(&Instruction::LocalGet(callee_bits));
            emit_call(func, reloc_enabled, import_ids["handle_resolve"]);
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
            emit_call(func, reloc_enabled, import_ids["recursion_guard_enter"]);
            func.instruction(&Instruction::I64Const(0));
            func.instruction(&Instruction::I64Ne);
            func.instruction(&Instruction::If(BlockType::Empty));
            let code_id = op.value.unwrap_or(0);
            func.instruction(&Instruction::I64Const(code_id));
            emit_call(func, reloc_enabled, import_ids["trace_enter_slot"]);
            func.instruction(&Instruction::Drop);
            // For closure functions, extract the closure environment
            // from the callee object and push it as the leading arg.
            // The WASM signature of closure functions is
            //   (closure_env, arg1, arg2, …) → i64
            // so we must prepend the env before the user arguments.
            if closure_functions.contains(target_name) {
                func.instruction(&Instruction::LocalGet(callee_bits));
                emit_call(func, reloc_enabled, import_ids["function_closure_bits"]);
            }
            for arg_name in &args_names[1..] {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(arg));
            }
            emit_call(func, reloc_enabled, func_idx);
            func.instruction(&Instruction::LocalSet(out));
            emit_call(func, reloc_enabled, import_ids["trace_exit"]);
            func.instruction(&Instruction::Drop);
            emit_call(func, reloc_enabled, import_ids["recursion_guard_exit"]);
            func.instruction(&Instruction::Else);
            // Recursion guard failed — exception is already pending.
            // Return immediately so the pending RecursionError
            // propagates to the caller instead of being silently
            // swallowed as None (which caused TypeError downstream).
            const_cache.emit_none(func);
            func.instruction(&Instruction::Return);
            func.instruction(&Instruction::End);

            // slow path: function object does not match expected target
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::I64Const(arity as i64));
            func.instruction(&Instruction::I64Const(0));
            emit_call(func, reloc_enabled, import_ids["callargs_new"]);
            func.instruction(&Instruction::LocalSet(callargs_tmp));
            for arg_name in &args_names[1..] {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(callargs_tmp));
                func.instruction(&Instruction::LocalGet(arg));
                emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                func.instruction(&Instruction::Drop);
            }
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                "call_guarded_slow_match_miss",
            ));
            func.instruction(&Instruction::I64Const(site_bits));
            func.instruction(&Instruction::LocalGet(callee_bits));
            func.instruction(&Instruction::LocalGet(callargs_tmp));
            emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
            func.instruction(&Instruction::LocalSet(out));
            func.instruction(&Instruction::End);

            // not a function object: fallback to call_bind
            func.instruction(&Instruction::Else);
            func.instruction(&Instruction::I64Const(arity as i64));
            func.instruction(&Instruction::I64Const(0));
            emit_call(func, reloc_enabled, import_ids["callargs_new"]);
            func.instruction(&Instruction::LocalSet(callargs_tmp));
            for arg_name in &args_names[1..] {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(callargs_tmp));
                func.instruction(&Instruction::LocalGet(arg));
                emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                func.instruction(&Instruction::Drop);
            }
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                "call_guarded_nonfunc",
            ));
            func.instruction(&Instruction::I64Const(site_bits));
            func.instruction(&Instruction::LocalGet(callee_bits));
            func.instruction(&Instruction::LocalGet(callargs_tmp));
            emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
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
                emit_call(func, reloc_enabled, import_ids["require_intrinsic_runtime"]);
                func.instruction(&Instruction::LocalSet(out));
                release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
                return CallOpEmission::Handled;
            }
            // Outlined: spill args to linear memory, then delegate
            // to molt_call_func_dispatch runtime helper.
            let func_bits = locals[&args_names[0]];
            let out = locals[op.out.as_ref().unwrap()];
            let nargs = args_names.len().saturating_sub(1);
            let spill_base = ctx.call_func_spill_offset;

            // Spill each arg to consecutive i64 slots in linear memory.
            for (i, arg_name) in args_names[1..].iter().enumerate() {
                let arg = locals[arg_name];
                // addr (i32) = spill_base + i * 8
                func.instruction(&Instruction::I32Const((spill_base + (i as u32) * 8) as i32));
                func.instruction(&Instruction::LocalGet(arg));
                func.instruction(&Instruction::I64Store(wasm_encoder::MemArg {
                    align: 3,
                    offset: 0,
                    memory_index: 0,
                }));
            }

            // Push args: func_bits, args_ptr, nargs, code_id
            func.instruction(&Instruction::LocalGet(func_bits));
            func.instruction(&Instruction::I64Const(spill_base as i64));
            func.instruction(&Instruction::I64Const(nargs as i64));
            let code_id = op.value.unwrap_or(0);
            func.instruction(&Instruction::I64Const(code_id));
            emit_call(func, reloc_enabled, import_ids["call_func_dispatch"]);
            func.instruction(&Instruction::LocalSet(out));
            release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
        }
        "invoke_ffi" => {
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
            let arity = args_names.len().saturating_sub(1);
            func.instruction(&Instruction::I64Const(arity as i64));
            func.instruction(&Instruction::I64Const(0));
            emit_call(func, reloc_enabled, import_ids["callargs_new"]);
            func.instruction(&Instruction::LocalSet(callargs_tmp));
            for arg_name in &args_names[1..] {
                let arg = locals[arg_name];
                func.instruction(&Instruction::LocalGet(callargs_tmp));
                func.instruction(&Instruction::LocalGet(arg));
                emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                func.instruction(&Instruction::Drop);
            }
            let invoke_bridge_lane = op.s_value.as_deref() == Some("bridge");
            let call_site_label = if invoke_bridge_lane {
                "invoke_ffi_bridge"
            } else {
                "invoke_ffi_deopt"
            };
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                call_site_label,
            ));
            func.instruction(&Instruction::I64Const(site_bits));
            func.instruction(&Instruction::LocalGet(func_bits));
            func.instruction(&Instruction::LocalGet(callargs_tmp));
            let require_bridge_cap = if invoke_bridge_lane { 1 } else { 0 };
            func.instruction(&Instruction::I64Const(box_bool(require_bridge_cap)));
            emit_call(func, reloc_enabled, import_ids["invoke_ffi_ic"]);
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
            let site_bits = box_int(stable_ic_site_id(
                func_ir.name.as_str(),
                op_idx,
                call_site_label,
            ));
            func.instruction(&Instruction::I64Const(site_bits));
            func.instruction(&Instruction::LocalGet(func_bits));
            func.instruction(&Instruction::LocalGet(builder_ptr));
            if op.kind == "call_indirect" {
                emit_call(func, reloc_enabled, import_ids["call_indirect_ic"]);
            } else {
                emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
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
                        emit_call(func, reloc_enabled, import_ids["fast_list_append"]);
                        true
                    }
                    "BoundMethod:str:join" if arity == 1 => {
                        let arg = locals[&args_names[1]];
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["fast_str_join"]);
                        true
                    }
                    "BoundMethod:dict:get" if arity == 2 => {
                        let key = locals[&args_names[1]];
                        let default = locals[&args_names[2]];
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(key));
                        func.instruction(&Instruction::LocalGet(default));
                        emit_call(func, reloc_enabled, import_ids["fast_dict_get"]);
                        true
                    }
                    "BoundMethod:str:startswith" if arity == 1 => {
                        let arg = locals[&args_names[1]];
                        func.instruction(&Instruction::LocalGet(method_bits));
                        func.instruction(&Instruction::LocalGet(arg));
                        emit_call(func, reloc_enabled, import_ids["fast_str_startswith"]);
                        true
                    }
                    "BoundMethod:str:upper" if arity == 0 => {
                        func.instruction(&Instruction::LocalGet(method_bits));
                        emit_call(func, reloc_enabled, import_ids["fast_str_upper"]);
                        true
                    }
                    "BoundMethod:str:lower" if arity == 0 => {
                        func.instruction(&Instruction::LocalGet(method_bits));
                        emit_call(func, reloc_enabled, import_ids["fast_str_lower"]);
                        true
                    }
                    "BoundMethod:str:strip" if arity == 0 => {
                        func.instruction(&Instruction::LocalGet(method_bits));
                        emit_call(func, reloc_enabled, import_ids["fast_str_strip"]);
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
                let arity = args_names.len().saturating_sub(1);
                func.instruction(&Instruction::I64Const(arity as i64));
                func.instruction(&Instruction::I64Const(0));
                emit_call(func, reloc_enabled, import_ids["callargs_new"]);
                func.instruction(&Instruction::LocalSet(callargs_tmp));
                for arg_name in &args_names[1..] {
                    let arg = locals[arg_name];
                    func.instruction(&Instruction::LocalGet(callargs_tmp));
                    func.instruction(&Instruction::LocalGet(arg));
                    emit_call(func, reloc_enabled, import_ids["callargs_push_pos"]);
                    func.instruction(&Instruction::Drop);
                }
                let site_bits = box_int(stable_ic_site_id(
                    func_ir.name.as_str(),
                    op_idx,
                    "call_method",
                ));
                func.instruction(&Instruction::I64Const(site_bits));
                func.instruction(&Instruction::LocalGet(method_bits));
                func.instruction(&Instruction::LocalGet(callargs_tmp));
                emit_call(func, reloc_enabled, import_ids["call_bind_ic"]);
            }
            func.instruction(&Instruction::LocalSet(out));
            release_live_object_locals(func, import_ids, reloc_enabled, &live_object_locals);
        }
        _ => return CallOpEmission::NotHandled,
    }

    CallOpEmission::Handled
}
