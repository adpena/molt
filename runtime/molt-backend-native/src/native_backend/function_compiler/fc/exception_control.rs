use super::super::*;

/// Single-source kind authority for [`handle_exception_control_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["raise", "check_exception", "try_start", "try_end"];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for runtime exception control: `raise` and
/// `check_exception`. This owns exception-pending branching, dead tracked-value
/// draining at exception boundaries, fallthrough block sealing, and propagation
/// of remaining cleanup roots to the non-exception path.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_exception_control_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    entry_block: Block,
    loop_depth: i32,
    label_blocks: &BTreeMap<i64, Block>,
    reachable_blocks: &mut BTreeSet<Block>,
    is_block_filled: &mut bool,
    rc_authority: NativeRcAuthority,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    already_decrefed: &mut BTreeSet<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    local_dec_ref_obj: FuncRef,
    local_exc_pending_fast: FuncRef,
    exc_flag_ptr_slot: Option<cranelift_codegen::ir::StackSlot>,
    maybe_debug_seal: &dyn Fn(&str, usize, Block),
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

    match op.kind.as_str() {
        "try_start" | "try_end" => {
            // Native exception CFG and handler labels are constructed before
            // codegen from the SimpleIR label map. These ops remain in the
            // stream as region metadata, so route them explicitly instead of
            // letting no-result ops disappear through the catch-all.
        }
        "raise" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let exc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Exception not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_raise",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*exc]);
            let res = builder.inst_results(call)[0];
            if let Some(out) = op.out.as_ref()
                && out != "none"
            {
                def_var_named(&mut *builder, vars, out.clone(), res);
            }
        }
        "check_exception" => {
            let target_id = op.value.unwrap_or_else(|| {
                panic!(
                    "check_exception missing target label id in function `{}` op {}",
                    func_name, op_idx
                )
            });
            if std::env::var("MOLT_DEBUG_CHECK_EXC").is_ok() {
                eprintln!(
                    "[CHECK_EXC] func={} op={} target_id={} found_in_label_blocks={}",
                    func_name,
                    op_idx,
                    target_id,
                    label_blocks.contains_key(&target_id)
                );
            }
            let Some(&target_block) = label_blocks.get(&target_id) else {
                panic!(
                    "check_exception target label {target_id} is not present in native \
                             label map for function `{}` op {}",
                    func_name, op_idx
                );
            };
            let mut carry_obj: Vec<String> = Vec::new();
            let mut carry_ptr: Vec<String> = Vec::new();
            // `check_exception` terminates the current block (brif) to either jump to the
            // exception handler label or continue on the fallthrough path. That means any
            // temporaries tracked on the current block would otherwise have no natural
            // "line"/control-flow cleanup point until much later. Drain dead values here so
            // short-lived temporaries (for example list indexing results) are decref'd
            // deterministically and do not leak across exception checks.
            if let Some(block) = builder.current_block() {
                if let Some(names) = block_tracked_obj.remove(&block) {
                    carry_obj.extend(names);
                }
                if let Some(names) = block_tracked_ptr.remove(&block) {
                    carry_ptr.extend(names);
                }
                if block == entry_block && loop_depth == 0 {
                    carry_obj.append(tracked_obj_vars);
                    carry_ptr.append(tracked_vars);
                    tracked_obj_vars_set.clear();
                    tracked_vars_set.clear();
                }
                if std::env::var("MOLT_DEBUG_CHECK_EXCEPTION").as_deref() == Ok("1")
                    && std::env::var("MOLT_DEBUG_FUNC_FILTER")
                        .ok()
                        .is_none_or(|f| func_name.contains(&f))
                {
                    eprintln!("check_exception {} op={}", func_name, op_idx,);
                }
            }
            let mut scrubbed_names: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            if !carry_obj.is_empty() {
                let cleanup = drain_cleanup_tracked_dedup_with_authority(
                    rc_authority,
                    &mut carry_obj,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                if std::env::var("MOLT_DEBUG_TRACKED_CLEANUP").as_deref() == Ok("1")
                    && std::env::var("MOLT_DEBUG_FUNC_FILTER")
                        .ok()
                        .is_none_or(|f| func_name.contains(&f))
                    && std::env::var("MOLT_DEBUG_OP_INDEX")
                        .ok()
                        .and_then(|s| s.parse::<usize>().ok())
                        .is_none_or(|target| target == op_idx)
                {
                    let _ = crate::debug_artifacts::append_debug_artifact(
                        "native/tracked_cleanup_debug.txt",
                        format!(
                            "func={} op_idx={} kind={} cleanup_obj={:?}\n",
                            func_name, op_idx, op.kind, cleanup,
                        ),
                    );
                }
                for name in cleanup {
                    let val = entry_vars.get(&name).copied().or_else(|| {
                        var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            &mut *sealed_blocks,
                            vars,
                            &name,
                            representation_plan,
                        )
                        .map(|v| *v)
                    });
                    let Some(val) = val else {
                        continue;
                    };
                    builder.ins().call(local_dec_ref_obj, &[val]);
                    entry_vars.remove(&name);
                    if let Some(var) = vars.get(&name) {
                        let scrub =
                            dead_scrub_value_for_var(&mut *builder, representation_plan, &name);
                        builder.def_var(*var, scrub);
                    }
                    scrubbed_names.insert(name);
                }
            }
            if !carry_ptr.is_empty() {
                let cleanup = drain_cleanup_tracked_dedup_with_authority(
                    rc_authority,
                    &mut carry_ptr,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                if std::env::var("MOLT_DEBUG_TRACKED_CLEANUP").as_deref() == Ok("1")
                    && std::env::var("MOLT_DEBUG_FUNC_FILTER")
                        .ok()
                        .is_none_or(|f| func_name.contains(&f))
                    && std::env::var("MOLT_DEBUG_OP_INDEX")
                        .ok()
                        .and_then(|s| s.parse::<usize>().ok())
                        .is_none_or(|target| target == op_idx)
                {
                    let _ = crate::debug_artifacts::append_debug_artifact(
                        "native/tracked_cleanup_debug.txt",
                        format!(
                            "func={} op_idx={} kind={} cleanup_ptr={:?}\n",
                            func_name, op_idx, op.kind, cleanup,
                        ),
                    );
                }
                for name in cleanup {
                    let val = entry_vars.get(&name).copied().or_else(|| {
                        var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            &mut *sealed_blocks,
                            vars,
                            &name,
                            representation_plan,
                        )
                        .map(|v| *v)
                    });
                    let Some(val) = val else {
                        continue;
                    };
                    builder.ins().call(local_dec_ref_obj, &[val]);
                    entry_vars.remove(&name);
                    if let Some(var) = vars.get(&name) {
                        let scrub =
                            dead_scrub_value_for_var(&mut *builder, representation_plan, &name);
                        builder.def_var(*var, scrub);
                    }
                    scrubbed_names.insert(name);
                }
            }
            // Single pass over all exception handler blocks to remove
            // scrubbed names, instead of one retain per name per block.
            if !scrubbed_names.is_empty() {
                for tracked_list in block_tracked_obj.values_mut() {
                    tracked_list.retain(|n| !scrubbed_names.contains(n));
                }
                for tracked_list in block_tracked_ptr.values_mut() {
                    tracked_list.retain(|n| !scrubbed_names.contains(n));
                }
            }
            let fallthrough = builder.create_block();
            reachable_blocks.insert(target_block);
            reachable_blocks.insert(fallthrough);
            let cond = emit_exception_pending_condition(
                &mut *builder,
                local_exc_pending_fast,
                exc_flag_ptr_slot,
            );
            brif_block(&mut *builder, cond, target_block, &[], fallthrough, &[]);
            // The fallthrough block is always fresh and has its only
            // predecessor emitted here. Seal it immediately so later
            // `use_var` calls in the fallthrough block cannot
            // synthesize placeholder predecessors with zero-valued
            // block params. This remains true in exception-bearing
            // functions; exception labels are separate target blocks.
            maybe_debug_seal("check_exception_fallthrough", op_idx, fallthrough);
            seal_block_once(&mut *builder, &mut *sealed_blocks, fallthrough);
            switch_to_block_with_rebind(&mut *builder, fallthrough, &mut *is_block_filled, true);
            // check_exception's fallthrough is always a fresh empty
            // block — force-clear is_block_filled so subsequent ops
            // (add, loop_index_next) are never incorrectly skipped by
            // the whitelist guard.
            *is_block_filled = false;
            // Propagate remaining tracked objects to BOTH the fallthrough
            // and the exception handler. Without this, the exception handler
            // may access objects that were only passed to the fallthrough,
            // causing use-after-free when the exception handler dec-refs them.
            // Propagate tracked values ONLY to the fallthrough block.
            // Exception labels now get the SSA names they need via
            // `exception_label_rebind_names`, but tracked-value cloning
            // to both sides reintroduces refcount multiplication in import
            // heavy code. Keep handler rooting separate from tracked
            // cleanup transport.
            if !carry_obj.is_empty() {
                block_tracked_obj
                    .entry(fallthrough)
                    .or_default()
                    .extend(carry_obj.iter().cloned());
            }
            if !carry_ptr.is_empty() {
                block_tracked_ptr
                    .entry(fallthrough)
                    .or_default()
                    .extend(carry_ptr.iter().cloned());
            }
        }
        _ => unreachable!("non-exception-control op routed to handle_exception_control_op"),
    }
}
