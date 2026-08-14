use super::super::*;

/// Single-source kind authority for [`handle_call_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "call",
    "call_internal",
    "call_guarded",
    "call_func",
    "invoke_ffi",
    "call_bind",
    "call_indirect",
    "call_method_ic",
    "call_super_method_ic",
    "call_method",
    "getargv",
    "getframe",
    "sys_executable",
];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for direct calls, guarded calls, Python function
/// calls, FFI invocation, call binding, method dispatch ICs, and adjacent
/// process/frame call helpers. Extracted from `compile_func_inner` as a
/// move-only function split: backend state is threaded explicitly, and every
/// handled arm falls through to the parent per-op epilogue.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_call_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    emit_traces: bool,
    has_frame_slot: bool,
    returns_value: bool,
    rc_authority: NativeRcAuthority,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    param_name_set: &BTreeSet<&str>,
    first_defined_at: &BTreeMap<String, usize>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    module_known_functions: &BTreeSet<String>,
    closure_functions: &BTreeSet<String>,
    leaf_functions: &BTreeSet<String>,
    local_closure_envs: &BTreeMap<String, String>,
    known_function_arities: &BTreeMap<String, usize>,
    declared_func_arities: &BTreeMap<String, usize>,
    function_has_ret: &BTreeMap<String, bool>,
    defined_functions: &BTreeSet<String>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    already_decrefed: &mut BTreeSet<String>,
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) {
    match op.kind.as_str() {
        "call" => handle_call_direct_op(
            op,
            op_idx,
            emit_traces,
            has_frame_slot,
            returns_value,
            rc_authority,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            param_name_set,
            last_use,
            alias_roots,
            module_known_functions,
            closure_functions,
            leaf_functions,
            local_closure_envs,
            known_function_arities,
            declared_func_arities,
            function_has_ret,
            defined_functions,
            &mut *block_tracked_obj,
            &mut *block_tracked_ptr,
            &mut *tracked_obj_vars,
            &mut *tracked_vars,
            &mut *tracked_obj_vars_set,
            &mut *tracked_vars_set,
            &mut *entry_vars,
            &mut *already_decrefed,
            local_dec_ref_obj,
            nbc,
        ),
        "call_internal" => handle_call_internal_op(
            op,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            closure_functions,
            local_closure_envs,
            function_has_ret,
            defined_functions,
            nbc,
        ),
        "call_guarded" => handle_call_guarded_op(
            op,
            op_idx,
            func_name,
            emit_traces,
            has_frame_slot,
            returns_value,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            closure_functions,
            local_closure_envs,
            known_function_arities,
            declared_func_arities,
            function_has_ret,
            defined_functions,
            nbc,
        ),
        "call_func" => handle_call_func_op(
            op,
            op_idx,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            first_defined_at,
            last_use,
            nbc,
        ),
        "invoke_ffi" => handle_invoke_ffi_op(
            op,
            op_idx,
            func_name,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            nbc,
        ),
        "call_bind" | "call_indirect" => handle_call_bind_indirect_op(
            op,
            op_idx,
            func_name,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            alias_roots,
            &mut *block_tracked_obj,
            &mut *block_tracked_ptr,
            &mut *tracked_obj_vars,
            &mut *tracked_vars,
            &mut *tracked_obj_vars_set,
            &mut *tracked_vars_set,
            &mut *entry_vars,
            nbc,
        ),
        "call_method_ic" => handle_call_method_ic_op(
            op,
            op_idx,
            func_name,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            nbc,
        ),
        "call_super_method_ic" => handle_call_super_method_ic_op(
            op,
            op_idx,
            func_name,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            nbc,
        ),
        "call_method" => handle_call_method_op(
            op,
            op_idx,
            func_name,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            nbc,
        ),
        "getargv" => handle_getargv_op(op, &mut *module, &mut *import_ids, &mut *builder, vars),
        "getframe" => handle_getframe_op(
            op,
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            representation_plan,
            nbc,
        ),
        "sys_executable" => {
            handle_sys_executable_op(op, &mut *module, &mut *import_ids, &mut *builder, vars)
        }
        _ => unreachable!("non-call op routed to handle_call_op"),
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_call_direct_op(
    op: &OpIR,
    op_idx: usize,
    emit_traces: bool,
    has_frame_slot: bool,
    returns_value: bool,
    rc_authority: NativeRcAuthority,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    param_name_set: &BTreeSet<&str>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    module_known_functions: &BTreeSet<String>,
    closure_functions: &BTreeSet<String>,
    leaf_functions: &BTreeSet<String>,
    local_closure_envs: &BTreeMap<String, String>,
    known_function_arities: &BTreeMap<String, usize>,
    declared_func_arities: &BTreeMap<String, usize>,
    function_has_ret: &BTreeMap<String, bool>,
    defined_functions: &BTreeSet<String>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    already_decrefed: &mut BTreeSet<String>,
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    let target_name = require_static_target_symbol(op);
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let mut args = Vec::new();
    for name in args_names {
        // Deferred overflow re-boxing at call argument.
        let val = ensure_boxed_primitive_safe(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            nbc,
            representation_plan,
            name,
        );
        args.push(val);
    }

    // Collect arg values that are dead after this call. We explicitly avoid
    // decrementing function parameters here: parameters are treated as borrowed
    // by this backend (caller owns), so only non-param temporaries should be
    // released at the call site.
    //
    // RC drop-insertion substrate (design 20 §4.1, ACTIVATION FINDING #2):
    // this per-call-site dead-argument release is the SECOND native
    // value-tracking RC source (alongside the `tracked_*` registration that
    // finding #2(A) already gated) and it is NOT covered by that gate — it
    // computes dead args directly from the SimpleIR `last_use` map, not from
    // the tracked lists. For a `drop_inserted` function the TIR drop pass is
    // the SOLE RC authority and already emits a `DecRef` at every dead value's
    // last use, INCLUDING dead call arguments (`DecRef(arg)` immediately after
    // the call). Letting `arg_cleanup` also fire double-frees every call
    // argument that dies at its call site — the broad-shape over-release UAF
    // (heap-layout-dependent `invalid object header before dec_ref` /
    // refcount-underflow abort) that blocked activation. So under
    // `drop_inserted` we leave `arg_cleanup`/`arg_cleanup_roots` empty: the
    // emit loop becomes a no-op, the root-filtered retains become identity, and
    // `already_decrefed` is not polluted with roots the native side never
    // decrefs (the TIR drop owns them).
    let mut arg_cleanup = Vec::new();
    let mut arg_cleanup_names = BTreeSet::new();
    let mut arg_cleanup_roots = BTreeSet::new();
    if rc_authority.native_value_tracking_enabled() {
        for (name, value) in args_names.iter().zip(args.iter()) {
            if param_name_set.contains(name.as_str()) {
                continue;
            }
            let last = last_use.get(name).copied().unwrap_or(op_idx);
            if last <= op_idx {
                arg_cleanup_names.insert(name.clone());
                let root = alias_root_name(alias_roots, name).to_string();
                if arg_cleanup_roots.insert(root.clone()) {
                    arg_cleanup.push(*value);
                    already_decrefed.insert(root);
                }
            }
        }
    }

    // `call` lowers to a multi-block control-flow sequence (recursion guard +
    // call block + fail block + merge block). If the call happens in a non-entry
    // block, any temporaries tracked on the current block would otherwise be
    // orphaned when we terminate the block with the guard brif. Drain the
    // current block's tracked sets here, but emit the actual decrefs *after* the
    // call (or on the guard-fail path) so arguments remain alive during the call.
    let origin_block = builder
        .current_block()
        .expect("call requires an active block");
    let mut origin_obj_live = block_tracked_obj.remove(&origin_block).unwrap_or_default();
    let origin_obj_cleanup = drain_cleanup_tracked_dedup_with_authority(
        rc_authority,
        &mut origin_obj_live,
        last_use,
        alias_roots,
        op_idx,
        None,
        Some(&mut *already_decrefed),
    );
    let mut origin_ptr_live = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
    let origin_ptr_cleanup = drain_cleanup_tracked_dedup_with_authority(
        rc_authority,
        &mut origin_ptr_live,
        last_use,
        alias_roots,
        op_idx,
        None,
        Some(&mut *already_decrefed),
    );

    // For direct calls to closures, extract env from function object
    if closure_functions.contains(target_name)
        && let Some(func_obj_var) = local_closure_envs.get(target_name)
    {
        let func_obj_bits = *var_get_boxed_overflow_safe(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            func_obj_var,
            representation_plan,
        )
        .expect("Closure func obj not found for direct call");
        let extract_local = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_function_closure_bits",
            &[types::I64],
            &[types::I64],
        );
        let extract_call = builder.ins().call(extract_local, &[func_obj_bits]);
        let env_bits = builder.inst_results(extract_call)[0];
        args.insert(0, env_bits);
    }
    // Declare the target function.
    // Use the previously-declared arity if available, so the
    // Cranelift signature matches the definition even when the
    // call site passes a different number of arguments (e.g.
    // expanded keyword arguments).
    let sig_arity = declared_func_arities
        .get(target_name)
        .copied()
        .or_else(|| known_function_arities.get(target_name).copied())
        .unwrap_or(args.len());
    let target_ret = function_has_ret.get(target_name).copied().unwrap_or(true);
    let mut target_sig = module.make_signature();
    for _ in 0..sig_arity {
        target_sig.params.push(AbiParam::new(types::I64));
    }
    if target_ret {
        target_sig.returns.push(AbiParam::new(types::I64));
    }
    let linkage = if defined_functions.contains(target_name) {
        Linkage::Export
    } else {
        Linkage::Import
    };
    let callee = module
        .declare_function(target_name, linkage, &target_sig)
        .unwrap_or_else(|e| {
            panic!(
                "call declaration mismatch for `{target_name}`: expected \
                 {sig_arity} parameter(s), returns={target_ret}: {e}"
            )
        });
    let local_callee = module.declare_func_in_func(callee, builder.func);

    // --- Fast path: direct call for known defined non-closure functions ---
    // When the target is a defined function in this module (not a closure),
    // emit a direct Cranelift call with a lightweight recursion guard.
    // This avoids: arg spill/reload, match-on-arity dispatch, indirect call.
    //
    // The caller's exception-handling state (`has_exc_handling`) does NOT
    // gate the direct call: the direct dispatch is semantically identical
    // regardless of whether the caller carries a function-level exception
    // label.  Post-call exception routing is handled by the separate
    // CHECK_EXCEPTION op the frontend inserts after the call (lowered to a
    // pending-flag test + branch to the handler), and the recursion-limit
    // path inside this fast path already returns early to propagate a
    // pending RecursionError.  Gating on `has_exc_handling` would disable
    // the fast path for *every* call now that all functions carry an
    // exception label (foundation C2), which is the exact perf regression
    // this exclusion exists to avoid.
    let use_direct_call = (module_known_functions.contains(target_name)
        || matches!(linkage, Linkage::Import))
        && !closure_functions.contains(target_name)
        && args.len() == sig_arity
        && !emit_traces;

    if std::env::var("MOLT_DEBUG_DIRECT_CALL").is_ok() {
        eprintln!(
            "call {} -> direct={} (module_known={} closure={} arity_match={} traces={})",
            target_name,
            use_direct_call,
            module_known_functions.contains(target_name),
            closure_functions.contains(target_name),
            args.len() == sig_arity,
            emit_traces,
        );
    }

    let is_leaf_call = use_direct_call && leaf_functions.contains(target_name);
    let _callee_has_ret = function_has_ret.get(target_name).copied().unwrap_or(true);
    let res = if is_leaf_call {
        // Leaf function: no user-level calls inside, so it
        // cannot recurse.  Skip the recursion guard entirely
        // (saves 2 atomic ops + 2 extern-C calls per call).
        let direct_call = builder.ins().call(local_callee, &args);
        let results = builder.inst_results(direct_call);
        if results.is_empty() {
            builder.ins().iconst(types::I64, box_none())
        } else {
            results[0]
        }
    } else if use_direct_call {
        // Lightweight recursion guard using global atomics
        // (no TLS on the hot path). The data-symbol inline
        // approach was reverted because Cranelift global_value
        // addresses caused segfaults on some programs.
        let enter_ref = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_recursion_enter_fast",
            &[],
            &[types::I64],
        );
        let enter_call = builder.ins().call(enter_ref, &[]);
        let guard_ok = builder.inst_results(enter_call)[0];

        // Branch on recursion guard result.
        let call_block = builder.create_block();
        let error_block = builder.create_block();

        let zero = builder.ins().iconst(types::I64, 0);
        let is_ok = builder.ins().icmp(IntCC::NotEqual, guard_ok, zero);
        brif_block(&mut *builder, is_ok, call_block, &[], error_block, &[]);

        // Error block: recursion limit exceeded (cold path).
        // Return immediately so the pending RecursionError
        // propagates to the caller instead of being silently
        // swallowed as None when no check_exception follows.
        switch_to_block_materialized(&mut *builder, error_block);
        let raise_ref = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_raise_recursion_error",
            &[],
            &[types::I64],
        );
        let raise_call = builder.ins().call(raise_ref, &[]);
        if returns_value {
            let raise_results = builder.inst_results(raise_call);
            let err_val = if raise_results.is_empty() {
                builder.ins().iconst(types::I64, box_none())
            } else {
                raise_results[0]
            };
            if has_frame_slot {
                let trace_exit_ref = import_func_ref(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    "molt_trace_exit",
                    &[],
                    &[types::I64],
                );
                builder.ins().call(trace_exit_ref, &[]);
            }
            builder.ins().return_(&[err_val]);
        } else {
            if has_frame_slot {
                let trace_exit_ref = import_func_ref(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    "molt_trace_exit",
                    &[],
                    &[types::I64],
                );
                builder.ins().call(trace_exit_ref, &[]);
            }
            builder.ins().return_(&[]);
        }

        // Call block: direct call to the target function.
        switch_to_block_materialized(&mut *builder, call_block);
        let direct_call = builder.ins().call(local_callee, &args);
        let direct_results = builder.inst_results(direct_call);
        let call_res = if direct_results.is_empty() {
            builder.ins().iconst(types::I64, box_none())
        } else {
            direct_results[0]
        };

        // Exit recursion guard.
        let exit_ref = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_recursion_exit_fast",
            &[],
            &[],
        );
        builder.ins().call(exit_ref, &[]);
        // The error arm returns from the function, so the successful call arm
        // is the sole continuing path. Keep lowering in that block and return
        // its result directly: a one-predecessor merge/phi is redundant, costs
        // a branch, and discards the FunctionBuilder variable state needed by
        // unrelated live SSA temporaries after the call.
        call_res
    } else {
        // --- Outlined guarded call via molt_guarded_call ---
        // Fallback for imported functions, closures, arity mismatches,
        // or when tracing is enabled.
        let fn_ptr_val = builder.ins().func_addr(types::I64, local_callee);

        // Spill args to a stack slot for the outlined helper.
        let nargs_count = args.len();
        let slot_size = std::cmp::max(nargs_count, 1) * 8;
        let args_slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            slot_size as u32,
            3, // align_shift: 2^3 = 8-byte alignment
        ));
        for (i, arg) in args.iter().enumerate() {
            builder.ins().stack_store(*arg, args_slot, (i * 8) as i32);
        }
        let args_ptr_val = builder.ins().stack_addr(types::I64, args_slot, 0);
        let nargs_val = builder.ins().iconst(types::I64, nargs_count as i64);
        let code_id_val = if emit_traces {
            builder.ins().iconst(types::I64, op.value.unwrap_or(0))
        } else {
            builder.ins().iconst(types::I64, -1i64)
        };

        // Declare and call molt_guarded_call.
        let gc_local = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_guarded_call",
            &[types::I64, types::I64, types::I64, types::I64],
            &[types::I64],
        );
        let gc_call = builder.ins().call(
            gc_local,
            &[fn_ptr_val, args_ptr_val, nargs_val, code_id_val],
        );
        builder.inst_results(gc_call)[0]
    };

    // Tracked-value cleanup (stays inline — varies per site).
    // Re-attach surviving tracked values to the current block.
    if let Some(cur_block) = builder.current_block() {
        if !origin_obj_live.is_empty() {
            extend_unique_tracked(
                block_tracked_obj.entry(cur_block).or_default(),
                origin_obj_live,
            );
        }
        if !origin_ptr_live.is_empty() {
            extend_unique_tracked(
                block_tracked_ptr.entry(cur_block).or_default(),
                origin_ptr_live,
            );
        }
    }
    for name in &origin_obj_cleanup {
        if arg_cleanup_roots.contains(alias_root_name(alias_roots, name)) {
            continue;
        }
        // Use entry_vars (definition-time Value) for dec_ref,
        // not var_get (current SSA Value). If the variable was
        // redefined, var_get returns the WRONG object.
        let val = entry_vars.get(name).copied().or_else(|| {
            var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                representation_plan,
            )
            .map(|v| *v)
        });
        let Some(val) = val else {
            continue;
        };
        builder.ins().call(local_dec_ref_obj, &[val]);
    }
    for name in &origin_ptr_cleanup {
        if arg_cleanup_roots.contains(alias_root_name(alias_roots, name)) {
            continue;
        }
        let val = entry_vars.get(name).copied().or_else(|| {
            var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                representation_plan,
            )
            .map(|v| *v)
        });
        let Some(val) = val else {
            continue;
        };
        builder.ins().call(local_dec_ref_obj, &[val]);
    }
    for val in &arg_cleanup {
        builder.ins().call(local_dec_ref_obj, &[*val]);
    }
    // Remove cleaned-up names from entry-tracked lists so the
    // function-return cleanup does not dec-ref them a second
    // time (the `call` op changes blocks, so the normal
    // entry-tracked drain no longer runs for these variables).
    if !arg_cleanup_roots.is_empty() {
        tracked_obj_vars.retain(|n: &String| {
            !arg_cleanup_roots.contains(alias_root_name(alias_roots, n.as_str()))
        });
        tracked_vars.retain(|n: &String| {
            !arg_cleanup_roots.contains(alias_root_name(alias_roots, n.as_str()))
        });
        tracked_obj_vars_set
            .retain(|n| !arg_cleanup_roots.contains(alias_root_name(alias_roots, n.as_str())));
        tracked_vars_set
            .retain(|n| !arg_cleanup_roots.contains(alias_root_name(alias_roots, n.as_str())));
        entry_vars
            .retain(|name, _| !arg_cleanup_roots.contains(alias_root_name(alias_roots, name)));
    }
    let origin_obj_cleanup_roots = cleanup_roots_for_names(
        alias_roots,
        origin_obj_cleanup
            .iter()
            .filter(|name| !arg_cleanup_roots.contains(alias_root_name(alias_roots, name)))
            .cloned(),
    );
    if !origin_obj_cleanup_roots.is_empty() {
        tracked_obj_vars.retain(|n: &String| {
            !origin_obj_cleanup_roots.contains(alias_root_name(alias_roots, n.as_str()))
        });
        tracked_obj_vars_set.retain(|n| {
            !origin_obj_cleanup_roots.contains(alias_root_name(alias_roots, n.as_str()))
        });
        entry_vars.retain(|name, _| {
            !origin_obj_cleanup_roots.contains(alias_root_name(alias_roots, name))
        });
    }
    let origin_ptr_cleanup_roots = cleanup_roots_for_names(
        alias_roots,
        origin_ptr_cleanup
            .iter()
            .filter(|name| !arg_cleanup_roots.contains(alias_root_name(alias_roots, name)))
            .cloned(),
    );
    if !origin_ptr_cleanup_roots.is_empty() {
        tracked_vars.retain(|n: &String| {
            !origin_ptr_cleanup_roots.contains(alias_root_name(alias_roots, n.as_str()))
        });
        tracked_vars_set.retain(|n| {
            !origin_ptr_cleanup_roots.contains(alias_root_name(alias_roots, n.as_str()))
        });
        entry_vars.retain(|name, _| {
            !origin_ptr_cleanup_roots.contains(alias_root_name(alias_roots, name))
        });
    }
    if let Some(out__) = op.out.as_ref() {
        def_var_named(&mut *builder, vars, out__, res);
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_call_internal_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    closure_functions: &BTreeSet<String>,
    local_closure_envs: &BTreeMap<String, String>,
    function_has_ret: &BTreeMap<String, bool>,
    defined_functions: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    let target_name = require_static_target_symbol(op);
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let mut args = Vec::new();
    for name in args_names {
        args.push(
            *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                representation_plan,
            )
            .expect("Arg not found"),
        );
    }

    // For direct calls to closures, extract env from function object
    if closure_functions.contains(target_name)
        && let Some(func_obj_var) = local_closure_envs.get(target_name)
    {
        let func_obj_bits = *var_get_boxed_overflow_safe(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            func_obj_var,
            representation_plan,
        )
        .expect("Closure func obj not found for direct call");
        let extract_local = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_function_closure_bits",
            &[types::I64],
            &[types::I64],
        );
        let extract_call = builder.ins().call(extract_local, &[func_obj_bits]);
        let env_bits = builder.inst_results(extract_call)[0];
        args.insert(0, env_bits);
    }
    let target_returns = function_has_ret.get(target_name).copied().unwrap_or(true);
    let mut sig = module.make_signature();
    for _ in 0..args.len() {
        sig.params.push(AbiParam::new(types::I64));
    }
    if target_returns {
        sig.returns.push(AbiParam::new(types::I64));
    }
    let linkage = if defined_functions.contains(target_name) {
        Linkage::Export
    } else {
        Linkage::Import
    };

    let callee = match module.declare_function(target_name, linkage, &sig) {
        Ok(id) => id,
        Err(e) => {
            panic!(
                "call_internal declaration mismatch for `{target_name}`: \
                 expected {} parameter(s), returns={target_returns}: {e}",
                args.len()
            );
        }
    };
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &args);
    if target_returns {
        let res = builder.inst_results(call)[0];
        if let Some(out__) = op.out.as_ref() {
            def_var_named(&mut *builder, vars, out__, res);
        }
    } else {
        // Target doesn't return -- assign None if output var requested.
        if let Some(out__) = op.out.as_ref() {
            if representation_plan.is_float_unboxed(out__) {
                let zero_f = builder.ins().f64const(0.0);
                def_var_named(&mut *builder, vars, out__, zero_f);
            } else {
                let none_val = builder.ins().iconst(types::I64, box_none());
                def_var_named(&mut *builder, vars, out__, none_val);
            }
        }
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_call_guarded_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    emit_traces: bool,
    has_frame_slot: bool,
    returns_value: bool,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    closure_functions: &BTreeSet<String>,
    local_closure_envs: &BTreeMap<String, String>,
    known_function_arities: &BTreeMap<String, usize>,
    declared_func_arities: &BTreeMap<String, usize>,
    function_has_ret: &BTreeMap<String, bool>,
    defined_functions: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    let target_name = require_static_target_symbol(op);
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let callee_bits = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[0],
        representation_plan,
    )
    .expect("Callee not found");
    let mut args = Vec::new();
    for name in &args_names[1..] {
        args.push(
            *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                representation_plan,
            )
            .expect("Arg not found"),
        );
    }

    // For direct calls to closures, extract env from function object
    if closure_functions.contains(target_name)
        && let Some(func_obj_var) = local_closure_envs.get(target_name)
    {
        let func_obj_bits = *var_get_boxed_overflow_safe(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            vars,
            func_obj_var,
            representation_plan,
        )
        .expect("Closure func obj not found for direct call");
        let extract_fn = SimpleBackend::import_func_id_split(
            &mut *module,
            &mut *import_ids,
            "molt_function_closure_bits",
            &[types::I64],
            &[types::I64],
        );
        let extract_local = module.declare_func_in_func(extract_fn, builder.func);
        let extract_call = builder.ins().call(extract_local, &[func_obj_bits]);
        let env_bits = builder.inst_results(extract_call)[0];
        args.insert(0, env_bits);
    }
    // Use the previously-declared arity if available so the
    // Cranelift signature matches the definition even when the
    // call site passes a different number of arguments.
    let sig_arity = declared_func_arities
        .get(target_name)
        .copied()
        .or_else(|| known_function_arities.get(target_name).copied())
        .unwrap_or(args.len());
    let target_returns = function_has_ret.get(target_name).copied().unwrap_or(true);
    let mut sig = module.make_signature();
    for _ in 0..sig_arity {
        sig.params.push(AbiParam::new(types::I64));
    }
    if target_returns {
        sig.returns.push(AbiParam::new(types::I64));
    }
    let linkage = if defined_functions.contains(target_name) {
        Linkage::Export
    } else {
        Linkage::Import
    };

    let callee = module
        .declare_function(target_name, linkage, &sig)
        .unwrap_or_else(|e| {
            panic!(
                "call_guarded declaration mismatch for `{target_name}`: expected \
                 {sig_arity} parameter(s), returns={target_returns}: {e}"
            )
        });
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let expected_addr = builder.ins().func_addr(types::I64, local_callee);

    let is_func_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_is_function_obj",
        &[types::I64],
        &[types::I64],
    );
    let truthy_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_is_truthy",
        &[types::I64],
        &[types::I64],
    );
    let guard_enter_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_recursion_guard_enter",
        &[],
        &[types::I64],
    );
    let guard_exit_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_recursion_guard_exit",
        &[],
        &[],
    );
    let trace_enter_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_trace_enter",
        &[types::I64],
        &[types::I64],
    );
    let trace_exit_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_trace_exit",
        &[],
        &[types::I64],
    );
    let is_func_call = builder.ins().call(is_func_local, &[*callee_bits]);
    let is_func_bits = builder.inst_results(is_func_call)[0];
    let truthy_call = builder.ins().call(truthy_local, &[is_func_bits]);
    let truthy_bits = builder.inst_results(truthy_call)[0];
    let is_func_bool = builder.ins().icmp_imm(IntCC::NotEqual, truthy_bits, 0);

    let resolve_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_handle_resolve",
        &[types::I64],
        &[types::I64],
    );
    let merge_block = builder.create_block();
    builder.append_block_param(merge_block, types::I64);

    let func_block = builder.create_block();
    let fallback_block = builder.create_block();
    builder
        .ins()
        .brif(is_func_bool, func_block, &[], fallback_block, &[]);

    switch_to_block_materialized(&mut *builder, fallback_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, fallback_block);
    let callargs_new_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_callargs_new",
        &[types::I64, types::I64],
        &[types::I64],
    );
    let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
    let kw_capacity = builder.ins().iconst(types::I64, 0);
    let callargs_call = builder
        .ins()
        .call(callargs_new_local, &[pos_capacity, kw_capacity]);
    let callargs_ptr = builder.inst_results(callargs_call)[0];
    let callargs_push_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_callargs_push_pos",
        &[types::I64, types::I64],
        &[types::I64],
    );
    for arg in &args {
        builder
            .ins()
            .call(callargs_push_local, &[callargs_ptr, *arg]);
    }
    let call_bind_local = import_func_ref(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        "molt_call_bind_ic",
        &[types::I64, types::I64, types::I64],
        &[types::I64],
    );
    let site_bits = builder.ins().iconst(
        types::I64,
        box_int(stable_ic_site_id(func_name, op_idx, "call_guarded")),
    );
    let fallback_call = builder
        .ins()
        .call(call_bind_local, &[site_bits, *callee_bits, callargs_ptr]);
    let fallback_res = builder.inst_results(fallback_call)[0];
    jump_block(&mut *builder, merge_block, &[fallback_res]);

    switch_to_block_materialized(&mut *builder, func_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, func_block);
    let resolve_call = builder.ins().call(resolve_local, &[*callee_bits]);
    let func_ptr = builder.inst_results(resolve_call)[0];
    let fn_ptr = builder
        .ins()
        .load(types::I64, MemFlagsData::trusted(), func_ptr, 0);
    let matches = builder.ins().icmp(IntCC::Equal, fn_ptr, expected_addr);
    let then_block = builder.create_block();
    let else_block = builder.create_block();
    builder
        .ins()
        .brif(matches, then_block, &[], else_block, &[]);

    switch_to_block_materialized(&mut *builder, then_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, then_block);
    let guard_call = builder.ins().call(guard_enter_local, &[]);
    let guard_val = builder.inst_results(guard_call)[0];
    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
    let then_call_block = builder.create_block();
    let then_fail_block = builder.create_block();
    builder
        .ins()
        .brif(guard_ok, then_call_block, &[], then_fail_block, &[]);

    switch_to_block_materialized(&mut *builder, then_call_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, then_call_block);
    if emit_traces {
        let _ = builder.ins().call(trace_enter_local, &[*callee_bits]);
    }
    let direct_call = builder.ins().call(local_callee, &args);
    let direct_results = builder.inst_results(direct_call);
    let direct_res = if direct_results.is_empty() {
        builder.ins().iconst(types::I64, box_none())
    } else {
        direct_results[0]
    };
    if emit_traces {
        let _ = builder.ins().call(trace_exit_local, &[]);
    }
    let _ = builder.ins().call(guard_exit_local, &[]);
    jump_block(&mut *builder, merge_block, &[direct_res]);

    switch_to_block_materialized(&mut *builder, then_fail_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, then_fail_block);
    // Recursion guard failed — exception is already pending
    // from molt_recursion_guard_enter.  Return immediately so
    // the pending RecursionError propagates to the caller
    // instead of being silently swallowed as None (which
    // caused TypeError: NoneType + int downstream).
    if has_frame_slot {
        let trace_exit_ref = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_trace_exit",
            &[],
            &[types::I64],
        );
        builder.ins().call(trace_exit_ref, &[]);
    }
    if returns_value {
        let none_bits = builder.ins().iconst(types::I64, box_none());
        builder.ins().return_(&[none_bits]);
    } else {
        builder.ins().return_(&[]);
    }

    switch_to_block_materialized(&mut *builder, else_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, else_block);
    let guard_call = builder.ins().call(guard_enter_local, &[]);
    let guard_val = builder.inst_results(guard_call)[0];
    let guard_ok = builder.ins().icmp_imm(IntCC::NotEqual, guard_val, 0);
    let else_call_block = builder.create_block();
    let else_fail_block = builder.create_block();
    builder
        .ins()
        .brif(guard_ok, else_call_block, &[], else_fail_block, &[]);

    switch_to_block_materialized(&mut *builder, else_call_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, else_call_block);
    if emit_traces {
        let _ = builder.ins().call(trace_enter_local, &[*callee_bits]);
    }
    let sig_ref = builder.import_signature(sig);
    let fallback_call = builder.ins().call_indirect(sig_ref, fn_ptr, &args);
    let fallback_results = builder.inst_results(fallback_call);
    let fallback_res = if fallback_results.is_empty() {
        builder.ins().iconst(types::I64, box_none())
    } else {
        fallback_results[0]
    };
    if emit_traces {
        let _ = builder.ins().call(trace_exit_local, &[]);
    }
    let _ = builder.ins().call(guard_exit_local, &[]);
    jump_block(&mut *builder, merge_block, &[fallback_res]);

    switch_to_block_materialized(&mut *builder, else_fail_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, else_fail_block);
    // Same as then_fail_block: return immediately on recursion
    // guard failure so the pending RecursionError propagates.
    if has_frame_slot {
        let trace_exit_ref = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_trace_exit",
            &[],
            &[types::I64],
        );
        builder.ins().call(trace_exit_ref, &[]);
    }
    if returns_value {
        let none_bits = builder.ins().iconst(types::I64, box_none());
        builder.ins().return_(&[none_bits]);
    } else {
        builder.ins().return_(&[]);
    }

    switch_to_block_materialized(&mut *builder, merge_block);
    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
    let res = builder.block_params(merge_block)[0];
    if let Some(out__) = op.out.as_ref() {
        def_var_named(&mut *builder, vars, out__, res);
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_call_func_op(
    op: &OpIR,
    op_idx: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    first_defined_at: &BTreeMap<String, usize>,
    last_use: &BTreeMap<String, usize>,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    // Inline probe fast-path: for 0–3 positional args with no tracing,
    // emit Cranelift IR that checks the callable's type/arity/closure
    // inline and calls through fn_ptr via call_indirect.  This avoids
    // ALL function-call overhead for the common case (non-closure,
    // exact arity, TYPE_ID_FUNCTION).  On the fast path, the generated
    // code does: tag check -> load type_id -> load closure_bits ->
    // load arity -> load fn_ptr -> recursion guard -> call_indirect.
    // All loads hit the same cache line, so this is very cheap.
    //
    // Slow path: falls back to molt_call_func_fast{N} for closures,
    // bound methods, arity mismatches; or molt_call_func_dispatch
    // for >3 args or tracing.
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let func_bits = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[0],
        representation_plan,
    )
    .expect("Func not found");
    let mut args = Vec::new();
    for name in &args_names[1..] {
        args.push(
            *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                representation_plan,
            )
            .expect("Arg not found"),
        );
    }
    let code_id = op.value.unwrap_or(0);
    let nargs = args.len();

    let use_inline_probe = nargs <= 3 && code_id == 0;
    let inline_live_through = if use_inline_probe {
        collect_live_through_values(
            &mut *builder,
            vars,
            first_defined_at,
            last_use,
            op_idx,
            op.out.as_deref(),
        )
    } else {
        Vec::new()
    };

    let res = if use_inline_probe {
        // --- Inline probe: check tag, type_id, closure, arity ---
        let tag_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
        let expected_ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
        let ptr_mask_val = builder.ins().iconst(types::I64, nbc.pointer_mask);

        let merge_block = builder.create_block();
        builder.append_block_param(merge_block, types::I64);
        append_live_through_params(&mut *builder, merge_block, &inline_live_through);
        let slow_block = builder.create_block();

        // Step 1: Check TAG_PTR
        let tag = builder.ins().band(*func_bits, tag_mask);
        let is_ptr = builder.ins().icmp(IntCC::Equal, tag, expected_ptr_tag);
        let probe_block = builder.create_block();
        brif_block(&mut *builder, is_ptr, probe_block, &[], slow_block, &[]);

        // Step 2: Extract pointer, check TYPE_ID_FUNCTION
        switch_to_block_materialized(&mut *builder, probe_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, probe_block);
        let raw_ptr = builder.ins().band(*func_bits, ptr_mask_val);
        let shift16 = builder.ins().iconst(types::I64, 16);
        let shifted = builder.ins().ishl(raw_ptr, shift16);
        let ptr_val = builder.ins().sshr(shifted, shift16);
        let type_id = builder
            .ins()
            .load(types::I32, MemFlagsData::trusted(), ptr_val, -16i32);
        let expected_type = builder
            .ins()
            .iconst(types::I32, i64::from(TYPE_ID_FUNCTION));
        let type_ok = builder.ins().icmp(IntCC::Equal, type_id, expected_type);
        let closure_check_block = builder.create_block();
        brif_block(
            &mut *builder,
            type_ok,
            closure_check_block,
            &[],
            slow_block,
            &[],
        );

        // Step 3: Check closure_bits == 0 (at ptr+24)
        switch_to_block_materialized(&mut *builder, closure_check_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, closure_check_block);
        let closure_bits_v =
            builder
                .ins()
                .load(types::I64, MemFlagsData::trusted(), ptr_val, 24i32);
        let zero = builder.ins().iconst(types::I64, 0);
        let no_closure = builder.ins().icmp(IntCC::Equal, closure_bits_v, zero);
        let trampoline_check_block = builder.create_block();
        brif_block(
            &mut *builder,
            no_closure,
            trampoline_check_block,
            &[],
            slow_block,
            &[],
        );

        // Step 4: Reject trampoline-backed functions. Those are
        // lowered through the canonical runtime trampoline path
        // rather than a raw fn_ptr call.
        switch_to_block_materialized(&mut *builder, trampoline_check_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, trampoline_check_block);
        let tramp_ptr_v = builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), ptr_val, 40i32);
        let no_trampoline = builder.ins().icmp(IntCC::Equal, tramp_ptr_v, zero);
        let binder_check_block = builder.create_block();
        let arity_check_block = builder.create_block();
        brif_block(
            &mut *builder,
            no_trampoline,
            binder_check_block,
            &[],
            slow_block,
            &[],
        );

        // Step 5: Reject functions whose Python call shape must
        // be bound before ABI dispatch (`*args`, keyword-only
        // params/defaults, `**kwargs`, or a builtin bind kind).
        // The runtime owns this shape bit because metadata can
        // be attached after function allocation.
        switch_to_block_materialized(&mut *builder, binder_check_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, binder_check_block);
        let requires_binder_ref = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_function_requires_binder_fast",
            &[types::I64],
            &[types::I64],
        );
        let requires_binder_call = builder.ins().call(requires_binder_ref, &[*func_bits]);
        let requires_binder = builder.inst_results(requires_binder_call)[0];
        let no_binder = builder.ins().icmp_imm(IntCC::Equal, requires_binder, 0);
        brif_block(
            &mut *builder,
            no_binder,
            arity_check_block,
            &[],
            slow_block,
            &[],
        );

        // Step 6: Check arity (at ptr+8)
        switch_to_block_materialized(&mut *builder, arity_check_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, arity_check_block);
        let arity = builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), ptr_val, 8i32);
        let expected_arity = builder.ins().iconst(types::I64, nargs as i64);
        let arity_ok = builder.ins().icmp(IntCC::Equal, arity, expected_arity);
        let direct_call_block = builder.create_block();
        brif_block(
            &mut *builder,
            arity_ok,
            direct_call_block,
            &[],
            slow_block,
            &[],
        );

        // Step 7: Load fn_ptr (at ptr+0), recursion guard, call_indirect
        switch_to_block_materialized(&mut *builder, direct_call_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, direct_call_block);
        let fn_ptr_v = builder
            .ins()
            .load(types::I64, MemFlagsData::trusted(), ptr_val, 0i32);
        let guard_enter = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_recursion_enter_fast",
            &[],
            &[types::I64],
        );
        let enter_call = builder.ins().call(guard_enter, &[]);
        let guard_ok = builder.inst_results(enter_call)[0];
        let guard_zero = builder.ins().iconst(types::I64, 0);
        let is_guard_ok = builder.ins().icmp(IntCC::NotEqual, guard_ok, guard_zero);
        let call_block = builder.create_block();
        let guard_fail_block = builder.create_block();
        brif_block(
            &mut *builder,
            is_guard_ok,
            call_block,
            &[],
            guard_fail_block,
            &[],
        );

        // Guard fail: raise RecursionError (cold)
        switch_to_block_materialized(&mut *builder, guard_fail_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, guard_fail_block);
        let raise_ref = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_raise_recursion_error",
            &[],
            &[types::I64],
        );
        let raise_call = builder.ins().call(raise_ref, &[]);
        let err_val = builder.inst_results(raise_call)[0];
        let merge_args = merge_args_with_live_through(err_val, &inline_live_through);
        jump_block(&mut *builder, merge_block, &merge_args);

        // Direct call via call_indirect
        switch_to_block_materialized(&mut *builder, call_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
        let mut call_sig = module.make_signature();
        for _ in 0..nargs {
            call_sig.params.push(AbiParam::new(types::I64));
        }
        call_sig.returns.push(AbiParam::new(types::I64));
        let sig_ref = builder.import_signature(call_sig);
        let indirect_call = builder.ins().call_indirect(sig_ref, fn_ptr_v, &args);
        let direct_res = builder.inst_results(indirect_call)[0];
        let guard_exit = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_recursion_exit_fast",
            &[],
            &[],
        );
        builder.ins().call(guard_exit, &[]);
        let merge_args = merge_args_with_live_through(direct_res, &inline_live_through);
        jump_block(&mut *builder, merge_block, &merge_args);

        // Slow path: call molt_call_func_fast{N}
        switch_to_block_materialized(&mut *builder, slow_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
        let fast_name: &'static str = match nargs {
            0 => "molt_call_func_fast0",
            1 => "molt_call_func_fast1",
            2 => "molt_call_func_fast2",
            3 => "molt_call_func_fast3",
            _ => unreachable!(),
        };
        let param_types = vec![types::I64; nargs + 1];
        let fast_ref = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            fast_name,
            &param_types,
            &[types::I64],
        );
        let mut slow_call_args = Vec::with_capacity(nargs + 1);
        slow_call_args.push(*func_bits);
        slow_call_args.extend_from_slice(&args);
        let slow_call = builder.ins().call(fast_ref, &slow_call_args);
        let slow_res = builder.inst_results(slow_call)[0];
        let merge_args = merge_args_with_live_through(slow_res, &inline_live_through);
        jump_block(&mut *builder, merge_block, &merge_args);

        switch_to_block_materialized(&mut *builder, merge_block);
        seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
        let merge_params = builder.block_params(merge_block).to_vec();
        rebind_live_through_values(
            &mut *builder,
            vars,
            &inline_live_through,
            &merge_params[1..],
        );
        merge_params[0]
    } else {
        // Fallback: spill to stack + call molt_call_func_dispatch.
        let slot_size = std::cmp::max(nargs, 1) * 8;
        let args_slot = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            slot_size as u32,
            3, // align_shift: 2^3 = 8-byte alignment
        ));
        for (i, arg) in args.iter().enumerate() {
            builder.ins().stack_store(*arg, args_slot, (i * 8) as i32);
        }
        let args_ptr = builder.ins().stack_addr(types::I64, args_slot, 0);
        let nargs_val = builder.ins().iconst(types::I64, nargs as i64);
        let code_id_val = builder.ins().iconst(types::I64, code_id);
        let callee = SimpleBackend::import_func_id_split(
            &mut *module,
            &mut *import_ids,
            "molt_call_func_dispatch",
            &[types::I64, types::I64, types::I64, types::I64],
            &[types::I64],
        );
        let local_callee = module.declare_func_in_func(callee, builder.func);
        let call = builder.ins().call(
            local_callee,
            &[*func_bits, args_ptr, nargs_val, code_id_val],
        );
        builder.inst_results(call)[0]
    };
    if let Some(out__) = op.out.as_ref() {
        def_var_from_boxed_transport(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            vars,
            representation_plan,
            nbc,
            out__,
            res,
        );
    }
}

