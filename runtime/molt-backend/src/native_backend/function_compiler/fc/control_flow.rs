use super::super::*;

/// Single-source kind authority for [`handle_control_flow_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["if", "else", "end_if"];
use super::OpFlow;
use super::list_index_fast_path::ListIndexFastPathState;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for structured `if`/`else`/`end_if` control flow.
///
/// Extracted from `compile_func_inner`'s per-op dispatch (M1.9). The handler
/// threads branch/phi/rebind state explicitly and preserves the original outer
/// op-loop `continue` exits through `OpFlow::Continue`, so the parent epilogue
/// is skipped exactly where the inline arms skipped it.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_control_flow_op(
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
    int_carriers_plan: &ScalarRepresentationPlan,
    bool_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    first_defined_at: &BTreeMap<String, usize>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    if_to_else: &BTreeMap<usize, usize>,
    if_to_end_if: &BTreeMap<usize, usize>,
    else_to_end_if: &BTreeMap<usize, usize>,
    int_store_target_names: &BTreeSet<String>,
    exception_label_ids: &BTreeSet<i64>,
    list_index_fast_paths: &ListIndexFastPathState,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    entry_vars: &mut BTreeMap<String, Value>,
    already_decrefed: &mut BTreeSet<String>,
    reachable_blocks: &mut BTreeSet<Block>,
    if_stack: &mut Vec<IfFrame>,
    skip_ops: &mut BTreeSet<usize>,
    is_block_filled: &mut bool,
    native_rc_tracking_enabled: bool,
    scalar_fast_paths_enabled: bool,
    representation_plan: &ScalarRepresentationPlan,
    maybe_debug_seal: &dyn Fn(&str, usize, Block),
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
    let op_prefers_bool_lane = |op: &OpIR| {
        scalar_fast_paths_enabled
            && representation_plan.op_scalar_lane(op) == Some(ScalarKind::Bool)
    };
    let op_prefers_int_lane = |op: &OpIR| {
        super::op_prefers_int_lane(
            scalar_fast_paths_enabled,
            representation_plan,
            op,
            int_like_vars,
            bool_like_vars,
            int_carriers_plan,
            bool_primary_vars,
        )
    };
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
                                       int_carriers_plan: &ScalarRepresentationPlan,
                                       float_primary_vars: &BTreeSet<String>|
     -> Option<crate::VarValue> {
        var_get_boxed_overflow_safe_fn(
            module,
            import_ids,
            builder,
            import_refs,
            sealed_blocks,
            vars,
            name,
            int_carriers_plan,
            float_primary_vars,
            bool_primary_vars,
            nbc,
        )
    };

    match op.kind.as_str() {
        "if" => {
            // Variable-backed shadows are phi-correct across branches —
            // use_var returns the merged value at any point.  No need to
            // clear or retain entries; the name→Variable mapping is static.
            // Value-tier raw shadows are block-local snapshots and must
            // not leak across branch boundaries.
            if builder.current_block().is_none()
                || builder
                    .current_block()
                    .is_some_and(|block| crate::block_has_terminator(&*builder, block))
            {
                let dead = builder.create_block();
                switch_to_block_materialized(&mut *builder, dead);
                seal_block_once(&mut *builder, sealed_blocks, dead);
                *is_block_filled = false;
            }
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Inline truthiness for bool/int types to avoid function call overhead.
            let cond_bool = if let Some(raw_val) =
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[0])
            {
                // Raw bool from proven list_bool getitem or const_bool.
                // Branch directly on raw 0/1 — ZERO NaN-box overhead.
                builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0)
            } else if op_prefers_bool_lane(op) {
                // NaN-boxed bool: bit 0 is the boolean value.
                let cond = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    import_refs,
                    sealed_blocks,
                    vars,
                    &args[0],
                    int_carriers_plan,
                    float_primary_vars,
                )
                .expect("Cond not found");
                let one = builder.ins().iconst(types::I64, 1);
                let bit0 = builder.ins().band(*cond, one);
                builder.ins().icmp_imm(IntCC::NotEqual, bit0, 0)
            } else if let Some(raw_shadow) =
                int_raw_value(&mut *builder, vars, int_carriers_plan, &args[0])
            {
                // Proven raw i64 carrier: truthiness is `value != 0`.
                builder.ins().icmp_imm(IntCC::NotEqual, raw_shadow, 0)
            } else if op_prefers_int_lane(op) {
                // op_prefers_int_lane only proves Python-`int` type, which
                // includes heap BigInts (TAG_PTR). The trusted unbox would
                // truncate a BigInt pointer, and e.g. `1 << 47` (low 47
                // bits zero) would be wrongly treated as falsy. Guard on a
                // runtime inline-int tag check: inline TAG_INT/TAG_BOOL use
                // `unbox != 0`; any heap int (BigInt) is non-zero by
                // construction, hence always truthy.
                let cond = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    import_refs,
                    sealed_blocks,
                    vars,
                    &args[0],
                    int_carriers_plan,
                    float_primary_vars,
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
                    import_refs,
                    sealed_blocks,
                    vars,
                    &args[0],
                    int_carriers_plan,
                    float_primary_vars,
                )
                .expect("Cond not found");
                // Speculative inline truthiness: check NaN-box tag
                // to avoid molt_is_truthy function call for bool/int.
                //
                // TAG_BOOL (0x7ffa...): bit 0 is the boolean value.
                // TAG_INT  (0x7ff9...): unbox and check payload != 0.
                // Other tags: fall through to molt_is_truthy call.
                let truthy_origin_block = builder.current_block();
                let truthy_live_through = collect_live_through_values(
                    &mut *builder,
                    vars,
                    first_defined_at,
                    last_use,
                    op_idx,
                    op.out.as_deref(),
                );
                let truthy_merge = builder.create_block();
                builder.append_block_param(truthy_merge, types::I8);
                append_live_through_params(&mut *builder, truthy_merge, &truthy_live_through);

                // Conditional list-bool carrier: when the source list
                // is list_bool, branch directly on the raw 0/1 payload;
                // otherwise continue into the normal NaN-box path.
                emit_conditional_list_bool_truthiness(
                    &mut *builder,
                    sealed_blocks,
                    &list_index_fast_paths.list_is_bool_cache,
                    list_index_fast_paths
                        .conditional_list_bool_shadows
                        .get(&args[0]),
                    truthy_merge,
                    &truthy_live_through,
                );

                let mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
                let masked = builder.ins().band(*cond, mask);

                // Check TAG_BOOL first (most likely for sieve-like patterns).
                let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
                let is_bool = builder.ins().icmp(IntCC::Equal, masked, bool_tag);
                let bool_block = builder.create_block();
                let not_bool_block = builder.create_block();
                builder
                    .ins()
                    .brif(is_bool, bool_block, &[], not_bool_block, &[]);

                // Bool path: extract bit 0.
                switch_to_block_materialized(&mut *builder, bool_block);
                seal_block_once(&mut *builder, sealed_blocks, bool_block);
                let bit0 = builder.ins().band_imm(*cond, 1);
                let bool_truthy = builder.ins().icmp_imm(IntCC::NotEqual, bit0, 0);
                let merge_args = merge_args_with_live_through(bool_truthy, &truthy_live_through);
                jump_block(&mut *builder, truthy_merge, &merge_args);

                // Not-bool: check TAG_INT.
                switch_to_block_materialized(&mut *builder, not_bool_block);
                seal_block_once(&mut *builder, sealed_blocks, not_bool_block);
                let int_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_int);
                let is_int = builder.ins().icmp(IntCC::Equal, masked, int_tag);
                let int_block = builder.create_block();
                let call_block = builder.create_block();
                builder.set_cold_block(call_block);
                builder.ins().brif(is_int, int_block, &[], call_block, &[]);

                // Int path: unbox and check != 0.
                switch_to_block_materialized(&mut *builder, int_block);
                seal_block_once(&mut *builder, sealed_blocks, int_block);
                let raw_val = unbox_int(&mut *builder, *cond, nbc);
                let int_truthy = builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0);
                let merge_args = merge_args_with_live_through(int_truthy, &truthy_live_through);
                jump_block(&mut *builder, truthy_merge, &merge_args);

                // Slow path: call molt_is_truthy.
                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, sealed_blocks, call_block);
                let truthy_fn_name = "molt_is_truthy";
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    truthy_fn_name,
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*cond]);
                let truthy = builder.inst_results(call)[0];
                let call_truthy = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
                let merge_args = merge_args_with_live_through(call_truthy, &truthy_live_through);
                jump_block(&mut *builder, truthy_merge, &merge_args);

                switch_to_block_materialized(&mut *builder, truthy_merge);
                seal_block_once(&mut *builder, sealed_blocks, truthy_merge);
                let truthy_params = builder.block_params(truthy_merge).to_vec();
                rebind_live_through_values(
                    &mut *builder,
                    vars,
                    &truthy_live_through,
                    &truthy_params[1..],
                );
                if let Some(origin_block) = truthy_origin_block
                    && origin_block != truthy_merge
                {
                    let obj_live = block_tracked_obj.remove(&origin_block).unwrap_or_default();
                    if !obj_live.is_empty() {
                        extend_unique_tracked(
                            block_tracked_obj.entry(truthy_merge).or_default(),
                            obj_live,
                        );
                    }
                    let ptr_live = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
                    if !ptr_live.is_empty() {
                        extend_unique_tracked(
                            block_tracked_ptr.entry(truthy_merge).or_default(),
                            ptr_live,
                        );
                    }
                }
                truthy_params[0]
            };
            // `if` terminates the current block (brif) into then/else blocks. Any live
            // tracked values must be carried into both successors; otherwise they leak
            // when the predecessor block is never revisited.
            let origin_block = builder
                .current_block()
                .expect("if requires an active block");
            let mut carry_obj = block_tracked_obj.remove(&origin_block).unwrap_or_default();
            let cleanup_obj = drain_cleanup_tracked_dedup_with_authority(
                native_rc_tracking_enabled,
                &mut carry_obj,
                last_use,
                alias_roots,
                op_idx,
                None,
                Some(already_decrefed),
            );
            for name in cleanup_obj {
                let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                    .unwrap_or_else(|| {
                        panic!(
                            "Tracked obj var not found in {} op {}: {}",
                            func_name, op_idx, name
                        )
                    });
                builder.ins().call(local_dec_ref_obj, &[val]);
            }
            let mut carry_ptr = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
            let cleanup_ptr = drain_cleanup_tracked_dedup_with_authority(
                native_rc_tracking_enabled,
                &mut carry_ptr,
                last_use,
                alias_roots,
                op_idx,
                None,
                Some(already_decrefed),
            );
            for name in cleanup_ptr {
                let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                    .unwrap_or_else(|| {
                        panic!(
                            "Tracked ptr var not found in {} op {}: {}",
                            func_name, op_idx, name
                        )
                    });
                builder.ins().call(local_dec_ref_obj, &[val]);
            }
            let has_explicit_else = if_to_else.contains_key(&op_idx);
            let end_if_idx = match if_to_end_if.get(&op_idx) {
                Some(&idx) => idx,
                None => {
                    eprintln!(
                        "WARNING: `if` at op {} in function `{}` has no matching end_if — skipping",
                        op_idx, func_name
                    );
                    return OpFlow::Continue;
                }
            };
            let has_phi_join = func_ops
                .get(end_if_idx + 1)
                .is_some_and(|next| next.kind == "phi");
            let then_block = builder.create_block();
            let else_block = if has_explicit_else || has_phi_join {
                Some(builder.create_block())
            } else {
                None
            };
            let merge_block = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(then_block, current_block);
                let mut last_layout_block = then_block;
                if let Some(else_block) = else_block {
                    builder.insert_block_after(else_block, then_block);
                    last_layout_block = else_block;
                }
                builder.insert_block_after(merge_block, last_layout_block);
            }
            reachable_blocks.insert(then_block);
            if let Some(else_block) = else_block {
                reachable_blocks.insert(else_block);
            }
            if !carry_obj.is_empty() {
                extend_unique_tracked(
                    block_tracked_obj.entry(then_block).or_default(),
                    carry_obj.clone(),
                );
                if let Some(else_block) = else_block {
                    extend_unique_tracked(
                        block_tracked_obj.entry(else_block).or_default(),
                        carry_obj.clone(),
                    );
                } else {
                    extend_unique_tracked(
                        block_tracked_obj.entry(merge_block).or_default(),
                        carry_obj.clone(),
                    );
                }
            }
            if !carry_ptr.is_empty() {
                extend_unique_tracked(
                    block_tracked_ptr.entry(then_block).or_default(),
                    carry_ptr.clone(),
                );
                if let Some(else_block) = else_block {
                    extend_unique_tracked(
                        block_tracked_ptr.entry(else_block).or_default(),
                        carry_ptr.clone(),
                    );
                } else {
                    extend_unique_tracked(
                        block_tracked_ptr.entry(merge_block).or_default(),
                        carry_ptr.clone(),
                    );
                }
            }
            let false_block = else_block.unwrap_or(merge_block);
            if else_block.is_none() {
                reachable_blocks.insert(merge_block);
            }
            let mut phi_ops: Vec<(String, String, String)> = Vec::new();
            let mut merge_rebind_names: Vec<String> = Vec::new();
            if let Some(end_if_idx) = if_to_end_if.get(&op_idx).copied() {
                let mut scan_idx = end_if_idx + 1;
                while scan_idx < func_ops.len() {
                    let next = &func_ops[scan_idx];
                    if next.kind != "phi" {
                        break;
                    }
                    let args = next.args.as_ref().expect("phi args missing");
                    if args.len() != 2 {
                        panic!("phi expects exactly two args");
                    }
                    let out = next.out.clone().expect("phi output missing");
                    phi_ops.push((out, args[0].clone(), args[1].clone()));
                    skip_ops.insert(scan_idx);
                    scan_idx += 1;
                }
                if phi_ops.is_empty() {
                    let mut seen_merge_rebind: BTreeSet<String> = BTreeSet::new();
                    for branch_idx in (op_idx + 1)..end_if_idx {
                        let branch_op = &func_ops[branch_idx];
                        if !matches!(branch_op.kind.as_str(), "store_var" | "delete_var") {
                            continue;
                        }
                        let Some(name) = branch_op.var.as_ref() else {
                            continue;
                        };
                        if name == "none" || int_store_target_names.contains(name) {
                            continue;
                        }
                        if last_use.get(name).copied().unwrap_or(0) <= end_if_idx {
                            continue;
                        }
                        if seen_merge_rebind.insert(name.clone()) {
                            merge_rebind_names.push(name.clone());
                        }
                    }
                }
            }
            let phi_params: Vec<Value> = phi_ops
                .iter()
                .map(|(out, _, _)| {
                    let storage = merge_rebind_storage_for_name(
                        out,
                        int_carriers_plan,
                        bool_primary_vars,
                        float_primary_vars,
                    );
                    builder.append_block_param(merge_block, merge_rebind_storage_clif_type(storage))
                })
                .collect();
            let merge_rebind_storages: Vec<MergeRebindStorageKind> = merge_rebind_names
                .iter()
                .map(|name| {
                    merge_rebind_storage_for_name(
                        name,
                        int_carriers_plan,
                        bool_primary_vars,
                        float_primary_vars,
                    )
                })
                .collect();
            let merge_rebind_params: Vec<Value> = merge_rebind_storages
                .iter()
                .map(|storage| {
                    builder
                        .append_block_param(merge_block, merge_rebind_storage_clif_type(*storage))
                })
                .collect();
            if std::env::var("MOLT_DEBUG_IF_MERGE_SLOTS").as_deref() == Ok(func_name) {
                let current_block = builder.current_block();
                let current_filled = current_block
                    .map(|block| crate::block_has_terminator(&*builder, block))
                    .unwrap_or(false);
                eprintln!(
                    "IF_MERGE_SLOTS func={} op={} block={:?} block_filled={} names={:?}",
                    func_name, op_idx, current_block, current_filled, merge_rebind_names
                );
            }
            let merge_rebind_slots = merge_rebind_names
                .iter()
                .zip(merge_rebind_storages.iter().copied())
                .map(|(name, storage)| {
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let has_reaching_def = first_defined_at
                        .get(name)
                        .copied()
                        .is_some_and(|first| first <= op_idx);
                    if std::env::var("MOLT_DEBUG_IF_MERGE_SLOTS").as_deref() == Ok(func_name) {
                        eprintln!(
                            "IF_MERGE_INIT func={} op={} name={} reaching_def={}",
                            func_name, op_idx, name, has_reaching_def
                        );
                    }
                    let init = if has_reaching_def {
                        merge_rebind_value_for_storage(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            sealed_blocks,
                            vars,
                            bool_like_vars,
                            int_carriers_plan,
                            bool_primary_vars,
                            float_primary_vars,
                            nbc,
                            name,
                            storage,
                        )
                    } else {
                        merge_rebind_default_value(&mut *builder, storage)
                    };
                    builder.ins().stack_store(init, slot, 0);
                    MergeRebindSlot { slot, storage }
                })
                .collect();
            if std::env::var("MOLT_DEBUG_PHI_ARGS").as_deref() == Ok(func_name) {
                let _ = crate::debug_artifacts::append_debug_artifact(
                    "native/ifmerge_debug.txt",
                    format!("MERGE_INIT {} names={:?}\n", func_name, merge_rebind_names),
                );
            }

            builder
                .ins()
                .brif(cond_bool, then_block, &[], false_block, &[]);

            // Seal blocks now that their predecessor sets are complete.
            // Structured `if` creates exactly one predecessor for each of then/else.
            //
            // Note: we deliberately do not seal `origin_block` here because it may have
            // been sealed earlier (for example the function entry block is sealed up-front).
            if exception_label_ids.is_empty() && sealed_blocks.insert(then_block) {
                maybe_debug_seal("if_then", op_idx, then_block);
                seal_block_once(&mut *builder, sealed_blocks, then_block);
            }
            if let Some(else_block) = else_block
                && exception_label_ids.is_empty()
                && sealed_blocks.insert(else_block)
            {
                maybe_debug_seal("if_else", op_idx, else_block);
                seal_block_once(&mut *builder, sealed_blocks, else_block);
            }

            switch_to_block_with_rebind(&mut *builder, then_block, is_block_filled, false);
            if_stack.push(IfFrame {
                else_block,
                merge_block,
                has_else: false,
                then_terminal: false,
                else_terminal: false,
                phi_ops,
                phi_params,
                merge_rebind_names,
                merge_rebind_params,
                merge_rebind_slots,
            });
        }
        "else" => {
            // Variable-backed shadows are phi-correct; value-tier raw
            // snapshots must be cleared when switching branch blocks.
            let frame = if_stack.last_mut().expect("No if on stack");
            frame.then_terminal = *is_block_filled;
            if frame.phi_ops.is_empty() {
                let end_if_idx = *else_to_end_if
                    .get(&op_idx)
                    .expect("else without matching end_if");
                let mut phi_ops: Vec<(String, String, String)> = Vec::new();
                let mut scan_idx = end_if_idx + 1;
                while scan_idx < func_ops.len() {
                    let next = &func_ops[scan_idx];
                    if next.kind != "phi" {
                        break;
                    }
                    let args = next.args.as_ref().expect("phi args missing");
                    if args.len() != 2 {
                        panic!("phi expects exactly two args");
                    }
                    let out = next.out.clone().expect("phi output missing");
                    phi_ops.push((out, args[0].clone(), args[1].clone()));
                    skip_ops.insert(scan_idx);
                    scan_idx += 1;
                }
                frame.phi_ops = phi_ops;
            }

            if !*is_block_filled {
                // If this structured `if` is followed by `phi` func_ops, route values through
                // merge-block parameters (real SSA join) instead of attempting to "define"
                // the output in each predecessor block.
                let mut phi_args: Vec<Value> = Vec::new();
                let mut merge_rebind_args: Vec<Value> = Vec::new();
                if !frame.phi_ops.is_empty() {
                    if frame.phi_params.is_empty() {
                        for (out, then_name, _else_name) in &frame.phi_ops {
                            let storage = merge_rebind_storage_for_name(
                                out,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                            );
                            let then_val = merge_rebind_value_for_storage(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                bool_like_vars,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                                nbc,
                                then_name,
                                storage,
                            );
                            let ty = builder.func.dfg.value_type(then_val);
                            let param = builder.append_block_param(frame.merge_block, ty);
                            frame.phi_params.push(param);
                            phi_args.push(then_val);
                        }
                    } else {
                        for (out, then_name, _else_name) in &frame.phi_ops {
                            let storage = merge_rebind_storage_for_name(
                                out,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                            );
                            let then_val = merge_rebind_value_for_storage(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                bool_like_vars,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                                nbc,
                                then_name,
                                storage,
                            );
                            phi_args.push(then_val);
                        }
                    }
                }
                if frame.phi_ops.is_empty() && !frame.merge_rebind_names.is_empty() {
                    for (idx, name) in frame.merge_rebind_names.iter().enumerate() {
                        let rebind_slot = frame.merge_rebind_slots[idx];
                        let val = merge_rebind_value_for_storage(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            sealed_blocks,
                            vars,
                            bool_like_vars,
                            int_carriers_plan,
                            bool_primary_vars,
                            float_primary_vars,
                            nbc,
                            name,
                            rebind_slot.storage,
                        );
                        builder.ins().stack_store(val, rebind_slot.slot, 0);
                        merge_rebind_args.push(val);
                    }
                    if std::env::var("MOLT_DEBUG_PHI_ARGS").as_deref() == Ok(func_name) {
                        eprintln!(
                            "MERGE_REBIND {} else_op names={:?} args={:?}",
                            func_name, frame.merge_rebind_names, merge_rebind_args
                        );
                        let _ = crate::debug_artifacts::append_debug_artifact(
                            "native/ifmerge_debug.txt",
                            format!(
                                "MERGE_REBIND {} else_op names={:?} args={:?}\n",
                                func_name, frame.merge_rebind_names, merge_rebind_args
                            ),
                        );
                    }
                }
                if let Some(block) = builder.current_block() {
                    let protected_phi_inputs: BTreeSet<&str> = frame
                        .phi_ops
                        .iter()
                        .map(|(_, then_name, _)| then_name.as_str())
                        .collect();
                    let mut carry_obj = block_tracked_obj.remove(&block).unwrap_or_default();
                    let cleanup = drain_cleanup_tracked_dedup_with_authority(
                        native_rc_tracking_enabled,
                        &mut carry_obj,
                        last_use,
                        alias_roots,
                        op_idx,
                        None,
                        Some(already_decrefed),
                    );
                    let cleanup = protect_cleanup_names(
                        &mut carry_obj,
                        cleanup,
                        &protected_phi_inputs,
                        alias_roots,
                        already_decrefed,
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
                            block_tracked_obj.entry(frame.merge_block).or_default(),
                            carry_obj,
                        );
                    }

                    let mut carry_ptr = block_tracked_ptr.remove(&block).unwrap_or_default();
                    let cleanup = drain_cleanup_tracked_dedup_with_authority(
                        native_rc_tracking_enabled,
                        &mut carry_ptr,
                        last_use,
                        alias_roots,
                        op_idx,
                        None,
                        Some(already_decrefed),
                    );
                    let cleanup = protect_cleanup_names(
                        &mut carry_ptr,
                        cleanup,
                        &protected_phi_inputs,
                        alias_roots,
                        already_decrefed,
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
                            block_tracked_ptr.entry(frame.merge_block).or_default(),
                            carry_ptr,
                        );
                    }
                    ensure_block_in_layout(&mut *builder, frame.merge_block);
                    reachable_blocks.insert(frame.merge_block);
                    if std::env::var("MOLT_DEBUG_PHI_ARGS").as_deref() == Ok(func_name) {
                        eprintln!(
                            "PHI_ARGS {} else_op block={:?} merge={:?} args={:?} phi_ops={:?}",
                            func_name,
                            builder.current_block(),
                            frame.merge_block,
                            phi_args,
                            frame.phi_ops
                        );
                    }
                    let jump_args = if frame.phi_ops.is_empty() {
                        &merge_rebind_args
                    } else {
                        &phi_args
                    };
                    jump_block(&mut *builder, frame.merge_block, jump_args);
                }
            }

            switch_to_block_with_rebind(
                &mut *builder,
                frame.else_block.expect("else without placeholder block"),
                is_block_filled,
                false,
            );
            frame.has_else = true;
        }
        "end_if" => {
            // Variable-backed shadows are phi-correct; value-tier raw
            // snapshots must not survive the branch merge.
            let mut frame = if_stack.pop().expect("No if on stack");
            if frame.phi_ops.is_empty() {
                let mut phi_ops: Vec<(String, String, String)> = Vec::new();
                let mut scan_idx = op_idx + 1;
                while scan_idx < func_ops.len() {
                    let next = &func_ops[scan_idx];
                    if next.kind != "phi" {
                        break;
                    }
                    let args = next.args.as_ref().expect("phi args missing");
                    if args.len() != 2 {
                        panic!("phi expects exactly two args");
                    }
                    let out = next.out.clone().expect("phi output missing");
                    phi_ops.push((out, args[0].clone(), args[1].clone()));
                    skip_ops.insert(scan_idx);
                    scan_idx += 1;
                }
                frame.phi_ops = phi_ops;
            }

            if frame.has_else {
                frame.else_terminal = *is_block_filled;
                if !*is_block_filled {
                    let mut phi_args: Vec<Value> = Vec::new();
                    let mut merge_rebind_args: Vec<Value> = Vec::new();
                    if !frame.phi_ops.is_empty() {
                        if frame.phi_params.is_empty() {
                            for (out, _then_name, else_name) in &frame.phi_ops {
                                let storage = merge_rebind_storage_for_name(
                                    out,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                );
                                let else_val = merge_rebind_value_for_storage(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    bool_like_vars,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                    nbc,
                                    else_name,
                                    storage,
                                );
                                let ty = builder.func.dfg.value_type(else_val);
                                let param = builder.append_block_param(frame.merge_block, ty);
                                frame.phi_params.push(param);
                                phi_args.push(else_val);
                            }
                        } else {
                            for (out, _then_name, else_name) in &frame.phi_ops {
                                let storage = merge_rebind_storage_for_name(
                                    out,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                );
                                let else_val = merge_rebind_value_for_storage(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    bool_like_vars,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                    nbc,
                                    else_name,
                                    storage,
                                );
                                phi_args.push(else_val);
                            }
                        }
                    }
                    if frame.phi_ops.is_empty() && !frame.merge_rebind_names.is_empty() {
                        for (idx, name) in frame.merge_rebind_names.iter().enumerate() {
                            let rebind_slot = frame.merge_rebind_slots[idx];
                            let then_val = merge_rebind_value_for_storage(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                bool_like_vars,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                                nbc,
                                name,
                                rebind_slot.storage,
                            );
                            builder.ins().stack_store(then_val, rebind_slot.slot, 0);
                            merge_rebind_args.push(then_val);
                        }
                    }
                    if let Some(block) = builder.current_block() {
                        let protected_phi_inputs: BTreeSet<&str> = frame
                            .phi_ops
                            .iter()
                            .map(|(_, _, else_name)| else_name.as_str())
                            .collect();
                        let mut carry_obj = block_tracked_obj.remove(&block).unwrap_or_default();
                        let cleanup = drain_cleanup_tracked_dedup_with_authority(
                            native_rc_tracking_enabled,
                            &mut carry_obj,
                            last_use,
                            alias_roots,
                            op_idx,
                            None,
                            Some(already_decrefed),
                        );
                        let cleanup = protect_cleanup_names(
                            &mut carry_obj,
                            cleanup,
                            &protected_phi_inputs,
                            alias_roots,
                            already_decrefed,
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
                                block_tracked_obj.entry(frame.merge_block).or_default(),
                                carry_obj,
                            );
                        }

                        let mut carry_ptr = block_tracked_ptr.remove(&block).unwrap_or_default();
                        let cleanup = drain_cleanup_tracked_dedup_with_authority(
                            native_rc_tracking_enabled,
                            &mut carry_ptr,
                            last_use,
                            alias_roots,
                            op_idx,
                            None,
                            Some(already_decrefed),
                        );
                        let cleanup = protect_cleanup_names(
                            &mut carry_ptr,
                            cleanup,
                            &protected_phi_inputs,
                            alias_roots,
                            already_decrefed,
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
                                block_tracked_ptr.entry(frame.merge_block).or_default(),
                                carry_ptr,
                            );
                        }
                        ensure_block_in_layout(&mut *builder, frame.merge_block);
                        reachable_blocks.insert(frame.merge_block);
                        if std::env::var("MOLT_DEBUG_PHI_ARGS").as_deref() == Ok(func_name) {
                            eprintln!(
                                "PHI_ARGS {} end_if_else block={:?} merge={:?} args={:?} phi_ops={:?}",
                                func_name,
                                builder.current_block(),
                                frame.merge_block,
                                phi_args,
                                frame.phi_ops
                            );
                        }
                        let jump_args = if frame.phi_ops.is_empty() {
                            &merge_rebind_args
                        } else {
                            &phi_args
                        };
                        jump_block(&mut *builder, frame.merge_block, jump_args);
                    }
                }
            } else {
                frame.then_terminal = *is_block_filled;
                frame.else_terminal = false;
                if !*is_block_filled {
                    let mut phi_args: Vec<Value> = Vec::new();
                    let mut merge_rebind_args: Vec<Value> = Vec::new();
                    if !frame.phi_ops.is_empty() {
                        if frame.phi_params.is_empty() {
                            for (out, then_name, _else_name) in &frame.phi_ops {
                                let storage = merge_rebind_storage_for_name(
                                    out,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                );
                                let then_val = merge_rebind_value_for_storage(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    bool_like_vars,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                    nbc,
                                    then_name,
                                    storage,
                                );
                                let ty = builder.func.dfg.value_type(then_val);
                                let param = builder.append_block_param(frame.merge_block, ty);
                                frame.phi_params.push(param);
                                phi_args.push(then_val);
                            }
                        } else {
                            for (out, then_name, _else_name) in &frame.phi_ops {
                                let storage = merge_rebind_storage_for_name(
                                    out,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                );
                                let then_val = merge_rebind_value_for_storage(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    bool_like_vars,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                    nbc,
                                    then_name,
                                    storage,
                                );
                                phi_args.push(then_val);
                            }
                        }
                    }
                    if frame.phi_ops.is_empty() && !frame.merge_rebind_names.is_empty() {
                        for (idx, name) in frame.merge_rebind_names.iter().enumerate() {
                            let rebind_slot = frame.merge_rebind_slots[idx];
                            let then_val = merge_rebind_value_for_storage(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                bool_like_vars,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                                nbc,
                                name,
                                rebind_slot.storage,
                            );
                            builder.ins().stack_store(then_val, rebind_slot.slot, 0);
                            merge_rebind_args.push(then_val);
                        }
                    }
                    if let Some(block) = builder.current_block() {
                        let protected_phi_inputs: BTreeSet<&str> = frame
                            .phi_ops
                            .iter()
                            .map(|(_, then_name, _)| then_name.as_str())
                            .collect();
                        let mut carry_obj = block_tracked_obj.remove(&block).unwrap_or_default();
                        let cleanup = drain_cleanup_tracked_dedup_with_authority(
                            native_rc_tracking_enabled,
                            &mut carry_obj,
                            last_use,
                            alias_roots,
                            op_idx,
                            None,
                            Some(already_decrefed),
                        );
                        let cleanup = protect_cleanup_names(
                            &mut carry_obj,
                            cleanup,
                            &protected_phi_inputs,
                            alias_roots,
                            already_decrefed,
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
                                block_tracked_obj.entry(frame.merge_block).or_default(),
                                carry_obj,
                            );
                        }

                        let mut carry_ptr = block_tracked_ptr.remove(&block).unwrap_or_default();
                        let cleanup = drain_cleanup_tracked_dedup_with_authority(
                            native_rc_tracking_enabled,
                            &mut carry_ptr,
                            last_use,
                            alias_roots,
                            op_idx,
                            None,
                            Some(already_decrefed),
                        );
                        let cleanup = protect_cleanup_names(
                            &mut carry_ptr,
                            cleanup,
                            &protected_phi_inputs,
                            alias_roots,
                            already_decrefed,
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
                                block_tracked_ptr.entry(frame.merge_block).or_default(),
                                carry_ptr,
                            );
                        }
                        ensure_block_in_layout(&mut *builder, frame.merge_block);
                        reachable_blocks.insert(frame.merge_block);
                        if std::env::var("MOLT_DEBUG_PHI_ARGS").as_deref() == Ok(func_name) {
                            eprintln!(
                                "PHI_ARGS {} end_if_then block={:?} merge={:?} args={:?} phi_ops={:?}",
                                func_name,
                                builder.current_block(),
                                frame.merge_block,
                                phi_args,
                                frame.phi_ops
                            );
                        }
                        let jump_args = if frame.phi_ops.is_empty() {
                            &merge_rebind_args
                        } else {
                            &phi_args
                        };
                        jump_block(&mut *builder, frame.merge_block, jump_args);
                    }
                }

                if let Some(else_block) = frame.else_block {
                    switch_to_block_with_rebind(&mut *builder, else_block, is_block_filled, false);
                    if *is_block_filled {
                        frame.else_terminal = true;
                        return OpFlow::Continue;
                    }
                    let mut phi_args: Vec<Value> = Vec::new();
                    let mut merge_rebind_args: Vec<Value> = Vec::new();
                    if !frame.phi_ops.is_empty() {
                        if frame.phi_params.is_empty() {
                            for (out, _then_name, else_name) in &frame.phi_ops {
                                let storage = merge_rebind_storage_for_name(
                                    out,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                );
                                let else_val = merge_rebind_value_for_storage(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    bool_like_vars,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                    nbc,
                                    else_name,
                                    storage,
                                );
                                let ty = builder.func.dfg.value_type(else_val);
                                let param = builder.append_block_param(frame.merge_block, ty);
                                frame.phi_params.push(param);
                                phi_args.push(else_val);
                            }
                        } else {
                            for (out, _then_name, else_name) in &frame.phi_ops {
                                let storage = merge_rebind_storage_for_name(
                                    out,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                );
                                let else_val = merge_rebind_value_for_storage(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    bool_like_vars,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                    nbc,
                                    else_name,
                                    storage,
                                );
                                phi_args.push(else_val);
                            }
                        }
                    }
                    if frame.phi_ops.is_empty() && !frame.merge_rebind_names.is_empty() {
                        for (idx, name) in frame.merge_rebind_names.iter().enumerate() {
                            let rebind_slot = frame.merge_rebind_slots[idx];
                            let else_val = merge_rebind_value_for_storage(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                bool_like_vars,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                                nbc,
                                name,
                                rebind_slot.storage,
                            );
                            builder.ins().stack_store(else_val, rebind_slot.slot, 0);
                            merge_rebind_args.push(else_val);
                        }
                        if std::env::var("MOLT_DEBUG_PHI_ARGS").as_deref() == Ok(func_name) {
                            eprintln!(
                                "MERGE_REBIND {} end_if_else names={:?} args={:?}",
                                func_name, frame.merge_rebind_names, merge_rebind_args
                            );
                            let _ = crate::debug_artifacts::append_debug_artifact(
                                "native/ifmerge_debug.txt",
                                format!(
                                    "MERGE_REBIND {} end_if_else names={:?} args={:?}\n",
                                    func_name, frame.merge_rebind_names, merge_rebind_args
                                ),
                            );
                        }
                    }
                    if let Some(block) = builder.current_block() {
                        let mut carry_obj = block_tracked_obj.remove(&block).unwrap_or_default();
                        let cleanup = drain_cleanup_tracked_dedup_with_authority(
                            native_rc_tracking_enabled,
                            &mut carry_obj,
                            last_use,
                            alias_roots,
                            op_idx,
                            None,
                            Some(already_decrefed),
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
                                block_tracked_obj.entry(frame.merge_block).or_default(),
                                carry_obj,
                            );
                        }

                        let mut carry_ptr = block_tracked_ptr.remove(&block).unwrap_or_default();
                        let cleanup = drain_cleanup_tracked_dedup_with_authority(
                            native_rc_tracking_enabled,
                            &mut carry_ptr,
                            last_use,
                            alias_roots,
                            op_idx,
                            None,
                            Some(already_decrefed),
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
                                block_tracked_ptr.entry(frame.merge_block).or_default(),
                                carry_ptr,
                            );
                        }
                    }
                    ensure_block_in_layout(&mut *builder, frame.merge_block);
                    reachable_blocks.insert(frame.merge_block);
                    if std::env::var("MOLT_DEBUG_PHI_ARGS").as_deref() == Ok(func_name) {
                        eprintln!(
                            "PHI_ARGS {} synthetic_else block={:?} merge={:?} args={:?} phi_ops={:?}",
                            func_name,
                            builder.current_block(),
                            frame.merge_block,
                            phi_args,
                            frame.phi_ops
                        );
                    }
                    let jump_args = if frame.phi_ops.is_empty() {
                        &merge_rebind_args
                    } else {
                        &phi_args
                    };
                    jump_block(&mut *builder, frame.merge_block, jump_args);
                }
            }

            let both_filled = frame.then_terminal && frame.else_terminal;
            if both_filled {
                *is_block_filled = true;
            } else if reachable_blocks.contains(&frame.merge_block) {
                ensure_block_in_layout(&mut *builder, frame.merge_block);
                // For plain structured if/else the predecessor set is
                // complete at `end_if`, so early sealing keeps SSA
                // repair from inventing placeholder predecessors.
                //
                // Exception-bearing functions are different: handler
                // paths can still reach the merge later in the linear
                // stream, and sealing here bakes in an incomplete
                // predecessor set. Let `seal_all_blocks()` finalize
                // those cases once every edge is emitted.
                if exception_label_ids.is_empty() && sealed_blocks.insert(frame.merge_block) {
                    maybe_debug_seal("if_merge", op_idx, frame.merge_block);
                    seal_block_once(&mut *builder, sealed_blocks, frame.merge_block);
                }
                switch_to_block_with_rebind(
                    &mut *builder,
                    frame.merge_block,
                    is_block_filled,
                    false,
                );
                if !*is_block_filled
                    && frame.phi_ops.is_empty()
                    && !frame.merge_rebind_names.is_empty()
                {
                    for (idx, name) in frame.merge_rebind_names.iter().enumerate() {
                        let rebind_slot = frame.merge_rebind_slots[idx];
                        let val = builder.ins().stack_load(
                            merge_rebind_storage_clif_type(rebind_slot.storage),
                            rebind_slot.slot,
                            0,
                        );
                        def_var_from_merge_rebind_storage(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            vars,
                            int_carriers_plan,
                            bool_primary_vars,
                            float_primary_vars,
                            nbc,
                            name,
                            val,
                            rebind_slot.storage,
                        );
                    }
                    if std::env::var("MOLT_DEBUG_PHI_ARGS").as_deref() == Ok(func_name) {
                        eprintln!(
                            "MERGE_REBIND {} merge names={:?} params={:?}",
                            func_name, frame.merge_rebind_names, frame.merge_rebind_params
                        );
                        let _ = crate::debug_artifacts::append_debug_artifact(
                            "native/ifmerge_debug.txt",
                            format!(
                                "MERGE_REBIND {} merge names={:?} params={:?}\n",
                                func_name, frame.merge_rebind_names, frame.merge_rebind_params
                            ),
                        );
                    }
                }
                // Materialize the merged value(s) for any `phi` ops by binding the
                // merge-block parameters to their output variable names.
                // Guard: skip if the merge block was already filled (can't emit defs).
                if !*is_block_filled && !frame.phi_ops.is_empty() {
                    let phi_join_slot_names: Vec<Option<String>> = {
                        let mut names: Vec<Option<String>> = Vec::new();
                        let mut scan_idx = op_idx + 1;
                        while scan_idx < func_ops.len() && func_ops[scan_idx].kind == "phi" {
                            scan_idx += 1;
                        }
                        if scan_idx < func_ops.len()
                            && matches!(func_ops[scan_idx].kind.as_str(), "label" | "state_label")
                        {
                            scan_idx += 1;
                        }
                        while scan_idx < func_ops.len() && names.len() < frame.phi_ops.len() {
                            let next = &func_ops[scan_idx];
                            if next.kind != "load_var" {
                                break;
                            }
                            if let Some(var_name) = next.var.as_ref()
                                && is_join_slot_name(var_name)
                            {
                                names.push(Some(var_name.clone()));
                                scan_idx += 1;
                                continue;
                            }
                            break;
                        }
                        names.resize(frame.phi_ops.len(), None);
                        names
                    };
                    let mut remove_names: BTreeSet<&str> = BTreeSet::new();
                    for (idx, (out, _then_name, _else_name)) in frame.phi_ops.iter().enumerate() {
                        let param = frame.phi_params.get(idx).copied().unwrap_or_else(|| {
                            panic!("phi param missing for {out} in {}", func_name)
                        });
                        let out_storage = merge_rebind_storage_for_name(
                            out,
                            int_carriers_plan,
                            bool_primary_vars,
                            float_primary_vars,
                        );
                        def_var_from_merge_rebind_storage(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            vars,
                            int_carriers_plan,
                            bool_primary_vars,
                            float_primary_vars,
                            nbc,
                            out,
                            param,
                            out_storage,
                        );
                        if let Some(Some(join_name)) = phi_join_slot_names.get(idx) {
                            let join_storage = merge_rebind_storage_for_name(
                                join_name,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                            );
                            let join_value = if join_storage == out_storage {
                                param
                            } else {
                                merge_rebind_value_for_storage(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    bool_like_vars,
                                    int_carriers_plan,
                                    bool_primary_vars,
                                    float_primary_vars,
                                    nbc,
                                    out,
                                    join_storage,
                                )
                            };
                            def_var_from_merge_rebind_storage(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                vars,
                                int_carriers_plan,
                                bool_primary_vars,
                                float_primary_vars,
                                nbc,
                                join_name,
                                join_value,
                                join_storage,
                            );
                        }
                    }
                    // Refcount tracking is name-based. A `phi` output is a new name for a
                    // value that came from one of the predecessor blocks. If we don't
                    // transfer tracking to the output name, the predecessor name can be
                    // decref'd at the phi boundary while the output is still live,
                    // leading to UAF/segfaults for object-valued if-expressions.
                    for (_out, then_name, else_name) in &frame.phi_ops {
                        remove_names.insert(then_name.as_str());
                        remove_names.insert(else_name.as_str());
                    }
                    tracked_vars.retain(|name: &String| !remove_names.contains(name.as_str()));
                    tracked_vars_set.retain(|name| !remove_names.contains(name.as_str()));
                    tracked_obj_vars.retain(|name: &String| !remove_names.contains(name.as_str()));
                    tracked_obj_vars_set.retain(|name| !remove_names.contains(name.as_str()));
                    entry_vars.retain(|name, _| !remove_names.contains(name.as_str()));
                    if let Some(tracked) = block_tracked_obj.get_mut(&frame.merge_block) {
                        tracked.retain(|name| !remove_names.contains(name.as_str()));
                        let mut present: BTreeSet<String> = tracked.iter().cloned().collect();
                        for (out, _then_name, _else_name) in &frame.phi_ops {
                            if present.insert(out.clone()) {
                                tracked.push(out.clone());
                            }
                        }
                    }
                    if let Some(tracked) = block_tracked_ptr.get_mut(&frame.merge_block) {
                        tracked.retain(|name| !remove_names.contains(name.as_str()));
                        let mut present: BTreeSet<String> = tracked.iter().cloned().collect();
                        for (out, _then_name, _else_name) in &frame.phi_ops {
                            if present.insert(out.clone()) {
                                tracked.push(out.clone());
                            }
                        }
                    }
                }
            } else {
                *is_block_filled = true;
            }
        }
        _ => unreachable!(
            "handle_control_flow_op received non-control-flow op `{}`",
            op.kind
        ),
    }

    OpFlow::Proceed
}
