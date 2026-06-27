use super::super::*;

/// Single-source kind authority for [`handle_ret_jump_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "ret",
    "ret_void",
    "jump",
    "br_if",
    "label",
    "state_label",
    "phi",
    "store_var",
    "delete_var",
    "load_var",
    "copy_var",
    "load_param",
];
use super::OpFlow;
use super::list_index_fast_path::ListIndexFastPathState;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for return, jump/branch, label, phi, and
/// TIR variable transfer ops.
///
/// Extracted from `compile_func_inner`'s per-op dispatch (M1.8). Backend
/// state is threaded explicitly, and original outer op-loop `continue` exits
/// are represented as `OpFlow::Continue` so the parent epilogue is skipped
/// exactly where the inline arms skipped it.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_ret_jump_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    func_ops: &[OpIR],
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    param_name_set: &BTreeSet<&str>,
    alias_roots: &BTreeMap<String, String>,
    last_use: &BTreeMap<String, usize>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    already_decrefed: &mut BTreeSet<String>,
    reachable_blocks: &mut BTreeSet<Block>,
    label_blocks: &BTreeMap<i64, Block>,
    label_join_slots: &BTreeMap<i64, Vec<String>>,
    function_exception_label_id: Option<i64>,
    slot_backed_join_slots: &BTreeMap<String, cranelift_codegen::ir::StackSlot>,
    raw_backed_slot_names: &BTreeSet<String>,
    list_index_fast_paths: &ListIndexFastPathState,
    master_return_block: Block,
    is_block_filled: &mut bool,
    returns_value: bool,
    rc_authority: NativeRcAuthority,
    scalar_fast_paths_enabled: bool,
    debug_block_origins: Option<&str>,
    maybe_debug_seal: &dyn Fn(&str, usize, Block),
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
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
        "ret" => {
            if !rc_authority.native_value_tracking_enabled() {
                block_tracked_obj.clear();
                block_tracked_ptr.clear();
                tracked_vars.clear();
                tracked_obj_vars.clear();
                tracked_vars_set.clear();
                tracked_obj_vars_set.clear();
                entry_vars.clear();
            }
            if std::env::var("MOLT_DEBUG_RET_CLEANUP").as_deref() == Ok("1")
                && std::env::var("MOLT_DEBUG_FUNC_FILTER")
                    .ok()
                    .is_none_or(|f| func_name.contains(&f))
            {
                eprintln!(
                    "debug ret cleanup func={} op_idx={} ret_var={:?} tracked_obj_vars_len={} tracked_vars_len={}",
                    func_name,
                    op_idx,
                    op.var.as_deref(),
                    tracked_obj_vars.len(),
                    tracked_vars.len(),
                );
                if !tracked_obj_vars.is_empty() {
                    eprintln!("debug ret cleanup tracked_obj_vars={:?}", tracked_obj_vars);
                }
                if !tracked_vars.is_empty() {
                    eprintln!("debug ret cleanup tracked_vars={:?}", tracked_vars);
                }
            }
            let Some(var_name) = op.var.as_ref() else {
                if let Some(block) = builder.current_block() {
                    // Function return: fully drain per-block tracked values.
                    if let Some(names) = block_tracked_obj.remove(&block) {
                        for name in names {
                            if cleanup_name_excluded(
                                &name,
                                None,
                                param_name_set,
                                representation_plan,
                            ) || !mark_cleanup_root_once(
                                alias_roots,
                                &mut *already_decrefed,
                                &name,
                            ) {
                                continue;
                            }
                            let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "Tracked obj var not found in {} op {}: {}",
                                        func_name, op_idx, name
                                    )
                                });
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                    }
                    if let Some(names) = block_tracked_ptr.remove(&block) {
                        for name in names {
                            if cleanup_name_excluded(
                                &name,
                                None,
                                param_name_set,
                                representation_plan,
                            ) || !mark_cleanup_root_once(
                                alias_roots,
                                &mut *already_decrefed,
                                &name,
                            ) {
                                continue;
                            }
                            let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "Tracked ptr var not found in {} op {}: {}",
                                        func_name, op_idx, name
                                    )
                                });
                            builder.ins().call(local_dec_ref_obj, &[val]);
                        }
                    }
                }
                for name in tracked_vars.iter() {
                    if cleanup_name_excluded(name, None, param_name_set, representation_plan) {
                        continue;
                    }
                    if let Some(val) = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        name,
                        representation_plan,
                    ) && mark_cleanup_root_once(alias_roots, &mut *already_decrefed, name)
                    {
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                }
                for name in tracked_obj_vars.iter() {
                    if cleanup_name_excluded(name, None, param_name_set, representation_plan) {
                        continue;
                    }
                    if let Some(val) = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        name,
                        representation_plan,
                    ) && mark_cleanup_root_once(alias_roots, &mut *already_decrefed, name)
                    {
                        builder.ins().call(local_dec_ref_obj, &[*val]);
                    }
                }
                reachable_blocks.insert(master_return_block);
                if returns_value {
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut *builder, master_return_block, &[none_bits]);
                } else {
                    jump_block(&mut *builder, master_return_block, &[]);
                }
                *is_block_filled = true;
                return OpFlow::Continue;
            };
            // Deferred primitive boxing at function return.
            let ret_val = ensure_boxed_primitive_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                nbc,
                representation_plan,
                var_name,
            );
            let ret_root = alias_roots
                .get(var_name)
                .cloned()
                .unwrap_or_else(|| var_name.clone());
            let mut protected_return_aliases: BTreeSet<String> = BTreeSet::from([var_name.clone()]);
            for (name, root) in alias_roots {
                if root == &ret_root {
                    protected_return_aliases.insert(name.clone());
                }
            }
            if let Some(block) = builder.current_block() {
                // Function return: fully drain per-block tracked values (except return).
                if let Some(names) = block_tracked_obj.remove(&block) {
                    for name in names {
                        if cleanup_name_excluded(
                            &name,
                            Some(&protected_return_aliases),
                            param_name_set,
                            representation_plan,
                        ) || !mark_cleanup_root_once(alias_roots, &mut *already_decrefed, &name)
                        {
                            continue;
                        }
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
                    }
                }
                if let Some(names) = block_tracked_ptr.remove(&block) {
                    for name in names {
                        if cleanup_name_excluded(
                            &name,
                            Some(&protected_return_aliases),
                            param_name_set,
                            representation_plan,
                        ) || !mark_cleanup_root_once(alias_roots, &mut *already_decrefed, &name)
                        {
                            continue;
                        }
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
                    }
                }
            }
            tracked_vars.retain(|v| !protected_return_aliases.contains(v));
            tracked_obj_vars.retain(|v| !protected_return_aliases.contains(v));
            for protected in &protected_return_aliases {
                tracked_vars_set.remove(protected);
                tracked_obj_vars_set.remove(protected);
            }
            for name in tracked_vars.iter() {
                if cleanup_name_excluded(
                    name,
                    Some(&protected_return_aliases),
                    param_name_set,
                    representation_plan,
                ) {
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
                if let Some(val) = val
                    && mark_cleanup_root_once(alias_roots, &mut *already_decrefed, name)
                {
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
            }
            for name in tracked_obj_vars.iter() {
                if cleanup_name_excluded(
                    name,
                    Some(&protected_return_aliases),
                    param_name_set,
                    representation_plan,
                ) {
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
                if let Some(val) = val
                    && mark_cleanup_root_once(alias_roots, &mut *already_decrefed, name)
                {
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
            }
            reachable_blocks.insert(master_return_block);
            if returns_value {
                jump_block(&mut *builder, master_return_block, &[ret_val]);
            } else {
                jump_block(&mut *builder, master_return_block, &[]);
            }
            *is_block_filled = true;
        }
        "ret_void" => {
            if !rc_authority.native_value_tracking_enabled() {
                block_tracked_obj.clear();
                block_tracked_ptr.clear();
                tracked_vars.clear();
                tracked_obj_vars.clear();
                tracked_vars_set.clear();
                tracked_obj_vars_set.clear();
                entry_vars.clear();
            }
            if let Some(block) = builder.current_block() {
                // Function return: fully drain per-block tracked values.
                if let Some(names) = block_tracked_obj.remove(&block) {
                    for name in names {
                        if cleanup_name_excluded(&name, None, param_name_set, representation_plan)
                            || !mark_cleanup_root_once(alias_roots, &mut *already_decrefed, &name)
                        {
                            continue;
                        }
                        let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                            .unwrap_or_else(|| {
                                panic!(
                                    "Tracked obj var not found in {} op {}: {}",
                                    func_name, op_idx, name
                                )
                            });
                        builder.ins().call(local_dec_ref_obj, &[val]);
                    }
                }
                if let Some(names) = block_tracked_ptr.remove(&block) {
                    for name in names {
                        if cleanup_name_excluded(&name, None, param_name_set, representation_plan)
                            || !mark_cleanup_root_once(alias_roots, &mut *already_decrefed, &name)
                        {
                            continue;
                        }
                        let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                            .unwrap_or_else(|| {
                                panic!(
                                    "Tracked ptr var not found in {} op {}: {}",
                                    func_name, op_idx, name
                                )
                            });
                        builder.ins().call(local_dec_ref_obj, &[val]);
                    }
                }
            }
            for name in tracked_vars.iter() {
                if cleanup_name_excluded(name, None, param_name_set, representation_plan) {
                    continue;
                }
                if let Some(val) = entry_vars.get(name)
                    && mark_cleanup_root_once(alias_roots, &mut *already_decrefed, name)
                {
                    builder.ins().call(local_dec_ref_obj, &[*val]);
                }
            }
            for name in tracked_obj_vars.iter() {
                if cleanup_name_excluded(name, None, param_name_set, representation_plan) {
                    continue;
                }
                if let Some(val) = entry_vars.get(name)
                    && mark_cleanup_root_once(alias_roots, &mut *already_decrefed, name)
                {
                    builder.ins().call(local_dec_ref_obj, &[*val]);
                }
            }
            reachable_blocks.insert(master_return_block);
            if returns_value {
                let none_bits = builder.ins().iconst(types::I64, box_none());
                jump_block(&mut *builder, master_return_block, &[none_bits]);
            } else {
                jump_block(&mut *builder, master_return_block, &[]);
            }
            *is_block_filled = true;
        }
        "jump" => {
            let target_id = op.value.unwrap_or(0);
            let target_block = label_blocks[&target_id];
            if let Some(block) = builder.current_block() {
                let mut carry_obj = block_tracked_obj.remove(&block).unwrap_or_default();
                let cleanup = drain_cleanup_tracked_dedup_with_authority(
                    rc_authority,
                    &mut carry_obj,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                for name in cleanup {
                    // Use entry_vars (definition-time Value) for dec_ref,
                    // not var_get (current SSA Value). If the variable was
                    // redefined, var_get returns the WRONG object.
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
                }
                if !carry_obj.is_empty() {
                    extend_unique_tracked(
                        block_tracked_obj.entry(target_block).or_default(),
                        carry_obj,
                    );
                }

                let mut carry_ptr = block_tracked_ptr.remove(&block).unwrap_or_default();
                let cleanup = drain_cleanup_tracked_dedup_with_authority(
                    rc_authority,
                    &mut carry_ptr,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
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
                }
                if !carry_ptr.is_empty() {
                    extend_unique_tracked(
                        block_tracked_ptr.entry(target_block).or_default(),
                        carry_ptr,
                    );
                }
            }
            reachable_blocks.insert(target_block);
            jump_block(&mut *builder, target_block, &[]);
            *is_block_filled = true;
        }
        "br_if" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let target_id = op.value.unwrap_or(0);
            let target_block = label_blocks[&target_id];
            let origin_block = builder
                .current_block()
                .expect("br_if requires an active block");

            let fallthrough_block = builder.create_block();
            if debug_block_origins.is_some() {
                eprintln!(
                    "BLOCK_ORIGIN {} op{} br_if target_label={} target_block={:?} fallthrough={:?}",
                    func_name, op_idx, target_id, target_block, fallthrough_block
                );
            }
            // cond is NaN-boxed unless representation facts prove a raw
            // bool-primary value; dispatch from representation_plan to avoid
            // unnecessary GIL-wrapped molt_is_truthy calls.
            let cond_name = &args[0];
            let cond_bool = if let Some(raw_val) =
                bool_raw_value(&mut *builder, vars, representation_plan, cond_name)
            {
                // Raw bool from proven list_bool getitem or const_bool.
                // Branch directly on raw 0/1 — ZERO NaN-box overhead.
                builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0)
            } else if scalar_fast_paths_enabled
                && representation_plan.name_is_bool_scalar(cond_name)
            {
                // NaN-boxed bool: bit 0 is the boolean value.
                let cond = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                )
                .expect("Cond not found");
                let one = builder.ins().iconst(types::I64, 1);
                let bit0 = builder.ins().band(*cond, one);
                builder.ins().icmp_imm(IntCC::NotEqual, bit0, 0)
            } else if let Some(raw_shadow) =
                int_raw_value(&mut *builder, vars, representation_plan, &args[0])
            {
                // Proven raw i64 carrier: truthiness is `value != 0`.
                builder.ins().icmp_imm(IntCC::NotEqual, raw_shadow, 0)
            } else if scalar_fast_paths_enabled
                && representation_plan.name_is_integer_scalar(cond_name)
            {
                // `var_is_int` only proves Python-`int` type, which includes
                // heap BigInts (TAG_PTR). The trusted unbox would truncate a
                // BigInt pointer (e.g. `1 << 47` has low 47 bits zero and
                // would be wrongly falsy). Guard on a runtime inline-int tag
                // check: inline TAG_INT/TAG_BOOL use `unbox != 0`; any heap
                // int (BigInt) is non-zero by construction, hence truthy.
                let cond = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                )
                .expect("Cond not found");
                let cond_val = unbox_int_or_bool(&mut *builder, *cond, nbc);
                let is_inline_int = fused_is_int_or_bool(&mut *builder, *cond, nbc);
                let inline_truthy = builder.ins().icmp_imm(IntCC::NotEqual, cond_val, 0);
                let true_val = builder.ins().iconst(types::I8, 1);
                builder.ins().select(is_inline_int, inline_truthy, true_val)
            } else {
                let cond = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                )
                .expect("Cond not found");
                // Speculative inline truthiness: check NaN-box tag
                // to avoid molt_is_truthy function call for bool/int.
                // NaN-boxed False is 0x7ffa000000000000 (nonzero),
                // so a raw icmp_imm(!=0) always evaluates true — we need
                // the runtime to decode the type tag.
                let brif_truthy_merge = builder.create_block();
                builder.append_block_param(brif_truthy_merge, types::I8);

                // Conditional list-bool carrier: when the source list
                // is list_bool, branch directly on the raw 0/1 payload;
                // otherwise continue into the normal NaN-box path.
                emit_conditional_list_bool_truthiness(
                    &mut *builder,
                    &mut *sealed_blocks,
                    &list_index_fast_paths.list_is_bool_cache,
                    list_index_fast_paths
                        .conditional_list_bool_shadows
                        .get(cond_name),
                    brif_truthy_merge,
                    &[],
                );

                let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
                let masked = builder.ins().band(*cond, mask);

                let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
                let is_bool = builder.ins().icmp(IntCC::Equal, masked, bool_tag);
                let brif_bool_block = builder.create_block();
                let brif_not_bool_block = builder.create_block();
                builder
                    .ins()
                    .brif(is_bool, brif_bool_block, &[], brif_not_bool_block, &[]);

                switch_to_block_materialized(&mut *builder, brif_bool_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, brif_bool_block);
                let bit0 = builder.ins().band_imm(*cond, 1);
                let bool_truthy = builder.ins().icmp_imm(IntCC::NotEqual, bit0, 0);
                jump_block(&mut *builder, brif_truthy_merge, &[bool_truthy]);

                switch_to_block_materialized(&mut *builder, brif_not_bool_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, brif_not_bool_block);
                let int_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
                let is_int = builder.ins().icmp(IntCC::Equal, masked, int_tag);
                let brif_int_block = builder.create_block();
                let brif_call_block = builder.create_block();
                builder.set_cold_block(brif_call_block);
                builder
                    .ins()
                    .brif(is_int, brif_int_block, &[], brif_call_block, &[]);

                switch_to_block_materialized(&mut *builder, brif_int_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, brif_int_block);
                let raw_val = unbox_int(&mut *builder, *cond, nbc);
                let int_truthy = builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0);
                jump_block(&mut *builder, brif_truthy_merge, &[int_truthy]);

                switch_to_block_materialized(&mut *builder, brif_call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, brif_call_block);
                let truthy_fn = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_is_truthy",
                    &[types::I64],
                    &[types::I64],
                );
                let truthy_ref = module.declare_func_in_func(truthy_fn, builder.func);
                let truthy_call = builder.ins().call(truthy_ref, &[*cond]);
                let truthy_val = builder.inst_results(truthy_call)[0];
                let call_truthy = builder.ins().icmp_imm(IntCC::NotEqual, truthy_val, 0);
                jump_block(&mut *builder, brif_truthy_merge, &[call_truthy]);

                switch_to_block_materialized(&mut *builder, brif_truthy_merge);
                seal_block_once(&mut *builder, &mut *sealed_blocks, brif_truthy_merge);
                builder.block_params(brif_truthy_merge)[0]
            };

            reachable_blocks.insert(target_block);
            reachable_blocks.insert(fallthrough_block);
            // br_if terminates the current block and can transfer control to either
            // successor. Carry all live tracked values into both.
            let mut carry_obj = block_tracked_obj.remove(&origin_block).unwrap_or_default();
            let cleanup = drain_cleanup_tracked_dedup_with_authority(
                rc_authority,
                &mut carry_obj,
                last_use,
                alias_roots,
                op_idx,
                None,
                Some(&mut *already_decrefed),
            );
            for name in cleanup {
                let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                    .unwrap_or_else(|| {
                        panic!(
                            "Tracked obj var not found in {} op {}: {}",
                            func_name, op_idx, name
                        )
                    });
                builder.ins().call(local_dec_ref_obj, &[val]);
            }
            if !carry_obj.is_empty() {
                extend_unique_tracked(
                    block_tracked_obj.entry(target_block).or_default(),
                    carry_obj.clone(),
                );
                extend_unique_tracked(
                    block_tracked_obj.entry(fallthrough_block).or_default(),
                    carry_obj.clone(),
                );
            }
            let mut carry_ptr = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
            let cleanup = drain_cleanup_tracked_dedup_with_authority(
                rc_authority,
                &mut carry_ptr,
                last_use,
                alias_roots,
                op_idx,
                None,
                Some(&mut *already_decrefed),
            );
            for name in cleanup {
                let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                    .unwrap_or_else(|| {
                        panic!(
                            "Tracked ptr var not found in {} op {}: {}",
                            func_name, op_idx, name
                        )
                    });
                builder.ins().call(local_dec_ref_obj, &[val]);
            }
            if !carry_ptr.is_empty() {
                extend_unique_tracked(
                    block_tracked_ptr.entry(target_block).or_default(),
                    carry_ptr.clone(),
                );
                extend_unique_tracked(
                    block_tracked_ptr.entry(fallthrough_block).or_default(),
                    carry_ptr.clone(),
                );
            }
            builder
                .ins()
                .brif(cond_bool, target_block, &[], fallthrough_block, &[]);
            switch_to_block_with_rebind(
                &mut *builder,
                fallthrough_block,
                &mut *is_block_filled,
                false,
            );
            maybe_debug_seal("br_if_fallthrough", op_idx, fallthrough_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, fallthrough_block);
        }
        "label" | "state_label" => {
            let label_id = op.value.unwrap_or(0);
            let block = label_blocks[&label_id];
            let is_function_exception_label = Some(label_id) == function_exception_label_id;
            let label_live_join_vars: BTreeMap<String, Variable> = label_join_slots
                .get(&label_id)
                .into_iter()
                .flat_map(|names| names.iter())
                .filter(|name| !slot_backed_join_slots.contains_key(name.as_str()))
                .filter_map(|name| vars.get(name).copied().map(|var| (name.clone(), var)))
                .collect();
            let rebind_label_join_state = |builder: &mut FunctionBuilder| {
                if builder.block_params(block).is_empty() && !is_function_exception_label {
                    return;
                }
                for var in label_live_join_vars.values() {
                    let value = builder.use_var(*var);
                    builder.def_var(*var, value);
                }
            };

            // Prevent normal fallthrough into the function-level exception handler.
            if is_function_exception_label && !*is_block_filled {
                reachable_blocks.insert(master_return_block);
                if returns_value {
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    jump_block(&mut *builder, master_return_block, &[none_bits]);
                } else {
                    jump_block(&mut *builder, master_return_block, &[]);
                }
                *is_block_filled = true;
            }

            if is_function_exception_label {
                // Exception handlers are cold — move them out of the
                // hot execution path for better i-cache/branch behavior.
                builder.set_cold_block(block);
                reachable_blocks.insert(block);
                materialize_label_block(&mut *builder, block, &mut *is_block_filled);
                if !*is_block_filled {
                    rebind_label_join_state(&mut *builder);
                }
                if std::env::var("MOLT_DEBUG_LABEL_BINDINGS").as_deref() == Ok(func_name) {
                    eprintln!(
                        "LABEL_BIND {} label={} block={:?} params={:?}",
                        func_name,
                        label_id,
                        block,
                        builder.block_params(block)
                    );
                }
            } else {
                reachable_blocks.insert(block);
                // Textual label sites define CFG ownership. Materialize
                // the block even when no already-emitted predecessor
                // has reached it yet; later backedges / deferred
                // branches may still target it.
                materialize_label_block(&mut *builder, block, &mut *is_block_filled);
                if !*is_block_filled {
                    rebind_label_join_state(&mut *builder);
                }
                if std::env::var("MOLT_DEBUG_LABEL_BINDINGS").as_deref() == Ok(func_name) {
                    eprintln!(
                        "LABEL_BIND {} label={} block={:?} params={:?}",
                        func_name,
                        label_id,
                        block,
                        builder.block_params(block)
                    );
                }
            }
        }
        "phi" => {
            // Phi ops are rewritten to store_var/load_var by
            // rewrite_phi_to_store_load() before compilation.
            // Any residual phi is a no-op (handled by end_if
            // for the non-TIR structured path).
        }
        // TIR round-trip variable ops — wire SSA values between blocks
        "store_var" => {
            // Store a value into a named variable.
            //
            // Fast path: when the source is raw-primary int and the
            // destination is proven-int, copy the raw i64 directly
            // with NO boxing and NO refcount ops.  Raw i64 values
            // are stack values, not heap pointers — refcounting them
            // is both incorrect and wasteful.  Overflow is handled
            // at escape points (function return, call args, heap
            // stores) via ensure_boxed_overflow_safe.
            //
            // Generic path: for variables inside back-edge loops
            // (TIR-generated label/jump/br_if control flow), we emit:
            //   inc_ref_obj(new)
            //   def_var(name, new)
            //
            // For non-loop store_var, drain_cleanup_tracked handles
            // the final dec_ref at the ret point.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let var_name = op.var.as_deref().or(op.out.as_deref());
            if let Some(name) = var_name {
                // --- Raw-primary int fast path ---
                // When source is raw-primary (its Variable holds unboxed i64)
                // AND destination is proven-int, transfer the raw i64 directly.
                // This eliminates box+unbox round-trips in tight loops like
                // `total += i; i += 1` where both sides are proven-int.
                if representation_plan.is_raw_int_carrier_name(&args[0])
                    && scalar_fast_paths_enabled
                    && representation_plan.is_raw_int_carrier_name(name)
                    && !slot_backed_join_slots.contains_key(name)
                {
                    // Read raw i64 from source Variable (no boxing).
                    let raw_val =
                        { int_raw_value(&mut *builder, vars, representation_plan, &args[0]) }
                            .unwrap_or_else(|| {
                                // Source is raw-primary but has no shadow entry yet.
                                // Read directly from the main Variable (which holds raw i64).
                                let var = *vars
                                    .get(&args[0])
                                    .expect("store_var: raw src var not found");
                                builder.use_var(var)
                            });
                    // Phase 1c: representation_plan join slots write raw
                    // i64 directly to the main Variable. The
                    // loop_start demote is taught to skip them, so
                    // both the entry preheader and the back edge
                    // pass raw i64 to the loop header phi —
                    // consistent representation, no per-iteration
                    // box→unbox round trip.
                    //
                    // Boxed join slots still box on the back edge because
                    // their other definition sites may produce NaN-boxed
                    // values (mixed-type stores or generic runtime calls).
                    def_var_named(&mut *builder, vars, name, raw_val);
                    // Propagate shadow to destination (both tiers).
                    // No refcount ops needed -- raw i64 is not a heap pointer.
                    return OpFlow::Continue;
                }
                // --- Raw-primary float fast path ---
                // When destination is a float-primary variable, transfer
                // raw f64 directly with no boxing and no refcount ops.
                // Float values are always stack values, never heap pointers.
                if representation_plan.is_float_unboxed(name)
                    && scalar_fast_paths_enabled
                    && !slot_backed_join_slots.contains_key(name)
                {
                    let raw_f64 =
                        float_value_for(&mut *builder, vars, representation_plan, &args[0])
                            .unwrap_or_else(|| {
                                // Source is NaN-boxed -- extract f64 bits.
                                let boxed = var_get_boxed_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    &mut *sealed_blocks,
                                    vars,
                                    &args[0],
                                    representation_plan,
                                )
                                .expect("store_var: float src not found");
                                builder
                                    .ins()
                                    .bitcast(types::F64, MemFlagsData::new(), *boxed)
                            });
                    def_var_named(&mut *builder, vars, name, raw_f64);
                    // No refcount ops needed -- raw f64 is not a heap pointer.
                    return OpFlow::Continue;
                }
                // --- Raw-primary bool fast path ---
                // Bool-primary store targets keep raw 0/1 in their
                // main Cranelift Variable, including proven join
                // carriers. The static fixpoint only admits targets
                // whose store sources are themselves raw-closed.
                if representation_plan.is_bool_unboxed(name)
                    && scalar_fast_paths_enabled
                    && !slot_backed_join_slots.contains_key(name)
                {
                    let raw_bool =
                        bool_raw_value(&mut *builder, vars, representation_plan, &args[0])
                            .unwrap_or_else(|| {
                                panic!("store_var: bool-primary src missing raw bool: {}", args[0])
                            });
                    def_raw_bool_value(
                        &mut *builder,
                        vars,
                        representation_plan,
                        name,
                        raw_bool,
                        nbc,
                    );
                    // No refcount ops needed -- raw bool is an inline scalar.
                    return OpFlow::Continue;
                }
                // --- Raw-backed join slots ---
                // The slot carries RAW i64 / raw 0-1 bool (no NaN
                // box, no refcount — a raw scalar is never a heap
                // pointer). Checked BEFORE the boxing read below so
                // no dead box blocks are emitted. The carrier chain
                // only admits a name when every store source is
                // raw-closed, so a non-raw source here is a chain
                // inconsistency.
                if raw_backed_slot_names.contains(name)
                    && let Some(&slot) = slot_backed_join_slots.get(name)
                {
                    let raw_val = if representation_plan.is_bool_unboxed(name) {
                        bool_raw_value(&mut *builder, vars, representation_plan, &args[0])
                    } else {
                        int_raw_value(&mut *builder, vars, representation_plan, &args[0])
                    }
                    .unwrap_or_else(|| {
                        panic!(
                            "store_var: raw-backed slot '{name}' fed by non-raw source '{}' (carrier chain inconsistency)",
                            args[0]
                        )
                    });
                    builder.ins().stack_store(raw_val, slot, 0);
                    return OpFlow::Continue;
                }
                // --- Slot-backed join slots ---
                let val = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                )
                .expect("store_var: src not found");
                if let Some(&slot) = slot_backed_join_slots.get(name) {
                    // RC drop-insertion substrate (design 20 §4.1, Phase 5):
                    // this is the memory-phi arm of the native value-tracking
                    // RC — a CPython-`STORE_FAST` retain-new / release-old on
                    // the loop-carried slot. For drop-inserted functions the
                    // TIR drops own this: the TIR `DecRef(old)` (inserted on
                    // the back-edge, right before this store) already releases
                    // the previous occupant, and the new value is produced
                    // OWNED (rc=1) so its single reference transfers into the
                    // slot with a bare store — no inc, no dec. Running the
                    // legacy inc(new)/dec(old) here too would add one
                    // unbalanced reference per iteration (inc not matched by
                    // the TIR drop), re-opening the O(n) loop-accumulator leak
                    // (the string-concat / bigint-accumulator headline case).
                    if !rc_authority.native_value_tracking_enabled() {
                        builder.ins().stack_store(*val, slot, 0);
                        return OpFlow::Continue;
                    }
                    let old = builder.ins().stack_load(types::I64, slot, 0);
                    emit_inc_ref_obj(&mut *builder, *val, local_inc_ref_obj, nbc);
                    builder.ins().stack_store(*val, slot, 0);
                    builder.ins().call(local_dec_ref_obj, &[old]);
                    return OpFlow::Continue;
                }
                // Check if this store_var is inside a back-edge loop.
                // If so, emit inc_ref(new) for correct refcount
                // management of heap-allocated values (bigints).
                // Detect if this store_var is inside a back-edge loop
                // by checking if any jump/br_if in the function targets
                // a label at a position before the jump.
                let in_loop = {
                    let mut found = false;
                    let mut lbl_pos: std::collections::HashMap<i64, usize> =
                        std::collections::HashMap::new();
                    for (i, o) in func_ops.iter().enumerate() {
                        if matches!(o.kind.as_str(), "label" | "state_label")
                            && let Some(id) = o.value
                        {
                            lbl_pos.insert(id, i);
                        }
                    }
                    for (i, o) in func_ops.iter().enumerate() {
                        if matches!(o.kind.as_str(), "jump" | "br_if")
                            && let Some(tid) = o.value
                            && let Some(&tp) = lbl_pos.get(&tid)
                            && tp < i
                            && op_idx >= tp
                            && op_idx <= i
                        {
                            found = true;
                            break;
                        }
                    }
                    found
                };
                let store_uses_boxed_transport = !representation_plan.is_raw_int_carrier_name(name)
                    && !representation_plan.is_bool_unboxed(name)
                    && !representation_plan.is_float_unboxed(name);
                // RC drop-insertion substrate (design 20, R1 guard — inc
                // side): when the TIR drop pass processed this function it
                // already inserted the loop-carried RC ownership transfer
                // (a `DecRef(old)` before the back-edge; the back-edge passes
                // the new value's single owned reference to the header phi).
                // The legacy `inc_ref(new)`-per-iteration path below would
                // then add an unmatched reference per iteration. This is the
                // symmetric twin of the `loop_reassign_old_val` dec-side
                // guard (§4.1) and is NECESSARY for sound activation — but
                // note it is NOT SUFFICIENT on its own: the broader native
                // value-tracking RC (`tracked_obj_vars` registration +
                // `drain_cleanup_tracked_dedup` at exits, retain-at-store /
                // release-at-scope-exit) still negates the TIR `DecRef(old)`
                // on loop-carried accumulators. Closing the residual O(n)
                // leak requires gating that whole system on `drop_inserted`
                // (the Phase-5 native-RC retirement — see the activation note
                // in `pass_manager::build_default_pipeline` and design 20
                // §4.1). The guard here is dormant until the pass is wired.
                if in_loop
                    && store_uses_boxed_transport
                    && rc_authority.native_value_tracking_enabled()
                {
                    // inc_ref the new value so it survives loop iterations.
                    // No dec_ref for old — drain_cleanup_tracked handles
                    // final cleanup at function return (lifetimes extended
                    // by the back-edge detection in preanalysis).
                    let inc_callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_inc_ref_obj",
                        &[types::I64],
                        &[],
                    );
                    let inc_local = module.declare_func_in_func(inc_callee, builder.func);
                    builder.ins().call(inc_local, &[*val]);
                }
                def_var_from_boxed_transport(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    name,
                    *val,
                );
            } else {
                // No destination variable name — still need to evaluate
                // the source for side effects (should not happen in
                // well-formed TIR, but defensive).
                let _val = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                );
            }
        }
        "delete_var" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let Some(name) = op.var.as_deref().or(op.out.as_deref()) else {
                panic!("delete_var missing target local");
            };
            if raw_backed_slot_names.contains(name) {
                panic!(
                    "delete_var target '{name}' was admitted to a raw-backed slot; missing sentinel requires boxed local storage"
                );
            }
            let Some(missing_name) = args.first() else {
                panic!("delete_var missing sentinel operand");
            };
            let Some(old_name) = args.get(1) else {
                panic!("delete_var missing old-slot operand");
            };
            let missing_val = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                missing_name,
                representation_plan,
            )
            .expect("delete_var: missing sentinel not found");
            let old_val = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                old_name,
                representation_plan,
            )
            .expect("delete_var: old local operand not found");
            if let Some(&slot) = slot_backed_join_slots.get(name) {
                builder.ins().stack_store(missing_val, slot, 0);
            } else {
                def_var_from_boxed_transport(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    name,
                    missing_val,
                );
            }
            remove_tracked_name(&mut *tracked_vars, name);
            tracked_vars_set.remove(name);
            remove_tracked_name(&mut *tracked_obj_vars, name);
            tracked_obj_vars_set.remove(name);
            entry_vars.remove(name);
            if let Some(block) = builder.current_block() {
                if let Some(tracked) = block_tracked_ptr.get_mut(&block) {
                    remove_tracked_name(tracked, name);
                }
                if let Some(tracked) = block_tracked_obj.get_mut(&block) {
                    remove_tracked_name(tracked, name);
                }
            }
            if rc_authority.native_value_tracking_enabled() {
                builder.ins().call(local_dec_ref_obj, &[old_val]);
            }
            return OpFlow::Continue;
        }
        "load_var" | "copy_var" => {
            // Load a named variable into an output (block arg receiving / copy).
            // Use Variable-backed shadow (phi-resolved across loop iterations)
            // when available, falling back to Value-based shadow.
            if let Some(ref var_name) = op.var
                && op.args.as_ref().is_none_or(|args| args.is_empty())
            {
                if let Some(&slot) = slot_backed_join_slots.get(var_name) {
                    // Raw-backed slot: the slot holds RAW i64 (or a
                    // raw 0/1 bool) — no unbox, no refcount. A
                    // raw-primary out takes the value verbatim; any
                    // other out gets the overflow-safe box (NEVER
                    // the trusted unboxed transport, which truncates
                    // at 2^47).
                    if raw_backed_slot_names.contains(var_name.as_str()) {
                        let raw_val = builder.ins().stack_load(types::I64, slot, 0);
                        if let Some(out_name) = op.out.as_ref().as_ref() {
                            if representation_plan.is_bool_unboxed(var_name.as_str()) {
                                def_raw_bool_value(
                                    &mut *builder,
                                    vars,
                                    representation_plan,
                                    out_name,
                                    raw_val,
                                    nbc,
                                );
                            } else if representation_plan.is_raw_int_carrier_name(out_name.as_str())
                            {
                                def_var_named(&mut *builder, vars, out_name, raw_val);
                            } else {
                                let boxed = box_raw_i64_value_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    &mut *sealed_blocks,
                                    raw_val,
                                );
                                def_var_from_boxed_transport(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    vars,
                                    representation_plan,
                                    nbc,
                                    out_name,
                                    boxed,
                                );
                            }
                        }
                        return OpFlow::Continue;
                    }
                    let val = builder.ins().stack_load(types::I64, slot, 0);
                    // RC drop-insertion substrate (design 20 §4.1, Phase 5):
                    // the load-side arm of the memory-phi value-tracking RC.
                    // The legacy model inc_refs on every slot LOAD so the
                    // loaded SSA value is OWNED, and balances it with a
                    // release at the value's last use. For drop-inserted
                    // functions the TIR drops own RC under the borrow model
                    // (design §1.2): a slot load is a BORROW (no new
                    // reference), and the TIR `DecRef` at the loaded value's
                    // last use is the genuine release of the slot occupant's
                    // single reference (the loop-carried back-edge drop).
                    // Keeping the load-inc here would pair it with that TIR
                    // `DecRef` (net zero) so the carried accumulator is never
                    // freed — the headline O(n) loop-accumulator leak. Skip
                    // it; the load yields a borrowed alias the TIR pass tracks
                    // in alias-root space.
                    if rc_authority.native_value_tracking_enabled() {
                        emit_inc_ref_obj(&mut *builder, val, local_inc_ref_obj, nbc);
                    }
                    if let Some(out_name) = op.out.as_ref().as_ref() {
                        def_var_from_boxed_transport(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            vars,
                            representation_plan,
                            nbc,
                            out_name,
                            val,
                        );
                    }
                    return OpFlow::Continue;
                }
                // --- Raw-primary int fast path ---
                // When source is raw-primary and output is proven-int,
                // transfer raw i64 directly -- no boxing, no refcount.
                if representation_plan.is_raw_int_carrier_name(var_name.as_str())
                    && scalar_fast_paths_enabled
                    && op
                        .out
                        .as_ref()
                        .is_some_and(|o| representation_plan.is_raw_int_carrier_name(o))
                {
                    let raw_val = int_raw_value(&mut *builder, vars, representation_plan, var_name)
                        .unwrap_or_else(|| {
                            let var = *vars
                                .get(var_name.as_str())
                                .expect("load_var: raw src var not found");
                            builder.use_var(var)
                        });
                    let out_name = op.out.as_ref().unwrap();
                    def_var_named(&mut *builder, vars, out_name, raw_val);
                    return OpFlow::Continue;
                }
                // --- Raw-primary float fast path ---
                // When output is float-primary, transfer raw f64 directly.
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| representation_plan.is_float_unboxed(o))
                    && scalar_fast_paths_enabled
                {
                    let raw_f64 =
                        float_value_for(&mut *builder, vars, representation_plan, var_name)
                            .unwrap_or_else(|| {
                                let boxed = var_get_boxed_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    &mut *sealed_blocks,
                                    vars,
                                    var_name,
                                    representation_plan,
                                )
                                .expect("load_var: float src not found");
                                builder
                                    .ins()
                                    .bitcast(types::F64, MemFlagsData::new(), *boxed)
                            });
                    let out_name = op.out.as_ref().unwrap();
                    def_var_named(&mut *builder, vars, out_name, raw_f64);
                    return OpFlow::Continue;
                }
                // --- Raw-primary bool fast path ---
                if representation_plan.is_bool_unboxed(var_name.as_str())
                    && scalar_fast_paths_enabled
                    && op
                        .out
                        .as_ref()
                        .is_some_and(|o| representation_plan.name_is_bool_scalar(o))
                {
                    let raw_bool =
                        bool_raw_value(&mut *builder, vars, representation_plan, var_name)
                            .unwrap_or_else(|| {
                                panic!("load_var: bool-primary src missing raw bool: {var_name}")
                            });
                    let out_name = op.out.as_ref().unwrap();
                    def_raw_bool_value(
                        &mut *builder,
                        vars,
                        representation_plan,
                        out_name,
                        raw_bool,
                        nbc,
                    );
                    return OpFlow::Continue;
                }
                let val = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    var_name,
                    representation_plan,
                )
                .expect("load_var: var not found");
                if let Some(out_name) = op.out.as_ref().as_ref() {
                    def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        representation_plan,
                        nbc,
                        out_name,
                        *val,
                    );
                }
            } else if let Some(args) = op.args.as_ref()
                && !args.is_empty()
            {
                if let Some(&slot) = slot_backed_join_slots.get(&args[0]) {
                    // Raw-backed slot (see the var-named arm above).
                    if raw_backed_slot_names.contains(args[0].as_str()) {
                        let raw_val = builder.ins().stack_load(types::I64, slot, 0);
                        if let Some(out_name) = op.out.as_ref().as_ref() {
                            if representation_plan.is_bool_unboxed(args[0].as_str()) {
                                def_raw_bool_value(
                                    &mut *builder,
                                    vars,
                                    representation_plan,
                                    out_name,
                                    raw_val,
                                    nbc,
                                );
                            } else if representation_plan.is_raw_int_carrier_name(out_name.as_str())
                            {
                                def_var_named(&mut *builder, vars, out_name, raw_val);
                            } else {
                                let boxed = box_raw_i64_value_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    &mut *sealed_blocks,
                                    raw_val,
                                );
                                def_var_from_boxed_transport(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    vars,
                                    representation_plan,
                                    nbc,
                                    out_name,
                                    boxed,
                                );
                            }
                        }
                        return OpFlow::Continue;
                    }
                    let val = builder.ins().stack_load(types::I64, slot, 0);
                    // RC drop-insertion substrate (design 20 §4.1, Phase 5):
                    // the load-side arm of the memory-phi value-tracking RC.
                    // The legacy model inc_refs on every slot LOAD so the
                    // loaded SSA value is OWNED, and balances it with a
                    // release at the value's last use. For drop-inserted
                    // functions the TIR drops own RC under the borrow model
                    // (design §1.2): a slot load is a BORROW (no new
                    // reference), and the TIR `DecRef` at the loaded value's
                    // last use is the genuine release of the slot occupant's
                    // single reference (the loop-carried back-edge drop).
                    // Keeping the load-inc here would pair it with that TIR
                    // `DecRef` (net zero) so the carried accumulator is never
                    // freed — the headline O(n) loop-accumulator leak. Skip
                    // it; the load yields a borrowed alias the TIR pass tracks
                    // in alias-root space.
                    if rc_authority.native_value_tracking_enabled() {
                        emit_inc_ref_obj(&mut *builder, val, local_inc_ref_obj, nbc);
                    }
                    if let Some(out_name) = op.out.as_ref().as_ref() {
                        def_var_from_boxed_transport(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            vars,
                            representation_plan,
                            nbc,
                            out_name,
                            val,
                        );
                    }
                    return OpFlow::Continue;
                }
                // --- Raw-primary int fast path (args-based copy_var) ---
                if representation_plan.is_raw_int_carrier_name(&args[0])
                    && scalar_fast_paths_enabled
                    && op
                        .out
                        .as_ref()
                        .is_some_and(|o| representation_plan.is_raw_int_carrier_name(o))
                {
                    let raw_val = int_raw_value(&mut *builder, vars, representation_plan, &args[0])
                        .unwrap_or_else(|| {
                            let var = *vars.get(&args[0]).expect("copy_var: raw src var not found");
                            builder.use_var(var)
                        });
                    let out_name = op.out.as_ref().unwrap();
                    def_var_named(&mut *builder, vars, out_name, raw_val);
                    return OpFlow::Continue;
                }
                // --- Raw-primary bool fast path (args-based copy_var) ---
                if representation_plan.is_bool_unboxed(&args[0])
                    && scalar_fast_paths_enabled
                    && op
                        .out
                        .as_ref()
                        .is_some_and(|o| representation_plan.name_is_bool_scalar(o))
                {
                    let raw_bool =
                        bool_raw_value(&mut *builder, vars, representation_plan, &args[0])
                            .unwrap_or_else(|| {
                                panic!("copy_var: bool-primary src missing raw bool: {}", args[0])
                            });
                    let out_name = op.out.as_ref().unwrap();
                    def_raw_bool_value(
                        &mut *builder,
                        vars,
                        representation_plan,
                        out_name,
                        raw_bool,
                        nbc,
                    );
                    return OpFlow::Continue;
                }
                let val = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                )
                .expect("copy_var: src not found");
                if let Some(out_name) = op.out.as_ref().as_ref() {
                    def_var_from_boxed_transport(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        vars,
                        representation_plan,
                        nbc,
                        out_name,
                        *val,
                    );
                }
            }
        }
        "load_param" => {
            // TIR emits load_param for function parameters — map param index
            // to the corresponding block param value
            let param_idx = op.value.unwrap_or(0) as usize;
            if let Some(out_name) = op.out.as_ref().as_ref() {
                let entry_block = builder.func.layout.entry_block().unwrap();
                let param_val = {
                    let params = builder.func.dfg.block_params(entry_block);
                    if param_idx < params.len() {
                        Some(params[param_idx])
                    } else {
                        None
                    }
                };
                if let Some(val) = param_val {
                    def_var_named(&mut *builder, vars, out_name, val);
                }
            }
        }
        _ => unreachable!("handle_ret_jump_op received non-ret/jump op `{}`", op.kind),
    }

    OpFlow::Proceed
}
