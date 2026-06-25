use super::super::*;

/// Single-source kind authority for [`handle_memory_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "alloc",
    "stack_alloc",
    "alloc_class",
    "alloc_class_trusted",
    "alloc_class_static",
    "alloc_task",
    "store",
    "store_init",
    "load",
    "closure_load",
    "closure_store",
    "guarded_load",
    "guarded_field_get",
    "guarded_field_set",
    "guarded_field_init",
    "guard_type",
    "guard_tag",
    "guard_layout",
    "guard_dict_shape",
];
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for memory, allocation, field access, and guard ops.
///
/// Extracted from `compile_func_inner`'s per-op dispatch (M1.7). The arm
/// bodies preserve the original lowering semantics; only access paths change
/// from backend fields to explicit split-borrowed parameters, and outer op-loop
/// `continue` becomes `OpFlow::Continue`.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_memory_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    int_carriers_plan: &ScalarRepresentationPlan,
    float_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    int_like_vars: &BTreeSet<String>,
    float_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    str_like_vars: &BTreeSet<String>,
    param_name_set: &BTreeSet<&str>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    field_store_modes: &BTreeMap<usize, FieldStoreMode>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    entry_vars: &mut BTreeMap<String, Value>,
    already_decrefed: &mut BTreeSet<String>,
    defined_functions: &BTreeSet<String>,
    scope_arena_ptr: Option<Value>,
    output_is_ptr: &mut bool,
    stateful: bool,
    entry_block: Block,
    local_profile_struct: Option<FuncRef>,
    profile_enabled_val: Option<Value>,
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
    native_rc_tracking_enabled: bool,
    scalar_fast_paths_enabled: bool,
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
        "alloc" | "stack_alloc" => {
            let size = op.value.unwrap_or(0);
            let iconst = builder.ins().iconst(types::I64, size);

            // Scope arena path: NoEscape allocs use the bump
            // allocator for O(1) allocation + O(1) bulk free.
            // `molt_arena_alloc_object` mirrors `molt_alloc`'s
            // contract: takes payload size, returns NaN-boxed bits
            // with an initialized MoltHeader (refcount 1, ARENA flag
            // set so dec_ref skips the global allocator).
            let is_arena = op.arena_eligible == Some(true) && scope_arena_ptr.is_some();
            let res = if is_arena {
                let arena_ptr = scope_arena_ptr.unwrap();
                let arena_alloc_id = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_arena_alloc_object",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_arena_alloc = module.declare_func_in_func(arena_alloc_id, builder.func);
                let call = builder.ins().call(local_arena_alloc, &[arena_ptr, iconst]);
                builder.inst_results(call)[0]
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_alloc",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[iconst]);
                builder.inst_results(call)[0]
            };
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        "alloc_class" => {
            let size = op.value.unwrap_or(0);
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let class_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Class not found");
            let iconst = builder.ins().iconst(types::I64, size);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_alloc_class",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
            let res = builder.inst_results(call)[0];
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        "alloc_class_trusted" => {
            let size = op.value.unwrap_or(0);
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let class_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Class not found");
            let iconst = builder.ins().iconst(types::I64, size);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_alloc_class_trusted",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
            let res = builder.inst_results(call)[0];
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        "alloc_class_static" => {
            let size = op.value.unwrap_or(0);
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let class_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Class not found");
            let iconst = builder.ins().iconst(types::I64, size);

            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_alloc_class_static",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
            let res = builder.inst_results(call)[0];
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        "alloc_task" => {
            let closure_size = op.value.unwrap_or(0);
            let task_kind = op.task_kind.as_deref().unwrap_or("future");
            let (kind_bits, payload_base) = match task_kind {
                "generator" => (TASK_KIND_GENERATOR, GENERATOR_CONTROL_BYTES),
                "future" => (TASK_KIND_FUTURE, 0),
                "coroutine" => (TASK_KIND_COROUTINE, 0),
                _ => panic!("unknown task kind: {task_kind}"),
            };
            let size = builder.ins().iconst(types::I64, closure_size);

            let Some(poll_func_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let mut poll_sig = module.make_signature();
            poll_sig.params.push(AbiParam::new(types::I64));
            poll_sig.returns.push(AbiParam::new(types::I64));

            let poll_linkage = if defined_functions.contains(poll_func_name.as_str()) {
                Linkage::Export
            } else {
                Linkage::Import
            };
            let poll_func_id = module
                .declare_function(poll_func_name, poll_linkage, &poll_sig)
                .unwrap();
            let poll_func_ref = module.declare_func_in_func(poll_func_id, builder.func);
            let poll_addr = builder.ins().func_addr(types::I64, poll_func_ref);

            let task_callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_task_new",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let task_local = module.declare_func_in_func(task_callee, builder.func);
            let kind_val = builder.ins().iconst(types::I64, kind_bits);
            let call = builder.ins().call(task_local, &[poll_addr, size, kind_val]);
            let obj = builder.inst_results(call)[0];
            let obj_ptr = unbox_ptr_value(&mut *builder, obj, nbc);
            if let Some(args_names) = &op.args {
                for (i, name) in args_names.iter().enumerate() {
                    let arg_val = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        name,
                        int_carriers_plan,
                        float_primary_vars,
                    )
                    .expect("Arg not found for alloc_task");
                    let offset = payload_base + (i * 8) as i32;
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), *arg_val, obj_ptr, offset);
                    emit_maybe_ref_adjust_v2(&mut *builder, *arg_val, local_inc_ref_obj, nbc);
                }
            }
            if matches!(task_kind, "future" | "coroutine") {
                let get_callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_cancel_token_get_current",
                    &[],
                    &[types::I64],
                );
                let get_local = module.declare_func_in_func(get_callee, builder.func);
                let get_call = builder.ins().call(get_local, &[]);
                let current_token = builder.inst_results(get_call)[0];

                let reg_callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_task_register_token_owned",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let reg_local = module.declare_func_in_func(reg_callee, builder.func);
                builder.ins().call(reg_local, &[obj, current_token]);
            }

            *output_is_ptr = false;
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, obj);
        }
        "store" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let origin_block = builder
                .current_block()
                .expect("store requires an active block");
            let mut origin_obj_live = block_tracked_obj.remove(&origin_block).unwrap_or_default();
            let origin_obj_cleanup = drain_cleanup_tracked_dedup_with_authority(
                native_rc_tracking_enabled,
                &mut origin_obj_live,
                last_use,
                alias_roots,
                op_idx,
                None,
                Some(&mut *already_decrefed),
            );
            let mut origin_ptr_live = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
            let origin_ptr_cleanup = drain_cleanup_tracked_dedup_with_authority(
                native_rc_tracking_enabled,
                &mut origin_ptr_live,
                last_use,
                alias_roots,
                op_idx,
                None,
                Some(&mut *already_decrefed),
            );
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Object not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Value not found");
            let offset = op.value.unwrap_or(0) as i32;
            let obj_ptr = unbox_ptr_value(&mut *builder, *obj, nbc);
            let field_store_mode = field_store_modes.get(&op_idx).copied();
            if field_store_mode == Some(FieldStoreMode::DirectNonHeap) {
                // Defense-in-depth (#50): the inlined-constructor direct
                // field store writes `*(obj_ptr + offset) = val` with no
                // header read, but `obj_ptr` is garbage/NULL when the
                // preceding allocation returned the None/exception sentinel
                // (a bad `cls_bits` into `object_new_bound`). Writing to it
                // corrupts memory / faults. Guard on `tag(obj) == TAG_PTR`;
                // when the receiver is not a live pointer the alloc-failure
                // pending exception is already set, so skip the store and
                // let the post-construction `check_exception` raise the
                // clean `TypeError`. A real instance takes the single
                // predictable-taken branch, preserving fast-path speed.
                let dfs_tag_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
                let dfs_tag_bits = builder.ins().band(*obj, dfs_tag_mask);
                let dfs_ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
                let dfs_is_ptr = builder.ins().icmp(IntCC::Equal, dfs_tag_bits, dfs_ptr_tag);
                let dfs_store_block = builder.create_block();
                let dfs_cont_block = builder.create_block();
                if let Some(current_block) = builder.current_block() {
                    builder.insert_block_after(dfs_store_block, current_block);
                    builder.insert_block_after(dfs_cont_block, dfs_store_block);
                }
                builder
                    .ins()
                    .brif(dfs_is_ptr, dfs_store_block, &[], dfs_cont_block, &[]);
                switch_to_block_materialized(&mut *builder, dfs_store_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, dfs_store_block);
                builder
                    .ins()
                    .store(MemFlagsData::trusted(), *val, obj_ptr, offset);
                jump_block(&mut *builder, dfs_cont_block, &[]);
                switch_to_block_materialized(&mut *builder, dfs_cont_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, dfs_cont_block);
                for name in origin_obj_cleanup {
                    if cleanup_name_excluded(
                        &name,
                        None,
                        param_name_set,
                        int_carriers_plan,
                        float_primary_vars,
                    ) {
                        continue;
                    }
                    if let Some(cleanup_val) = entry_vars.get(&name).copied().or_else(|| {
                        var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            &mut *sealed_blocks,
                            vars,
                            &name,
                            int_carriers_plan,
                            float_primary_vars,
                        )
                        .map(|v| *v)
                    }) {
                        builder.ins().call(local_dec_ref_obj, &[cleanup_val]);
                    }
                }
                for name in origin_ptr_cleanup {
                    if cleanup_name_excluded(
                        &name,
                        None,
                        param_name_set,
                        int_carriers_plan,
                        float_primary_vars,
                    ) {
                        continue;
                    }
                    if let Some(cleanup_val) = entry_vars.get(&name).copied().or_else(|| {
                        var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            &mut *sealed_blocks,
                            vars,
                            &name,
                            int_carriers_plan,
                            float_primary_vars,
                        )
                        .map(|v| *v)
                    }) {
                        builder.ins().call(local_dec_ref_obj, &[cleanup_val]);
                    }
                }
                if !origin_obj_live.is_empty() {
                    extend_unique_tracked(
                        block_tracked_obj.entry(origin_block).or_default(),
                        origin_obj_live,
                    );
                }
                if !origin_ptr_live.is_empty() {
                    extend_unique_tracked(
                        block_tracked_ptr.entry(origin_block).or_default(),
                        origin_ptr_live,
                    );
                }
                if let Some(out_name) = op.out.as_ref()
                    && out_name != "none"
                {
                    let none_val = builder.ins().iconst(types::I64, box_none());
                    def_var_named(&mut *builder, vars, out_name.clone(), none_val);
                }
                return OpFlow::Continue;
            }

            let local_profile_struct =
                local_profile_struct.expect("store lowering requires profile import");
            let profile_enabled_val =
                profile_enabled_val.expect("store lowering requires profile flag");

            // Profile hook: gated on profile_enabled_val so it's
            // a single branch when profiling is off.
            let profile_block = builder.create_block();
            let profile_cont = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(profile_block, current_block);
                builder.insert_block_after(profile_cont, profile_block);
            }
            let profile_bool = builder
                .ins()
                .icmp_imm(IntCC::NotEqual, profile_enabled_val, 0);
            builder
                .ins()
                .brif(profile_bool, profile_block, &[], profile_cont, &[]);
            switch_to_block_materialized(&mut *builder, profile_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, profile_block);
            builder.ins().call(local_profile_struct, &[]);
            jump_block(&mut *builder, profile_cont, &[]);
            switch_to_block_materialized(&mut *builder, profile_cont);
            seal_block_once(&mut *builder, &mut *sealed_blocks, profile_cont);

            // Fast path: when (HEADER_FLAG_HAS_PTRS is clear)
            // AND (new value is immediate), emit a direct
            // memory write at obj_ptr + offset, skipping the
            // `molt_object_field_set_ptr` runtime call.
            //
            // Soundness rests on three invariants from the
            // runtime contract:
            //  1. `HEADER_FLAG_HAS_PTRS` is set whenever any
            //     pointer is stored into ANY slot of the
            //     object — including the trailing `__dict__`
            //     slot, after the `instance_set_dict_bits`
            //     change in `runtime/molt-runtime/src/object/mod.rs`.
            //     With the flag clear, every slot holds an
            //     immediate (or zero) and no live pointer
            //     needs decref.
            //  2. The new value being immediate (tag != PTR)
            //     means no `inc_ref` is needed for the new
            //     content.  When the flag is clear AND the
            //     new value is immediate, the runtime helper
            //     would just have written `*slot = val` and
            //     called `sync_materialized_instance_dict_for_field_offset`,
            //     which itself early-returns when
            //     `instance_dict_bits == 0` (held by
            //     invariant 1).
            //  3. `unbox_ptr_value` returns a pointer that
            //     is past the `MoltHeader` (it points to the
            //     payload start; see `lib.rs:1480` and the
            //     `header_from_obj_ptr` helper at
            //     `runtime/molt-runtime/src/object/mod.rs:1185`
            //     which subtracts `size_of::<MoltHeader>()`
            //     to get back to the header).  `MoltHeader`
            //     is 24 bytes (`object/mod.rs:289-298`) with
            //     `flags: u32` at field offset 8, so the
            //     absolute offset of `flags` from `obj_ptr`
            //     (which points to payload start) is
            //     `-24 + 8 = -16`.  `HEADER_FLAG_HAS_PTRS = 1`
            //     (bit 0; same file at line 423).
            //
            // The slow path remains the existing runtime
            // call and is reached on flag-set or pointer
            // value — the runtime helper handles decref of
            // the old slot, inc_ref of the new value, the
            // has-ptrs flag transition, and dict sync.
            const MOLT_HEADER_FLAGS_OFFSET_FROM_PAYLOAD: i32 = -16;
            const HEADER_FLAG_HAS_PTRS: i64 = 1;

            // Defense-in-depth (#50): never dereference the object header
            // when the receiver is not a live heap pointer. A failed
            // allocation upstream (e.g. `object_new_bound` fed a bad
            // `cls_bits`) returns the None/exception sentinel, whose
            // NaN-box tag is NOT `TAG_PTR`; `unbox_ptr_value` then yields a
            // garbage/NULL address. Reading `flags = *(obj_ptr - 16)` below
            // on that address SIGSEGVs (the observed #50 crash). The
            // alloc-failure path has already set a pending exception, so the
            // correct behavior is to SKIP the store entirely and let the
            // post-construction `check_exception` raise the clean
            // `TypeError` — exactly what the slow path produces. Guard on
            // `tag(obj) == TAG_PTR`; a real instance (the overwhelmingly
            // common case) takes the single predictable-taken branch into
            // the store, so the fast path stays fast.
            let obj_tag_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
            let obj_tag_bits = builder.ins().band(*obj, obj_tag_mask);
            let obj_ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
            let obj_is_ptr = builder.ins().icmp(IntCC::Equal, obj_tag_bits, obj_ptr_tag);

            let store_block = builder.create_block();
            let merge_block = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(store_block, current_block);
                builder.insert_block_after(merge_block, store_block);
            }
            // Skip straight to merge when the receiver is not a pointer
            // (pending exception propagates to the next check_exception).
            builder
                .ins()
                .brif(obj_is_ptr, store_block, &[], merge_block, &[]);
            switch_to_block_materialized(&mut *builder, store_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, store_block);

            let flags_val = builder.ins().load(
                types::I32,
                MemFlagsData::trusted(),
                obj_ptr,
                MOLT_HEADER_FLAGS_OFFSET_FROM_PAYLOAD,
            );
            let flags_64 = builder.ins().uextend(types::I64, flags_val);
            let has_ptrs_bit = builder.ins().band_imm(flags_64, HEADER_FLAG_HAS_PTRS);
            let has_ptrs_set = builder.ins().icmp_imm(IntCC::NotEqual, has_ptrs_bit, 0);

            let tag_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
            let tag_bits = builder.ins().band(*val, tag_mask);
            let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
            let new_is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);

            let go_slow = builder.ins().bor(has_ptrs_set, new_is_ptr);

            let fast_block = builder.create_block();
            let slow_block = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(fast_block, current_block);
                builder.insert_block_after(slow_block, fast_block);
            }
            builder.set_cold_block(slow_block);
            builder
                .ins()
                .brif(go_slow, slow_block, &[], fast_block, &[]);

            // Fast path: direct store at obj_ptr + offset, no
            // GIL acquire, no runtime call.
            switch_to_block_materialized(&mut *builder, fast_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
            builder
                .ins()
                .store(MemFlagsData::trusted(), *val, obj_ptr, offset);
            jump_block(&mut *builder, merge_block, &[]);

            // Slow path: existing runtime helper handles all
            // refcount + dict sync semantics.
            switch_to_block_materialized(&mut *builder, slow_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
            let offset_bits = builder.ins().iconst(types::I64, i64::from(offset));
            let helper_name = if field_store_mode == Some(FieldStoreMode::FreshInit) {
                "molt_object_field_init_ptr"
            } else {
                "molt_object_field_set_ptr"
            };
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                helper_name,
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder
                .ins()
                .call(local_callee, &[obj_ptr, offset_bits, *val]);
            jump_block(&mut *builder, merge_block, &[]);

            // Merge: continue downstream IR.  The runtime
            // helper returns `MoltObject::none().bits()`; the
            // fast path returns nothing, but `out` (when
            // present) is bound to box_none() which is
            // structurally identical to the helper's return
            // for the "side-effect, no value" contract.
            switch_to_block_materialized(&mut *builder, merge_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
            if !origin_obj_live.is_empty() {
                extend_unique_tracked(
                    block_tracked_obj.entry(merge_block).or_default(),
                    origin_obj_live,
                );
            }
            if !origin_ptr_live.is_empty() {
                extend_unique_tracked(
                    block_tracked_ptr.entry(merge_block).or_default(),
                    origin_ptr_live,
                );
            }
            for name in origin_obj_cleanup {
                if cleanup_name_excluded(
                    &name,
                    None,
                    param_name_set,
                    int_carriers_plan,
                    float_primary_vars,
                ) {
                    continue;
                }
                if let Some(cleanup_val) = entry_vars.get(&name).copied().or_else(|| {
                    var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &name,
                        int_carriers_plan,
                        float_primary_vars,
                    )
                    .map(|v| *v)
                }) {
                    builder.ins().call(local_dec_ref_obj, &[cleanup_val]);
                }
            }
            for name in origin_ptr_cleanup {
                if cleanup_name_excluded(
                    &name,
                    None,
                    param_name_set,
                    int_carriers_plan,
                    float_primary_vars,
                ) {
                    continue;
                }
                if let Some(cleanup_val) = entry_vars.get(&name).copied().or_else(|| {
                    var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &name,
                        int_carriers_plan,
                        float_primary_vars,
                    )
                    .map(|v| *v)
                }) {
                    builder.ins().call(local_dec_ref_obj, &[cleanup_val]);
                }
            }
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                let none_val = builder.ins().iconst(types::I64, box_none());
                def_var_named(&mut *builder, vars, out_name.clone(), none_val);
            }
        }
        "store_init" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let origin_block = builder
                .current_block()
                .expect("store_init requires an active block");
            let mut origin_obj_live = block_tracked_obj.remove(&origin_block).unwrap_or_default();
            let origin_obj_cleanup = drain_cleanup_tracked_dedup_with_authority(
                native_rc_tracking_enabled,
                &mut origin_obj_live,
                last_use,
                alias_roots,
                op_idx,
                None,
                Some(&mut *already_decrefed),
            );
            let mut origin_ptr_live = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
            let origin_ptr_cleanup = drain_cleanup_tracked_dedup_with_authority(
                native_rc_tracking_enabled,
                &mut origin_ptr_live,
                last_use,
                alias_roots,
                op_idx,
                None,
                Some(&mut *already_decrefed),
            );
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Object not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Value not found");
            let offset = op.value.unwrap_or(0) as i32;
            let obj_ptr = unbox_ptr_value(&mut *builder, *obj, nbc);
            if field_store_modes.get(&op_idx).copied() == Some(FieldStoreMode::DirectNonHeap) {
                // Defense-in-depth (#50): the inlined-constructor direct
                // field store writes `*(obj_ptr + offset) = val` with no
                // header read, but `obj_ptr` is garbage/NULL when the
                // preceding allocation returned the None/exception sentinel
                // (a bad `cls_bits` into `object_new_bound`). Writing to it
                // corrupts memory / faults. Guard on `tag(obj) == TAG_PTR`;
                // when the receiver is not a live pointer the alloc-failure
                // pending exception is already set, so skip the store and
                // let the post-construction `check_exception` raise the
                // clean `TypeError`. A real instance takes the single
                // predictable-taken branch, preserving fast-path speed.
                let dfs_tag_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
                let dfs_tag_bits = builder.ins().band(*obj, dfs_tag_mask);
                let dfs_ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
                let dfs_is_ptr = builder.ins().icmp(IntCC::Equal, dfs_tag_bits, dfs_ptr_tag);
                let dfs_store_block = builder.create_block();
                let dfs_cont_block = builder.create_block();
                if let Some(current_block) = builder.current_block() {
                    builder.insert_block_after(dfs_store_block, current_block);
                    builder.insert_block_after(dfs_cont_block, dfs_store_block);
                }
                builder
                    .ins()
                    .brif(dfs_is_ptr, dfs_store_block, &[], dfs_cont_block, &[]);
                switch_to_block_materialized(&mut *builder, dfs_store_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, dfs_store_block);
                builder
                    .ins()
                    .store(MemFlagsData::trusted(), *val, obj_ptr, offset);
                jump_block(&mut *builder, dfs_cont_block, &[]);
                switch_to_block_materialized(&mut *builder, dfs_cont_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, dfs_cont_block);
                for name in origin_obj_cleanup {
                    if cleanup_name_excluded(
                        &name,
                        None,
                        param_name_set,
                        int_carriers_plan,
                        float_primary_vars,
                    ) {
                        continue;
                    }
                    if let Some(cleanup_val) = entry_vars.get(&name).copied().or_else(|| {
                        var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            &mut *sealed_blocks,
                            vars,
                            &name,
                            int_carriers_plan,
                            float_primary_vars,
                        )
                        .map(|v| *v)
                    }) {
                        builder.ins().call(local_dec_ref_obj, &[cleanup_val]);
                    }
                }
                for name in origin_ptr_cleanup {
                    if cleanup_name_excluded(
                        &name,
                        None,
                        param_name_set,
                        int_carriers_plan,
                        float_primary_vars,
                    ) {
                        continue;
                    }
                    if let Some(cleanup_val) = entry_vars.get(&name).copied().or_else(|| {
                        var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            &mut *import_refs,
                            &mut *sealed_blocks,
                            vars,
                            &name,
                            int_carriers_plan,
                            float_primary_vars,
                        )
                        .map(|v| *v)
                    }) {
                        builder.ins().call(local_dec_ref_obj, &[cleanup_val]);
                    }
                }
                if !origin_obj_live.is_empty() {
                    extend_unique_tracked(
                        block_tracked_obj.entry(origin_block).or_default(),
                        origin_obj_live,
                    );
                }
                if !origin_ptr_live.is_empty() {
                    extend_unique_tracked(
                        block_tracked_ptr.entry(origin_block).or_default(),
                        origin_ptr_live,
                    );
                }
                if let Some(out_name) = op.out.as_ref()
                    && out_name != "none"
                {
                    let none_val = builder.ins().iconst(types::I64, box_none());
                    def_var_named(&mut *builder, vars, out_name.clone(), none_val);
                }
                return OpFlow::Continue;
            }
            // Inline the field init for immediate values (int/float/
            // bool/none): just store to obj_ptr + offset with no GIL
            // acquire and no function call. For heap-pointer values
            // we must call the runtime to inc_ref + mark_has_ptrs.
            //
            // Defense-in-depth (#50): both the inline store and the runtime
            // `molt_object_field_init_ptr` dereference `obj_ptr`, which is
            // garbage/NULL when the preceding allocation returned the
            // None/exception sentinel. Guard on `tag(obj) == TAG_PTR` and
            // skip the init when the receiver is not a live pointer (the
            // alloc-failure pending exception then surfaces at the next
            // `check_exception` as the clean `TypeError`). Mirrors the
            // `store` go-slow guard above.
            let init_obj_tag_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
            let init_obj_tag_bits = builder.ins().band(*obj, init_obj_tag_mask);
            let init_obj_ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
            let init_obj_is_ptr =
                builder
                    .ins()
                    .icmp(IntCC::Equal, init_obj_tag_bits, init_obj_ptr_tag);
            let init_guard_block = builder.create_block();
            let merge_block = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(init_guard_block, current_block);
                builder.insert_block_after(merge_block, init_guard_block);
            }
            builder
                .ins()
                .brif(init_obj_is_ptr, init_guard_block, &[], merge_block, &[]);
            switch_to_block_materialized(&mut *builder, init_guard_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, init_guard_block);
            // Check if val is a heap pointer:
            //   (val & TAG_MASK) == TAG_PTR
            let tag_mask = builder.ins().iconst(types::I64, nbc.qnan_tag_mask);
            let tag_bits = builder.ins().band(*val, tag_mask);
            let ptr_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_ptr);
            let is_ptr = builder.ins().icmp(IntCC::Equal, tag_bits, ptr_tag);
            let fast_block = builder.create_block();
            let slow_block = builder.create_block();
            if let Some(current_block) = builder.current_block() {
                builder.insert_block_after(fast_block, current_block);
                builder.insert_block_after(slow_block, fast_block);
            }
            builder.set_cold_block(slow_block);
            builder.ins().brif(is_ptr, slow_block, &[], fast_block, &[]);
            // Fast path: immediate value — direct store, no GIL.
            switch_to_block_materialized(&mut *builder, fast_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
            builder
                .ins()
                .store(MemFlagsData::trusted(), *val, obj_ptr, offset);
            jump_block(&mut *builder, merge_block, &[]);
            // Slow path: pointer value — call runtime for inc_ref + mark_has_ptrs + store.
            switch_to_block_materialized(&mut *builder, slow_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
            let offset_bits = builder.ins().iconst(types::I64, i64::from(offset));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_object_field_init_ptr",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            builder
                .ins()
                .call(local_callee, &[obj_ptr, offset_bits, *val]);
            jump_block(&mut *builder, merge_block, &[]);
            // Merge: continue.
            switch_to_block_materialized(&mut *builder, merge_block);
            seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
            if !origin_obj_live.is_empty() {
                extend_unique_tracked(
                    block_tracked_obj.entry(merge_block).or_default(),
                    origin_obj_live,
                );
            }
            if !origin_ptr_live.is_empty() {
                extend_unique_tracked(
                    block_tracked_ptr.entry(merge_block).or_default(),
                    origin_ptr_live,
                );
            }
            for name in origin_obj_cleanup {
                if cleanup_name_excluded(
                    &name,
                    None,
                    param_name_set,
                    int_carriers_plan,
                    float_primary_vars,
                ) {
                    continue;
                }
                if let Some(cleanup_val) = entry_vars.get(&name).copied().or_else(|| {
                    var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &name,
                        int_carriers_plan,
                        float_primary_vars,
                    )
                    .map(|v| *v)
                }) {
                    builder.ins().call(local_dec_ref_obj, &[cleanup_val]);
                }
            }
            for name in origin_ptr_cleanup {
                if cleanup_name_excluded(
                    &name,
                    None,
                    param_name_set,
                    int_carriers_plan,
                    float_primary_vars,
                ) {
                    continue;
                }
                if let Some(cleanup_val) = entry_vars.get(&name).copied().or_else(|| {
                    var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &name,
                        int_carriers_plan,
                        float_primary_vars,
                    )
                    .map(|v| *v)
                }) {
                    builder.ins().call(local_dec_ref_obj, &[cleanup_val]);
                }
            }
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                let none_val = builder.ins().iconst(types::I64, box_none());
                def_var_named(&mut *builder, vars, out_name.clone(), none_val);
            }
        }
        "load" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Object not found");
            let offset_val = op.value.unwrap_or(0);
            let res = emit_guarded_object_field_get(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                *obj,
                offset_val,
                nbc,
            );
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        "closure_load" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
            let obj_ptr = if stateful && args.first().map(String::as_str) == Some("self") {
                builder.block_params(entry_block)[0]
            } else {
                let obj = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    int_carriers_plan,
                    float_primary_vars,
                )
                .expect("Object not found");
                unbox_ptr_value(&mut *builder, *obj, nbc)
            };
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_closure_load",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[obj_ptr, offset]);
            let res = builder.inst_results(call)[0];
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        "closure_store" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Value not found");
            let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
            let obj_ptr = if stateful && args.first().map(String::as_str) == Some("self") {
                builder.block_params(entry_block)[0]
            } else {
                let obj = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    int_carriers_plan,
                    float_primary_vars,
                )
                .expect("Object not found");
                unbox_ptr_value(&mut *builder, *obj, nbc)
            };
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_closure_store",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[obj_ptr, offset, *val]);
            if let Some(out_name) = op.out.as_ref() {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name, res);
            }
        }
        "guarded_load" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Object not found");
            let offset = op.value.unwrap_or(0);
            let res = emit_guarded_object_field_get(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                *obj,
                offset,
                nbc,
            );
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        "guarded_field_get" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Object not found");
            let obj_ptr = unbox_ptr_value(&mut *builder, *obj, nbc);
            let class_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Class not found");
            let expected_version = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Expected version not found");
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_guarded_field_get_ptr",
                &[
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                ],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[
                    obj_ptr,
                    *class_bits,
                    *expected_version,
                    offset,
                    attr_ptr,
                    attr_len,
                ],
            );
            let res = builder.inst_results(call)[0];
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        "guarded_field_set" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Object not found");
            let obj_ptr = unbox_ptr_value(&mut *builder, *obj, nbc);
            let class_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Class not found");
            let expected_version = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Expected version not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Value not found");
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_guarded_field_set_ptr",
                &[
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                ],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[
                    obj_ptr,
                    *class_bits,
                    *expected_version,
                    offset,
                    *val,
                    attr_ptr,
                    attr_len,
                ],
            );
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name.clone(), res);
            }
        }
        "guarded_field_init" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Object not found");
            let obj_ptr = unbox_ptr_value(&mut *builder, *obj, nbc);
            let class_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Class not found");
            let expected_version = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Expected version not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Value not found");
            let Some(attr_name) = op.s_value.as_ref() else {
                return OpFlow::Continue;
            };
            let data_id = module
                .declare_data(
                    &format!("attr_{}_{}", func_name, op_idx),
                    Linkage::Local,
                    false,
                    false,
                )
                .unwrap();
            let mut data_ctx = DataDescription::new();
            data_ctx.define(attr_name.as_bytes().to_vec().into_boxed_slice());
            module.define_data(data_id, &data_ctx).unwrap();

            let global_ptr = module.declare_data_in_func(data_id, builder.func);
            let attr_ptr = builder.ins().symbol_value(types::I64, global_ptr);
            let attr_len = builder.ins().iconst(types::I64, attr_name.len() as i64);
            let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_guarded_field_init_ptr",
                &[
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                    types::I64,
                ],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[
                    obj_ptr,
                    *class_bits,
                    *expected_version,
                    offset,
                    *val,
                    attr_ptr,
                    attr_len,
                ],
            );
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name.clone(), res);
            }
        }
        "guard_type" | "guard_tag" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Static guard satisfaction for proven types: when the
            // value is known to be a specific scalar type and the
            // expected tag matches, the guard is statically satisfied.
            if scalar_fast_paths_enabled {
                let tag = op.s_value.as_deref().unwrap_or("");
                let val_name = args.first().map(String::as_str).unwrap_or("");
                if (tag == "int"
                    && (int_like_vars.contains(val_name)
                        || int_carriers_plan.is_raw_int_carrier_name(val_name)))
                    || (tag == "float" && float_like_vars.contains(val_name))
                    || (tag == "bool" && bool_like_vars.contains(val_name))
                    || (tag == "str" && str_like_vars.contains(val_name))
                {
                    // Type already proven — skip runtime guard.
                    return OpFlow::Continue;
                }
            }
            // When both the value and expected tag are proven-int
            // (raw-primary), the guard is statically satisfied:
            // an int value always matches an int tag.  Skip the
            // runtime call entirely.
            if scalar_fast_paths_enabled
                && int_carriers_plan.is_raw_int_carrier_name(&args[0])
                && int_carriers_plan.is_raw_int_carrier_name(&args[1])
            {
                // Static guard: int matches int.  No-op.
            } else {
                let val = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    int_carriers_plan,
                    float_primary_vars,
                )
                .expect("Guard value not found");
                let expected = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[1],
                    int_carriers_plan,
                    float_primary_vars,
                )
                .expect("Guard expected tag not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_guard_type",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                builder.ins().call(local_callee, &[*val, *expected]);
            }
        }
        "guard_layout" | "guard_dict_shape" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Guard object not found");
            let obj_ptr = unbox_ptr_value(&mut *builder, *obj, nbc);
            let class_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Guard class not found");
            let expected_version = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Guard version not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_guard_layout_ptr",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[obj_ptr, *class_bits, *expected_version]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handle_memory_op received non-memory op `{}`", op.kind),
    }

    OpFlow::Proceed
}
