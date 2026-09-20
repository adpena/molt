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
            // Boxed storage owns a retained binding. Publication precedes
            // release of the displaced occupant on every path, including loops.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            if let Some(binding) = simple_ir_binding(op) {
                let name = binding.destination;
                assert!(
                    !name.is_empty() && name != "none",
                    "store_var requires a nonempty, non-reserved binding destination"
                );
                // A result-carrying store has two definitions: the mutable
                // destination and an SSA alias of its source. Preserve both;
                // generated field roles, not out.or(var), own that distinction.
                if let Some(result) = binding.result {
                    let source = args.first().expect("store_var source");
                    let value = var_get_boxed_overflow_safe(
                        module,
                        import_ids,
                        builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        source,
                        representation_plan,
                    )
                    .expect("store_var result source");
                    if rc_authority.native_value_tracking_enabled()
                        && native_alias_mints_owner(alias_roots, source, result)
                    {
                        if cleanup_roots.contains(result)
                            && merge_rebind_storage_for_name(source, representation_plan)
                                == MergeRebindStorageKind::BoxedI64
                        {
                            emit_inc_ref_obj(builder, *value, local_inc_ref_obj);
                        }
                        cleanup_roots.acquire(builder, local_dec_ref_obj, result, *value);
                    }
                    def_var_from_boxed_transport(
                        module,
                        import_ids,
                        builder,
                        import_refs,
                        vars,
                        representation_plan,
                        nbc,
                        result,
                        *value,
                    );
                    if let Some(block) = builder.current_block() {
                        extend_unique_tracked(
                            block_tracked_obj.entry(block).or_default(),
                            vec![result.to_string()],
                        );
                    }
                }
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
                    if merge_rebind_storage_for_name(&args[0], representation_plan)
                        == MergeRebindStorageKind::BoxedI64
                    {
                        emit_inc_ref_obj(builder, *val, local_inc_ref_obj);
                    }
                    builder.ins().stack_store(*val, slot, 0);
                    builder.ins().call(local_dec_ref_obj, &[old]);
                    return OpFlow::Continue;
                }
                // A mutable binding owns a reference independently of its source.
                // NativeCleanupRoots publishes/replaces this owner after the store;
                // the TIR lane supplies its own explicit ownership operations.
                if rc_authority.native_value_tracking_enabled()
                    && cleanup_roots.contains(name)
                    && merge_rebind_storage_for_name(&args[0], representation_plan)
                        == MergeRebindStorageKind::BoxedI64
                {
                    emit_inc_ref_obj(builder, *val, local_inc_ref_obj);
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
                if rc_authority.native_value_tracking_enabled() {
                    cleanup_roots.acquire(builder, local_dec_ref_obj, name, *val);
                    if let Some(block) = builder.current_block() {
                        extend_unique_tracked(
                            block_tracked_obj.entry(block).or_default(),
                            vec![name.to_string()],
                        );
                    }
                }
                return OpFlow::Continue;
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
                                if rc_authority.native_value_tracking_enabled() {
                                    cleanup_roots.acquire(
                                        builder,
                                        local_dec_ref_obj,
                                        out_name,
                                        boxed,
                                    );
                                }
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
                    if rc_authority.native_value_tracking_enabled()
                        && op
                            .out
                            .as_deref()
                            .is_some_and(|out| cleanup_roots.contains(out))
                    {
                        emit_inc_ref_obj(builder, val, local_inc_ref_obj);
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
                        if rc_authority.native_value_tracking_enabled() {
                            cleanup_roots.acquire(builder, local_dec_ref_obj, out_name, val);
                        }
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
                    let source = preanalyze_alias_source(op).expect("variable load source");
                    if rc_authority.native_value_tracking_enabled()
                        && native_alias_mints_owner(alias_roots, source, out_name)
                        && cleanup_roots.contains(out_name)
                        && merge_rebind_storage_for_name(source, representation_plan)
                            == MergeRebindStorageKind::BoxedI64
                    {
                        emit_inc_ref_obj(builder, *val, local_inc_ref_obj);
                    }
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
                                if rc_authority.native_value_tracking_enabled() {
                                    cleanup_roots.acquire(
                                        builder,
                                        local_dec_ref_obj,
                                        out_name,
                                        boxed,
                                    );
                                }
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
                    if rc_authority.native_value_tracking_enabled()
                        && op
                            .out
                            .as_deref()
                            .is_some_and(|out| cleanup_roots.contains(out))
                    {
                        emit_inc_ref_obj(builder, val, local_inc_ref_obj);
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
                        if rc_authority.native_value_tracking_enabled() {
                            cleanup_roots.acquire(builder, local_dec_ref_obj, out_name, val);
                        }
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
                    let source = preanalyze_alias_source(op).expect("variable load source");
                    if rc_authority.native_value_tracking_enabled()
                        && native_alias_mints_owner(alias_roots, source, out_name)
                        && cleanup_roots.contains(out_name)
                        && merge_rebind_storage_for_name(source, representation_plan)
                            == MergeRebindStorageKind::BoxedI64
                    {
                        emit_inc_ref_obj(builder, *val, local_inc_ref_obj);
                    }
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
        _ => unreachable!("handle_ret_jump_op received non-ret/jump op `{}`", op.kind),
    }

    OpFlow::Proceed
}
