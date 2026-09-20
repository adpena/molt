use super::super::*;

/// Single-source kind authority for [`handle_ret_jump_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "jump",
    "br_if",
    "label",
    "state_label",
    "phi",
    "store_var",
    "delete_var",
    "load_var",
    "copy_var",
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
    first_defined_at: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    last_use: &BTreeMap<String, usize>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    cleanup_roots: &mut NativeCleanupRoots,
    reachable_blocks: &mut BTreeSet<Block>,
    label_blocks: &BTreeMap<i64, Block>,
    label_transport_plans: &BTreeMap<i64, BlockTransportPlan>,
    cfg_liveness: &crate::tir::cfg_liveness::SimpleCfgLiveness,
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
        kind if matches!(
            crate::tir::op_kinds_generated::simpleir_return_shape(kind),
            crate::tir::op_kinds_generated::SimpleIrReturnShape::Value
                | crate::tir::op_kinds_generated::SimpleIrReturnShape::Void
        ) =>
        {
            let return_name = (crate::tir::op_kinds_generated::simpleir_return_shape(kind)
                == crate::tir::op_kinds_generated::SimpleIrReturnShape::Value)
                .then(|| op.args.as_ref().and_then(|args| args.first()))
                .flatten();
            let return_value = if let Some(name) = return_name {
                let value = ensure_boxed_primitive_safe(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    vars,
                    nbc,
                    representation_plan,
                    name,
                );
                if rc_authority.native_value_tracking_enabled() {
                    cleanup_roots.return_owned(builder, local_inc_ref_obj, name, value);
                }
                value
            } else {
                builder.ins().iconst(types::I64, box_none())
            };
            cleanup_roots.release_all(builder, local_dec_ref_obj);
            reachable_blocks.insert(master_return_block);
            if returns_value {
                jump_block(builder, master_return_block, &[return_value]);
            } else {
                jump_block(builder, master_return_block, &[]);
            }
            *is_block_filled = true;
        }
        "jump" => {
            let target_id = op.value.unwrap_or(0);
            let target_block = label_blocks[&target_id];
            if let Some(block) = builder.current_block() {
                let mut carry_obj = block_tracked_obj.remove(&block).unwrap_or_default();
                let cleanup =
                    drain_cleanup_candidates(rc_authority, &mut carry_obj, last_use, op_idx, None);
                for name in cleanup {
                    // The token carries this path's owner across SSA redefinitions.
                    cleanup_roots.release(builder, local_dec_ref_obj, &name);
                }
                if !carry_obj.is_empty() {
                    extend_unique_tracked(
                        block_tracked_obj.entry(target_block).or_default(),
                        carry_obj,
                    );
                }

                let mut carry_ptr = block_tracked_ptr.remove(&block).unwrap_or_default();
                let cleanup =
                    drain_cleanup_candidates(rc_authority, &mut carry_ptr, last_use, op_idx, None);
                for name in cleanup {
                    cleanup_roots.release(builder, local_dec_ref_obj, &name);
                }
                if !carry_ptr.is_empty() {
                    extend_unique_tracked(
                        block_tracked_ptr.entry(target_block).or_default(),
                        carry_ptr,
                    );
                }
            }
            reachable_blocks.insert(target_block);
            let transport_args = label_transport_plans
                .get(&target_id)
                .map(|plan| plan.edge_args(&mut *builder))
                .unwrap_or_default();
            jump_block(&mut *builder, target_block, &transport_args);
            *is_block_filled = true;
        }
        "br_if" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let target_id = op.value.unwrap_or(0);
            let target_block = label_blocks[&target_id];

            let fallthrough_block = builder.create_block();
            let fallthrough_transport = if op_idx + 1 < func_ops.len() {
                let block_id = cfg_liveness.block_for_op(op_idx + 1);
                BlockTransportPlan::from_live_names(
                    &cfg_liveness.live_in_by_block[block_id],
                    vars,
                    representation_plan,
                    slot_backed_join_slots,
                )
            } else {
                BlockTransportPlan::from_live_names(
                    &BTreeSet::new(),
                    vars,
                    representation_plan,
                    slot_backed_join_slots,
                )
            };
            fallthrough_transport.append_block_params(&mut *builder, fallthrough_block);
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
                super::truthiness::emit_boxed_truthiness(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *sealed_blocks,
                    vars,
                    first_defined_at,
                    last_use,
                    op_idx,
                    op.out.as_deref(),
                    list_index_fast_paths,
                    cond_name,
                    *cond,
                    block_tracked_obj,
                    block_tracked_ptr,
                    nbc,
                )
            };

            // Dynamic truthiness expands an internal mini-CFG and carries the
            // origin's cleanup roots to its own merge. The semantic branch must
            // drain that final continuation, not the predecessor that the
            // truthiness authority has already retired.
            let origin_block = builder
                .current_block()
                .expect("br_if requires an active block after condition lowering");

            reachable_blocks.insert(target_block);
            reachable_blocks.insert(fallthrough_block);
            // br_if terminates the current block and can transfer control to either
            // successor. Carry all live tracked values into both.
            let mut carry_obj = block_tracked_obj.remove(&origin_block).unwrap_or_default();
            let cleanup =
                drain_cleanup_candidates(rc_authority, &mut carry_obj, last_use, op_idx, None);
            for name in cleanup {
                cleanup_roots.release(builder, local_dec_ref_obj, &name);
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
            let cleanup =
                drain_cleanup_candidates(rc_authority, &mut carry_ptr, last_use, op_idx, None);
            for name in cleanup {
                cleanup_roots.release(builder, local_dec_ref_obj, &name);
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
            let target_args = label_transport_plans
                .get(&target_id)
                .map(|plan| plan.edge_args(&mut *builder))
                .unwrap_or_default();
            let fallthrough_args = fallthrough_transport.edge_args(&mut *builder);
            brif_block(
                &mut *builder,
                cond_bool,
                target_block,
                &target_args,
                fallthrough_block,
                &fallthrough_args,
            );
            crate::switch_to_block_tracking(
                &mut *builder,
                fallthrough_block,
                &mut *is_block_filled,
            );
            fallthrough_transport.bind_block_params(&mut *builder, fallthrough_block);
            maybe_debug_seal("br_if_fallthrough", op_idx, fallthrough_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, fallthrough_block);
        }
        "label" | "state_label" => {
            let label_id = op.value.unwrap_or(0);
            let block = label_blocks[&label_id];
            let is_function_exception_label = Some(label_id) == function_exception_label_id;
            let transport = label_transport_plans.get(&label_id);

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
                materialize_label_block(&mut *builder, block, &mut *is_block_filled, transport);
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
                materialize_label_block(&mut *builder, block, &mut *is_block_filled, transport);
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
            let binding = simple_ir_binding(op).expect("store_var missing target local");
            let name = binding.destination;
            assert!(
                !name.is_empty() && name != "none",
                "store_var requires a nonempty, non-reserved binding destination"
            );
            let source = op
                .args
                .as_ref()
                .and_then(|args| args.first())
                .expect("store_var source");
            let mut incoming = CapturedScalarTransport::read_local(
                builder,
                vars,
                representation_plan,
                slot_backed_join_slots,
                raw_backed_slot_names,
                source,
            );
            let slot = slot_backed_join_slots.get(name).copied();
            let storage = if slot.is_some() && !raw_backed_slot_names.contains(name) {
                MergeRebindStorageKind::BoxedI64
            } else {
                merge_rebind_storage_for_name(name, representation_plan)
            };
            // Resolve every output from the captured input before publishing
            // either name (the source may itself be one of the outputs).
            let value = incoming.value_for_home(
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                representation_plan,
                nbc,
                name,
                storage,
            );
            let result = binding.result.map(|result| {
                let result_storage = merge_rebind_storage_for_name(result, representation_plan);
                let result_value = incoming.value_for_home(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    representation_plan,
                    nbc,
                    result,
                    result_storage,
                );
                (result, result_storage, result_value)
            });
            let native_rc = rc_authority.native_value_tracking_enabled();
            let owns_destination = native_rc
                && storage == MergeRebindStorageKind::BoxedI64
                && (slot.is_some() || cleanup_roots.contains(name));
            let owns_result = native_rc
                && result.is_some_and(|(result, storage, _)| {
                    storage == MergeRebindStorageKind::BoxedI64
                        && cleanup_roots.contains(result)
                        && native_alias_mints_owner(alias_roots, source, result)
                });
            // One raw-to-boxed materialization supplies one credit. Additional
            // independent owners retain that same object, never box it again.
            // Acquire all credits before any displaced owner can be released.
            if owns_destination && !incoming.take_boxed_owner() {
                emit_inc_ref_obj(builder, value, local_inc_ref_obj);
            }
            if owns_result && !incoming.take_boxed_owner() {
                emit_inc_ref_obj(
                    builder,
                    result.expect("snapshot owner").2,
                    local_inc_ref_obj,
                );
            }
            let displaced = if native_rc && storage == MergeRebindStorageKind::BoxedI64 {
                slot.map(|slot| builder.ins().stack_load(types::I64, slot, 0))
            } else {
                None
            };
            if let Some(slot) = slot {
                builder.ins().stack_store(value, slot, 0);
            } else {
                def_var_from_merge_rebind_storage(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    name,
                    value,
                    storage,
                );
            }
            if let Some((result, result_storage, result_value)) = result {
                def_var_from_merge_rebind_storage(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    vars,
                    representation_plan,
                    nbc,
                    result,
                    result_value,
                    result_storage,
                );
            }
            // Explicit TIR ownership remains authoritative in drop-inserted
            // functions. Native owners publish before displaced cleanup.
            if owns_result {
                let (result, _, result_value) = result.expect("snapshot owner");
                cleanup_roots.acquire(builder, local_dec_ref_obj, result, result_value);
            }
            if let Some(displaced) = displaced {
                builder.ins().call(local_dec_ref_obj, &[displaced]);
            } else if owns_destination {
                cleanup_roots.acquire(builder, local_dec_ref_obj, name, value);
            }
            if let Some(block) = builder.current_block() {
                let tracked = block_tracked_obj.entry(block).or_default();
                if owns_destination && slot.is_none() {
                    extend_unique_tracked(tracked, vec![name.to_string()]);
                }
                if let Some((result, MergeRebindStorageKind::BoxedI64, _)) = result {
                    extend_unique_tracked(tracked, vec![result.to_string()]);
                }
            }
            return OpFlow::Continue;
        }
        "delete_var" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let Some(binding) = simple_ir_binding(op) else {
                panic!("delete_var missing target local");
            };
            let name = binding.destination;
            assert!(
                !name.is_empty() && name != "none",
                "delete_var requires a nonempty, non-reserved binding destination"
            );
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
            if rc_authority.native_value_tracking_enabled() {
                if slot_backed_join_slots.contains_key(name) {
                    builder.ins().call(local_dec_ref_obj, &[old_val]);
                } else {
                    cleanup_roots.release(builder, local_dec_ref_obj, name);
                }
            }
            return OpFlow::Continue;
        }
        "load_var" | "copy_var" => {
            let source = preanalyze_alias_source(op).expect("variable load source");
            let Some(out) = op.out.as_deref().filter(|out| *out != "none") else {
                return OpFlow::Continue;
            };
            let mut incoming = CapturedScalarTransport::read_local(
                builder,
                vars,
                representation_plan,
                slot_backed_join_slots,
                raw_backed_slot_names,
                source,
            );
            let storage = merge_rebind_storage_for_name(out, representation_plan);
            let value = incoming.value_for_home(
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                representation_plan,
                nbc,
                out,
                storage,
            );
            let owns_result = rc_authority.native_value_tracking_enabled()
                && storage == MergeRebindStorageKind::BoxedI64
                && cleanup_roots.contains(out)
                && native_alias_mints_owner(alias_roots, source, out);
            if owns_result && !incoming.take_boxed_owner() {
                emit_inc_ref_obj(builder, value, local_inc_ref_obj);
            }
            def_var_from_merge_rebind_storage(
                module,
                import_ids,
                builder,
                import_refs,
                vars,
                representation_plan,
                nbc,
                out,
                value,
                storage,
            );
            if owns_result {
                cleanup_roots.acquire(builder, local_dec_ref_obj, out, value);
            }
            if storage == MergeRebindStorageKind::BoxedI64
                && let Some(block) = builder.current_block()
            {
                extend_unique_tracked(
                    block_tracked_obj.entry(block).or_default(),
                    vec![out.to_string()],
                );
            }
            return OpFlow::Continue;
        }
        _ => unreachable!("handle_ret_jump_op received non-ret/jump op `{}`", op.kind),
    }

    OpFlow::Proceed
}