#[cfg(feature = "native-backend")]
fn native_callable_cranelift_type(
    machine_type: molt_ir::native_callable_abi::NativeCallableMachineType,
    pointer_type: types::Type,
) -> types::Type {
    use molt_ir::native_callable_abi::NativeCallableMachineType;
    match machine_type {
        NativeCallableMachineType::MoltValue | NativeCallableMachineType::U64 => types::I64,
        NativeCallableMachineType::Pointer => pointer_type,
        NativeCallableMachineType::I32 => types::I32,
    }
}

#[cfg(feature = "native-backend")]
fn declare_native_callable_symbol(
    module: &mut ObjectModule,
    builder: &mut FunctionBuilder<'_>,
    symbol: &str,
    signature: &molt_ir::native_callable_abi::NativeCallableMachineSignature,
) -> FuncRef {
    let pointer_type = module.target_config().pointer_type();
    let mut cranelift_signature = module.make_signature();
    cranelift_signature.params.extend(
        signature
            .params
            .iter()
            .copied()
            .map(|machine_type| native_callable_cranelift_type(machine_type, pointer_type))
            .map(AbiParam::new),
    );
    cranelift_signature.returns.extend(
        signature
            .results
            .iter()
            .copied()
            .map(|machine_type| native_callable_cranelift_type(machine_type, pointer_type))
            .map(AbiParam::new),
    );
    let function = module
        .declare_function(symbol, Linkage::Import, &cranelift_signature)
        .unwrap_or_else(|error| {
            panic!(
                "native callable direct symbol `{symbol}` declaration conflicts with its canonical ABI signature: {error}"
            )
        });
    module.declare_func_in_func(function, builder.func)
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn emit_native_forward_f32_call(
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    symbol: &str,
    signature: &molt_ir::native_callable_abi::NativeCallableMachineSignature,
    input_bits: Value,
) -> Value {
    let pointer_type = module.target_config().pointer_type();
    let direct_symbol = declare_native_callable_symbol(module, builder, symbol, signature);
    let bytes_as_ptr = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_bytes_as_ptr",
        &[types::I64, pointer_type],
        &[pointer_type],
    );
    let scratch_alloc = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_scratch_alloc",
        &[types::I64],
        &[types::I64],
    );
    let scratch_free = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_scratch_free",
        &[types::I64, types::I64],
        &[],
    );
    let bytes_from = import_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        "molt_bytes_from",
        &[pointer_type, types::I64],
        &[types::I64],
    );

    let length_slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
    let zero = builder.ins().iconst(types::I64, 0);
    builder.ins().stack_store(zero, length_slot, 0);
    let length_ptr = builder.ins().stack_addr(pointer_type, length_slot, 0);
    let input_call = builder.ins().call(bytes_as_ptr, &[input_bits, length_ptr]);
    let input_ptr = builder.inst_results(input_call)[0];
    let input_len = builder.ins().stack_load(types::I64, length_slot, 0);

    let have_input = builder.create_block();
    let have_output = builder.create_block();
    let convert_output = builder.create_block();
    let cleanup_output = builder.create_block();
    builder.append_block_param(cleanup_output, types::I64);
    let done = builder.create_block();
    builder.append_block_param(done, types::I64);

    let input_valid = builder.ins().icmp_imm(IntCC::NotEqual, input_ptr, 0);
    builder
        .ins()
        .brif(input_valid, have_input, &[], done, &[BlockArg::from(zero)]);

    switch_to_block_materialized(builder, have_input);
    seal_block_once(builder, sealed_blocks, have_input);
    let output_call = builder.ins().call(scratch_alloc, &[input_len]);
    let output_ptr = builder.inst_results(output_call)[0];
    let output_valid = builder.ins().icmp_imm(IntCC::NotEqual, output_ptr, 0);
    builder.ins().brif(
        output_valid,
        have_output,
        &[],
        done,
        &[BlockArg::from(zero)],
    );

    switch_to_block_materialized(builder, have_output);
    seal_block_once(builder, sealed_blocks, have_output);
    let output_native_ptr = if pointer_type == types::I64 {
        output_ptr
    } else {
        builder.ins().ireduce(pointer_type, output_ptr)
    };
    let native_call = builder
        .ins()
        .call(direct_symbol, &[input_ptr, input_len, output_native_ptr]);
    let native_status = builder.inst_results(native_call)[0];
    let native_ok = builder.ins().icmp_imm(IntCC::Equal, native_status, 0);
    builder.ins().brif(
        native_ok,
        convert_output,
        &[],
        cleanup_output,
        &[BlockArg::from(zero)],
    );

    switch_to_block_materialized(builder, convert_output);
    seal_block_once(builder, sealed_blocks, convert_output);
    let result_call = builder
        .ins()
        .call(bytes_from, &[output_native_ptr, input_len]);
    let result_bits = builder.inst_results(result_call)[0];
    jump_block(builder, cleanup_output, &[result_bits]);

    switch_to_block_materialized(builder, cleanup_output);
    seal_block_once(builder, sealed_blocks, cleanup_output);
    let result_bits = builder.block_params(cleanup_output)[0];
    builder.ins().call(scratch_free, &[output_ptr, input_len]);
    jump_block(builder, done, &[result_bits]);

    switch_to_block_materialized(builder, done);
    seal_block_once(builder, sealed_blocks, done);
    builder.block_params(done)[0]
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_invoke_ffi_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    // `module_attr` exports resolve a callable object and share the runtime FFI
    // inline-cache path with WASM. `direct_symbol` exports instead declare an
    // object-file import with the canonical machine signature owned by
    // `molt-ir`; the final native linker resolves that relocation from the
    // checksummed source-extension archive selected during admission.
    let module_attr_dispatch = if let Some(export_name) = op.native_callable_export.as_deref() {
        let binding = op.native_callable_binding.as_deref().unwrap_or("<missing>");
        let abi = op.native_callable_abi.as_deref().unwrap_or("<missing>");
        let abi_contract = molt_ir::native_callable_abi::parse_native_callable_abi(abi)
            .unwrap_or_else(|| {
                panic!("native callable export `{export_name}` declares unknown ABI `{abi}`")
            });
        if binding == "direct_symbol" {
            let symbol = op
                .native_callable_symbol
                .as_deref()
                .unwrap_or_else(|| {
                    panic!(
                        "native callable export `{export_name}` uses direct_symbol without native_callable_symbol"
                    )
                });
            if symbol.is_empty() {
                panic!("native callable export `{export_name}` has an empty direct symbol");
            }
            let args_names = op.args.as_ref().unwrap_or_else(|| {
                panic!("native callable export `{export_name}` invoke_ffi is missing args")
            });
            let arity = args_names.len();
            if let Some(expected) = abi_contract.fixed_arity()
                && arity != expected
            {
                panic!(
                    "native callable export `{export_name}` declares `{}` with arity {arity}; expected exactly {expected} ABI payload argument(s)",
                    abi_contract.token()
                );
            }
            let machine_signature = abi_contract
                .native_machine_signature(arity)
                .expect("validated native callable arity must have a machine signature");
            let result = match abi_contract {
                molt_ir::native_callable_abi::NativeCallableAbi::ForwardF32V1 => {
                    let input_bits = *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args_names[0],
                        representation_plan,
                    )
                    .expect("native forward_f32 payload not found");
                    emit_native_forward_f32_call(
                        module,
                        import_ids,
                        builder,
                        import_refs,
                        sealed_blocks,
                        symbol,
                        &machine_signature,
                        input_bits,
                    )
                }
                molt_ir::native_callable_abi::NativeCallableAbi::PyinitModuleV1 => {
                    let pointer_type = module.target_config().pointer_type();
                    let direct_symbol =
                        declare_native_callable_symbol(module, builder, symbol, &machine_signature);
                    let call = builder.ins().call(direct_symbol, &[]);
                    let pointer = builder.inst_results(call)[0];
                    if pointer_type == types::I64 {
                        pointer
                    } else {
                        builder.ins().uextend(types::I64, pointer)
                    }
                }
                molt_ir::native_callable_abi::NativeCallableAbi::ObjectCallV1
                | molt_ir::native_callable_abi::NativeCallableAbi::ObjectCallargsV1 => {
                    let mut args = Vec::with_capacity(args_names.len());
                    for name in args_names {
                        args.push(
                            *var_get_boxed_overflow_safe(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                &mut *import_refs,
                                &mut *sealed_blocks,
                                vars,
                                name,
                                representation_plan,
                            )
                            .unwrap_or_else(|| {
                                panic!(
                                    "native callable export `{export_name}` payload `{name}` not found"
                                )
                            }),
                        );
                    }
                    let direct_symbol =
                        declare_native_callable_symbol(module, builder, symbol, &machine_signature);
                    let call = builder.ins().call(direct_symbol, &args);
                    builder.inst_results(call)[0]
                }
            };
            if let Some(out) = op.out.as_ref() {
                def_var_from_boxed_transport(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    out,
                    result,
                );
            }
            return;
        }
        if binding != "module_attr" {
            panic!("native callable export `{export_name}` uses unsupported binding `{binding}`");
        }
        if abi_contract.requires_direct_symbol_binding() {
            panic!(
                "native callable module_attr export `{export_name}` cannot use direct-symbol memory ABI `{}`",
                abi_contract.token()
            );
        }
        Some(abi_contract)
    } else {
        None
    };
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let func_bits = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[0],
        representation_plan,
    )
    .expect("Func not found");

    // `object_callargs_v1` module_attr exports pass the callargs object through
    // `args[1]` directly; every other lane materializes positional args into a
    // fresh callargs builder.
    let prebuilt_callargs = if matches!(
        module_attr_dispatch,
        Some(molt_ir::native_callable_abi::NativeCallableAbi::ObjectCallargsV1)
    ) {
        let export_name = op.native_callable_export.as_deref().unwrap_or("<export>");
        if args_names.len() != 2 {
            panic!(
                "native callable module_attr export `{export_name}` object_callargs ABI expects the callable handle plus exactly one callargs payload; got {} arg(s)",
                args_names.len()
            );
        }
        Some(
            *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args_names[1],
                representation_plan,
            )
            .expect("Callargs payload not found"),
        )
    } else {
        None
    };

    let callargs_ptr = if let Some(callargs_ptr) = prebuilt_callargs {
        callargs_ptr
    } else {
        let mut args = Vec::new();
        for name in &args_names[1..] {
            args.push(
                *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    name,
                    representation_plan,
                )
                .expect("Arg not found"),
            );
        }
        let callargs_new_local = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_callargs_new",
            &[types::I64, types::I64],
            &[types::I64],
        );
        let pos_capacity = builder.ins().iconst(types::I64, args.len() as i64);
        let kw_capacity = builder.ins().iconst(types::I64, 0);
        let callargs_call = builder
            .ins()
            .call(callargs_new_local, &[pos_capacity, kw_capacity]);
        let callargs_ptr = builder.inst_results(callargs_call)[0];

        let callargs_push_local = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_callargs_push_pos",
            &[types::I64, types::I64],
            &[types::I64],
        );
        for arg in &args {
            builder
                .ins()
                .call(callargs_push_local, &[callargs_ptr, *arg]);
        }
        callargs_ptr
    };

    let bridge_lane = op.s_value.as_deref() == Some("bridge");
    let call_site_label = if bridge_lane {
        "invoke_ffi_bridge"
    } else {
        "invoke_ffi_deopt"
    };
    let site_bits = builder.ins().iconst(
        types::I64,
        box_int(stable_ic_site_id(func_name, op_idx, call_site_label)),
    );
    let require_bridge_cap = builder
        .ins()
        .iconst(types::I64, box_bool(if bridge_lane { 1 } else { 0 }));

    let invoke_fn = SimpleBackend::import_func_id_split(
        &mut *module,
        &mut *import_ids,
        "molt_invoke_ffi_ic",
        &[types::I64, types::I64, types::I64, types::I64],
        &[types::I64],
    );
    let invoke_local = module.declare_func_in_func(invoke_fn, builder.func);
    let invoke_call = builder.ins().call(
        invoke_local,
        &[site_bits, *func_bits, callargs_ptr, require_bridge_cap],
    );
    let res = builder.inst_results(invoke_call)[0];
    if let Some(out__) = op.out.as_ref() {
        def_var_from_boxed_transport(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            vars,
            representation_plan,
            nbc,
            out__,
            res,
        );
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_call_bind_indirect_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    alias_roots: &BTreeMap<String, String>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let func_bits = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[0],
        representation_plan,
    )
    .expect("Func not found");
    let builder_ptr = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[1],
        representation_plan,
    )
    .expect("Callargs not found");
    let callargs_name = &args_names[1];
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.params.push(AbiParam::new(types::I64));
    sig.returns.push(AbiParam::new(types::I64));
    let callee_name = if op.kind == "call_indirect" {
        "molt_call_indirect_ic"
    } else {
        "molt_call_bind_ic"
    };
    let local_callee = if op.kind == "call_bind" {
        import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_call_bind_ic",
            &[types::I64, types::I64, types::I64],
            &[types::I64],
        )
    } else {
        let callee = module
            .declare_function(callee_name, Linkage::Import, &sig)
            .unwrap();
        module.declare_func_in_func(callee, builder.func)
    };
    let call_site_label = if op.kind == "call_indirect" {
        "call_indirect"
    } else {
        "call_bind"
    };
    let site_bits = builder.ins().iconst(
        types::I64,
        box_int(stable_ic_site_id(func_name, op_idx, call_site_label)),
    );
    let call = builder
        .ins()
        .call(local_callee, &[site_bits, *func_bits, *builder_ptr]);
    let res = builder.inst_results(call)[0];
    if let Some(out__) = op.out.as_ref() {
        def_var_from_boxed_transport(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            vars,
            representation_plan,
            nbc,
            out__,
            res,
        );
    }

    // `molt_call_bind*` consumes the CallArgs builder pointer and decrefs it
    // internally (see `PtrDropGuard` in runtime). The backend's lifetime tracking
    // must therefore *not* emit an additional decref for the builder variable,
    // or we'll double-free the CallArgs object and corrupt unrelated state.
    //
    // call_bind consumes the callargs builder. Remove it from
    // tracking to prevent double-free. The last_use assertion is
    // omitted: the IR may reference the variable in unreachable
    // branches (different if/else arms), inflating last_use.
    let consumed_builder_roots = cleanup_roots_for_names(alias_roots, [callargs_name.to_string()]);
    scrub_tracked_roots(
        &consumed_builder_roots,
        alias_roots,
        &mut *tracked_vars,
        &mut *tracked_obj_vars,
        &mut *tracked_vars_set,
        &mut *tracked_obj_vars_set,
        &mut *entry_vars,
        &mut *block_tracked_obj,
        &mut *block_tracked_ptr,
    );
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_call_method_ic_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    // Fused instance-method dispatch (LOAD_METHOD/CALL_METHOD):
    //   args = [recv, a0, a1, ...]  s_value = <method name>
    // Lowers to a single `molt_call_method_icN(site, recv, name,
    // name_len, a0..)` call — no bound-method/callargs alloc on
    // the fast path, identical legacy behaviour on the slow path.
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let recv_bits = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[0],
        representation_plan,
    )
    .expect("call_method_ic receiver not found");
    let mut extra_args = Vec::new();
    for name in &args_names[1..] {
        extra_args.push(
            *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                representation_plan,
            )
            .expect("call_method_ic arg not found"),
        );
    }
    let method_name = op
        .s_value
        .as_ref()
        .expect("call_method_ic missing method name");
    // Emit the method name as a private data symbol (same shape
    // as get_attr_generic_ptr) and pass (ptr, len).
    let data_id = module
        .declare_data(
            &format!("mname_{}_{}", func_name, op_idx),
            Linkage::Local,
            false,
            false,
        )
        .unwrap();
    let mut data_ctx = DataDescription::new();
    data_ctx.define(method_name.as_bytes().to_vec().into_boxed_slice());
    module.define_data(data_id, &data_ctx).unwrap();
    let global_ptr = module.declare_data_in_func(data_id, builder.func);
    let name_ptr = builder.ins().symbol_value(types::I64, global_ptr);
    let name_len = builder.ins().iconst(types::I64, method_name.len() as i64);
    let site_bits = builder.ins().iconst(
        types::I64,
        box_int(stable_ic_site_id(func_name, op_idx, "call_method_ic")),
    );
    let symbol = match extra_args.len() {
        0 => "molt_call_method_ic0",
        1 => "molt_call_method_ic1",
        2 => "molt_call_method_ic2",
        3 => "molt_call_method_ic3",
        _ => "molt_call_method_ic4",
    };
    // site + recv + name_ptr + name_len + one I64 per extra arg.
    let sig_params = vec![types::I64; 4 + extra_args.len()];
    let callee = SimpleBackend::import_func_id_split(
        &mut *module,
        &mut *import_ids,
        symbol,
        &sig_params,
        &[types::I64],
    );
    let local = module.declare_func_in_func(callee, builder.func);
    let mut call_args = vec![site_bits, *recv_bits, name_ptr, name_len];
    call_args.extend_from_slice(&extra_args);
    let call = builder.ins().call(local, &call_args);
    let res = builder.inst_results(call)[0];
    if let Some(out__) = op.out.as_ref() {
        def_var_from_boxed_transport(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            vars,
            representation_plan,
            nbc,
            out__,
            res,
        );
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_call_super_method_ic_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    // Fused `super().method(args)` dispatch (no super-object /
    // bound-method / callargs allocation on the fast path):
    //   args = [class, self, a0, a1, ...]  s_value = <method>
    // Lowers to `molt_call_super_method_icN(site, class, self,
    // name, name_len, a0..)`.
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let class_bits = *var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[0],
        representation_plan,
    )
    .expect("call_super_method_ic class not found");
    let self_bits = *var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[1],
        representation_plan,
    )
    .expect("call_super_method_ic self not found");
    let mut extra_args = Vec::new();
    for name in &args_names[2..] {
        extra_args.push(
            *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                representation_plan,
            )
            .expect("call_super_method_ic arg not found"),
        );
    }
    let method_name = op
        .s_value
        .as_ref()
        .expect("call_super_method_ic missing method name");
    let data_id = module
        .declare_data(
            &format!("smname_{}_{}", func_name, op_idx),
            Linkage::Local,
            false,
            false,
        )
        .unwrap();
    let mut data_ctx = DataDescription::new();
    data_ctx.define(method_name.as_bytes().to_vec().into_boxed_slice());
    module.define_data(data_id, &data_ctx).unwrap();
    let global_ptr = module.declare_data_in_func(data_id, builder.func);
    let name_ptr = builder.ins().symbol_value(types::I64, global_ptr);
    let name_len = builder.ins().iconst(types::I64, method_name.len() as i64);
    let site_bits = builder.ins().iconst(
        types::I64,
        box_int(stable_ic_site_id(func_name, op_idx, "call_super_method_ic")),
    );
    let symbol = match extra_args.len() {
        0 => "molt_call_super_method_ic0",
        1 => "molt_call_super_method_ic1",
        2 => "molt_call_super_method_ic2",
        3 => "molt_call_super_method_ic3",
        _ => "molt_call_super_method_ic4",
    };
    // site + class + self + name_ptr + name_len + one per arg.
    let sig_params = vec![types::I64; 5 + extra_args.len()];
    let callee = SimpleBackend::import_func_id_split(
        &mut *module,
        &mut *import_ids,
        symbol,
        &sig_params,
        &[types::I64],
    );
    let local = module.declare_func_in_func(callee, builder.func);
    let mut call_args = vec![site_bits, class_bits, self_bits, name_ptr, name_len];
    call_args.extend_from_slice(&extra_args);
    let call = builder.ins().call(local, &call_args);
    let res = builder.inst_results(call)[0];
    if let Some(out__) = op.out.as_ref() {
        def_var_from_boxed_transport(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            vars,
            representation_plan,
            nbc,
            out__,
            res,
        );
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_call_method_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let method_bits = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args_names[0],
        representation_plan,
    )
    .expect("Method not found");
    let mut extra_args = Vec::new();
    for name in &args_names[1..] {
        extra_args.push(
            *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                representation_plan,
            )
            .expect("Arg not found"),
        );
    }

    // --- Fast-path: dispatch known bound-method patterns
    // directly without callargs allocation or IC lookup. ---
    let fast_dispatched = if let Some(sv) = op.s_value.as_deref() {
        match sv {
            // list.append(elem) — 1 extra arg
            "BoundMethod:list:append" if extra_args.len() == 1 => {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_fast_list_append",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local, &[*method_bits, extra_args[0]]);
                Some(builder.inst_results(call)[0])
            }
            // str.join(iterable) — 1 extra arg
            "BoundMethod:str:join" if extra_args.len() == 1 => {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_fast_str_join",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local, &[*method_bits, extra_args[0]]);
                Some(builder.inst_results(call)[0])
            }
            // dict.get(key, default) — 2 extra args
            "BoundMethod:dict:get" if extra_args.len() == 2 => {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_fast_dict_get",
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local = module.declare_func_in_func(callee, builder.func);
                let call = builder
                    .ins()
                    .call(local, &[*method_bits, extra_args[0], extra_args[1]]);
                Some(builder.inst_results(call)[0])
            }
            // str.startswith(prefix) — 1 extra arg
            "BoundMethod:str:startswith" if extra_args.len() == 1 => {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_fast_str_startswith",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local, &[*method_bits, extra_args[0]]);
                Some(builder.inst_results(call)[0])
            }
            // str.upper() — 0 extra args
            "BoundMethod:str:upper" if extra_args.is_empty() => {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_fast_str_upper",
                    &[types::I64],
                    &[types::I64],
                );
                let local = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local, &[*method_bits]);
                Some(builder.inst_results(call)[0])
            }
            // str.lower() — 0 extra args
            "BoundMethod:str:lower" if extra_args.is_empty() => {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_fast_str_lower",
                    &[types::I64],
                    &[types::I64],
                );
                let local = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local, &[*method_bits]);
                Some(builder.inst_results(call)[0])
            }
            // str.strip() — 0 extra args (no-arg form)
            "BoundMethod:str:strip" if extra_args.is_empty() => {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_fast_str_strip",
                    &[types::I64],
                    &[types::I64],
                );
                let local = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local, &[*method_bits]);
                Some(builder.inst_results(call)[0])
            }
            _ => None,
        }
    } else {
        None
    };

    let res = if let Some(fast_res) = fast_dispatched {
        fast_res
    } else {
        // Generic path: allocate callargs and dispatch via IC.
        let callargs_new_local = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_callargs_new",
            &[types::I64, types::I64],
            &[types::I64],
        );
        let pos_capacity = builder.ins().iconst(types::I64, extra_args.len() as i64);
        let kw_capacity = builder.ins().iconst(types::I64, 0);
        let callargs_call = builder
            .ins()
            .call(callargs_new_local, &[pos_capacity, kw_capacity]);
        let callargs_ptr = builder.inst_results(callargs_call)[0];
        let callargs_push_local = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_callargs_push_pos",
            &[types::I64, types::I64],
            &[types::I64],
        );
        for arg in &extra_args {
            builder
                .ins()
                .call(callargs_push_local, &[callargs_ptr, *arg]);
        }
        let call_bind_local = import_func_ref(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            "molt_call_bind_ic",
            &[types::I64, types::I64, types::I64],
            &[types::I64],
        );
        let site_bits = builder.ins().iconst(
            types::I64,
            box_int(stable_ic_site_id(func_name, op_idx, "call_method")),
        );
        let call = builder
            .ins()
            .call(call_bind_local, &[site_bits, *method_bits, callargs_ptr]);
        builder.inst_results(call)[0]
    };
    if let Some(out__) = op.out.as_ref() {
        def_var_from_boxed_transport(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            vars,
            representation_plan,
            nbc,
            out__,
            res,
        );
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_getargv_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    let callee = SimpleBackend::import_func_id_split(
        &mut *module,
        &mut *import_ids,
        "molt_getargv",
        &[],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &[]);
    let res = builder.inst_results(call)[0];
    if let Some(out__) = op.out.as_ref() {
        def_var_named(&mut *builder, vars, out__, res);
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_getframe_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
    let var_get_boxed_overflow_safe = |module: &mut ObjectModule,
                                       import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut BTreeMap<&'static str, FuncRef>,
                                       sealed_blocks: &mut BTreeSet<Block>,
                                       vars: &BTreeMap<String, Variable>,
                                       name: &str,
                                       representation_plan: &ScalarRepresentationPlan|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            representation_plan,
            nbc,
        )
    };
    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let depth = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        &mut *import_refs,
        &mut *sealed_blocks,
        vars,
        &args[0],
        representation_plan,
    )
    .expect("depth not found");
    let callee = SimpleBackend::import_func_id_split(
        &mut *module,
        &mut *import_ids,
        "molt_getframe",
        &[types::I64],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &[*depth]);
    let res = builder.inst_results(call)[0];
    if let Some(out__) = op.out.as_ref() {
        def_var_named(&mut *builder, vars, out__, res);
    }
}

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
fn handle_sys_executable_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
) {
    let callee = SimpleBackend::import_func_id_split(
        &mut *module,
        &mut *import_ids,
        "molt_sys_executable",
        &[],
        &[types::I64],
    );
    let local_callee = module.declare_func_in_func(callee, builder.func);
    let call = builder.ins().call(local_callee, &[]);
    let res = builder.inst_results(call)[0];
    if let Some(out__) = op.out.as_ref() {
        def_var_named(&mut *builder, vars, out__, res);
    }
}
