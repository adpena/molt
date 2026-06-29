use super::super::*;
use super::list_index_fast_path::{
    ListIndexFastPathState, emit_regular_list_container_absorb_store,
    generic_list_int_lane_eligible, store_index_fallback_import_name,
};
use super::var_get_boxed_overflow_safe_fn;

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &["store_index"];

/// Cranelift codegen for subscript write (`store_index`).
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_subscript_store_op(
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
    list_index_fast_paths: &mut ListIndexFastPathState,
    scalar_fast_paths_enabled: bool,
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) {
    let var_is_bool =
        |name: &str| scalar_fast_paths_enabled && representation_plan.name_is_bool_scalar(name);
    let var_is_known_non_heap = |name: &str| representation_plan.name_is_non_heap_scalar(name);
    let op_index_key_is_integer_family = |op: &OpIR| {
        scalar_fast_paths_enabled && representation_plan.op_index_key_is_integer_family(op)
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
    list_index_fast_paths.invalidate_for_store_index(&args[0]);
    let obj = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        import_refs,
        sealed_blocks,
        vars,
        &args[0],
        representation_plan,
    )
    .unwrap_or_else(|| panic!("Obj not found in {} op {}", func_name, op_idx));
    let idx = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        import_refs,
        sealed_blocks,
        vars,
        &args[1],
        representation_plan,
    )
    .unwrap_or_else(|| panic!("Index not found in {} op {}", func_name, op_idx));
    let val = var_get_boxed_overflow_safe(
        &mut *module,
        &mut *import_ids,
        &mut *builder,
        import_refs,
        sealed_blocks,
        vars,
        &args[2],
        representation_plan,
    )
    .unwrap_or_else(|| panic!("Value not found in {} op {}", func_name, op_idx));
    if representation_plan.op_has_container_storage(op_idx, op, ContainerStorageKind::FlatListInt) {
        // Inline list[int] setitem with bounds check using
        // ListIntStorage (#[repr(C)]): [data@0, len@8, cap@16].
        // Inside loops, use Variable-only shadows (phi-correct).
        let raw_idx_opt = int_raw_value(&mut *builder, vars, representation_plan, &args[1]);
        let raw_val_opt = int_raw_value(&mut *builder, vars, representation_plan, &args[2]);
        if let (Some(raw_idx), Some(raw_val)) = (raw_idx_opt, raw_val_opt) {
            // Extract storage_ptr, data_ptr, len (cached).
            let (data_ptr, len_val) = {
                let dp = if let Some(&var) = list_index_fast_paths.list_int_data_cache.get(&args[0])
                {
                    builder.use_var(var)
                } else {
                    let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                    let shifted = builder.ins().ishl_imm(masked, 16);
                    let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                    let storage_ptr =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
                    let dp = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        LIST_INT_STORAGE_DATA_OFFSET,
                    );
                    let cvar = builder.declare_var(types::I64);
                    builder.def_var(cvar, dp);
                    list_index_fast_paths
                        .list_int_data_cache
                        .insert(args[0].clone(), cvar);
                    let len = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        LIST_INT_STORAGE_LEN_OFFSET,
                    );
                    let lvar = builder.declare_var(types::I64);
                    builder.def_var(lvar, len);
                    list_index_fast_paths
                        .list_int_len_cache
                        .insert(args[0].clone(), lvar);
                    dp
                };
                let lv = if let Some(&var) = list_index_fast_paths.list_int_len_cache.get(&args[0])
                {
                    builder.use_var(var)
                } else {
                    let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                    let shifted = builder.ins().ishl_imm(masked, 16);
                    let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                    let storage_ptr =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
                    let len = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        LIST_INT_STORAGE_LEN_OFFSET,
                    );
                    let lvar = builder.declare_var(types::I64);
                    builder.def_var(lvar, len);
                    list_index_fast_paths
                        .list_int_len_cache
                        .insert(args[0].clone(), lvar);
                    len
                };
                (dp, lv)
            };
            let bce_safe_si = op.bce_safe == Some(true);
            if bce_safe_si {
                // BCE-proven safe: straight-line store, no
                // bounds check, no branch, no slow path.
                let byte_offset = builder.ins().ishl_imm(raw_idx, 3);
                let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                builder
                    .ins()
                    .store(MemFlagsData::trusted(), raw_val, elem_addr, 0);
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, *obj);
                }
            } else {
                // Bounds check: 0 <= raw_idx < len (unsigned comparison).
                let in_bounds = builder
                    .ins()
                    .icmp(IntCC::UnsignedLessThan, raw_idx, len_val);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder
                    .ins()
                    .brif(in_bounds, fast_block, &[], slow_block, &[]);

                // Fast path: direct store
                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, sealed_blocks, fast_block);
                let byte_offset = builder.ins().imul_imm(raw_idx, 8);
                let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                builder
                    .ins()
                    .store(MemFlagsData::trusted(), raw_val, elem_addr, 0);
                jump_block(&mut *builder, merge_block, &[]);

                // Slow path: safe runtime call
                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, sealed_blocks, slow_block);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_list_int_setitem",
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                builder.ins().call(local_callee, &[*obj, *idx, *val]);
                jump_block(&mut *builder, merge_block, &[]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, sealed_blocks, merge_block);
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, *obj);
                }
            } // end else (non-bce_safe list_int setitem)
        } else {
            // Fallback: at least one arg is NaN-boxed, use standard variant.
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_list_int_setitem",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *idx, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
    } else if generic_list_int_lane_eligible(
        representation_plan,
        op,
        op_index_key_is_integer_family(op),
    ) {
        // Inline list setitem — handles both TYPE_ID_LIST (Vec<u64>)
        // and TYPE_ID_LIST_BOOL (ListBoolStorage, repr(C): [data@0, len@8, cap@16]).
        let raw_idx_opt = int_raw_value(&mut *builder, vars, representation_plan, &args[1]);
        if let Some(raw_idx) = raw_idx_opt {
            let vec_layout = vec_u64_layout();
            let (data_ptr, len_val, is_bool_val) = {
                let dp = if let Some(&var) = list_index_fast_paths.list_data_cache.get(&args[0]) {
                    builder.use_var(var)
                } else {
                    let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                    let shifted = builder.ins().ishl_imm(masked, 16);
                    let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                    // Load type_id from header (obj_ptr - 24).
                    let tid = builder.ins().load(
                        types::I32,
                        MemFlagsData::trusted(),
                        obj_ptr,
                        HEADER_TYPE_ID_OFFSET,
                    );
                    let bool_tid = builder.ins().iconst(types::I32, JIT_TYPE_ID_LIST_BOOL);
                    let is_bool = builder.ins().icmp(IntCC::Equal, tid, bool_tid);
                    let ibvar = builder.declare_var(types::I8);
                    builder.def_var(ibvar, is_bool);
                    list_index_fast_paths
                        .list_is_bool_cache
                        .insert(args[0].clone(), ibvar);
                    let storage_ptr =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
                    // ListBoolStorage (repr(C)): data@0, len@8
                    let dp_bool =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), storage_ptr, 0i32);
                    let len_bool =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), storage_ptr, 8i32);
                    // Vec<u64> (repr(Rust), probed offsets)
                    let dp_vec = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        vec_layout.data_offset,
                    );
                    let len_vec = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        vec_layout.len_offset,
                    );
                    let dp = builder.ins().select(is_bool, dp_bool, dp_vec);
                    let len = builder.ins().select(is_bool, len_bool, len_vec);
                    let var = builder.declare_var(types::I64);
                    builder.def_var(var, dp);
                    list_index_fast_paths
                        .list_data_cache
                        .insert(args[0].clone(), var);
                    let lvar = builder.declare_var(types::I64);
                    builder.def_var(lvar, len);
                    list_index_fast_paths
                        .list_len_cache
                        .insert(args[0].clone(), lvar);
                    dp
                };
                let lv = if let Some(&var) = list_index_fast_paths.list_len_cache.get(&args[0]) {
                    builder.use_var(var)
                } else {
                    let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                    let shifted = builder.ins().ishl_imm(masked, 16);
                    let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                    let storage_ptr =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
                    let is_bool = if let Some(&ibv) =
                        list_index_fast_paths.list_is_bool_cache.get(&args[0])
                    {
                        builder.use_var(ibv)
                    } else {
                        let tid = builder.ins().load(
                            types::I32,
                            MemFlagsData::trusted(),
                            obj_ptr,
                            HEADER_TYPE_ID_OFFSET,
                        );
                        let bool_tid = builder.ins().iconst(types::I32, JIT_TYPE_ID_LIST_BOOL);
                        let ib = builder.ins().icmp(IntCC::Equal, tid, bool_tid);
                        let ibvar = builder.declare_var(types::I8);
                        builder.def_var(ibvar, ib);
                        list_index_fast_paths
                            .list_is_bool_cache
                            .insert(args[0].clone(), ibvar);
                        ib
                    };
                    let len_bool =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), storage_ptr, 8i32);
                    let len_vec = builder.ins().load(
                        types::I64,
                        MemFlagsData::trusted(),
                        storage_ptr,
                        vec_layout.len_offset,
                    );
                    let len = builder.ins().select(is_bool, len_bool, len_vec);
                    let lvar = builder.declare_var(types::I64);
                    builder.def_var(lvar, len);
                    list_index_fast_paths
                        .list_len_cache
                        .insert(args[0].clone(), lvar);
                    len
                };
                let ibv = if let Some(&v) = list_index_fast_paths.list_is_bool_cache.get(&args[0]) {
                    builder.use_var(v)
                } else {
                    builder.ins().iconst(types::I8, 0)
                };
                (dp, lv, ibv)
            };
            let bce_safe_list_si = op.bce_safe == Some(true);
            let setitem_val_is_bool = var_is_bool(&args[2]);
            if bce_safe_list_si && setitem_val_is_bool {
                // BCE-proven safe + proven bool value: skip
                // bounds check.  Still branch on is_bool for
                // u8 vs u64 storage layout.
                let zero_i8_bce = builder.ins().iconst(types::I8, 0);
                let is_bool_bce = builder
                    .ins()
                    .icmp(IntCC::NotEqual, is_bool_val, zero_i8_bce);
                let bool_store_bce = builder.create_block();
                let vec_store_bce = builder.create_block();
                let merge_bce = builder.create_block();
                builder
                    .ins()
                    .brif(is_bool_bce, bool_store_bce, &[], vec_store_bce, &[]);
                // Bool list path: store bool as u8.
                switch_to_block_materialized(&mut *builder, bool_store_bce);
                seal_block_once(&mut *builder, sealed_blocks, bool_store_bce);
                let baddr = builder.ins().iadd(data_ptr, raw_idx);
                let bv = if let Some(rb) =
                    bool_raw_value(&mut *builder, vars, representation_plan, &args[2])
                {
                    builder.ins().ireduce(types::I8, rb)
                } else {
                    let lb = builder.ins().band_imm(*val, 1);
                    builder.ins().ireduce(types::I8, lb)
                };
                builder.ins().store(MemFlagsData::trusted(), bv, baddr, 0);
                jump_block(&mut *builder, merge_bce, &[]);
                // Regular list path: store the inline bool, then release old.
                switch_to_block_materialized(&mut *builder, vec_store_bce);
                seal_block_once(&mut *builder, sealed_blocks, vec_store_bce);
                let boff = builder.ins().imul_imm(raw_idx, 8);
                let eaddr = builder.ins().iadd(data_ptr, boff);
                emit_regular_list_container_absorb_store(
                    &mut *builder,
                    sealed_blocks,
                    *obj,
                    eaddr,
                    *val,
                    true,
                    local_inc_ref_obj,
                    local_dec_ref_obj,
                    nbc,
                    merge_bce,
                );
                switch_to_block_materialized(&mut *builder, merge_bce);
                seal_block_once(&mut *builder, sealed_blocks, merge_bce);
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, *obj);
                }
            } else {
                // Bounds check
                let in_bounds = builder
                    .ins()
                    .icmp(IntCC::UnsignedLessThan, raw_idx, len_val);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder
                    .ins()
                    .brif(in_bounds, fast_block, &[], slow_block, &[]);

                // Fast path: branch on is_bool for element store.
                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, sealed_blocks, fast_block);
                let zero_i8 = builder.ins().iconst(types::I8, 0);
                let is_bool_check = builder.ins().icmp(IntCC::NotEqual, is_bool_val, zero_i8);

                if setitem_val_is_bool {
                    // Value is a compile-time-proven bool: inline both paths.
                    let bool_store_block = builder.create_block();
                    let vec_store_block = builder.create_block();
                    builder
                        .ins()
                        .brif(is_bool_check, bool_store_block, &[], vec_store_block, &[]);

                    // Bool list path: store bool as u8.
                    // No dec_ref/inc_ref needed — bools are inline values.
                    switch_to_block_materialized(&mut *builder, bool_store_block);
                    seal_block_once(&mut *builder, sealed_blocks, bool_store_block);
                    let bool_elem_addr = builder.ins().iadd(data_ptr, raw_idx);
                    let byte_val = if let Some(raw_val) =
                        bool_raw_value(&mut *builder, vars, representation_plan, &args[2])
                    {
                        // Raw bool primary available — skip NaN-box extraction.
                        builder.ins().ireduce(types::I8, raw_val)
                    } else {
                        // Extract low bit from NaN-boxed bool.
                        let low_bit = builder.ins().band_imm(*val, 1);
                        builder.ins().ireduce(types::I8, low_bit)
                    };
                    builder
                        .ins()
                        .store(MemFlagsData::trusted(), byte_val, bool_elem_addr, 0);
                    jump_block(&mut *builder, merge_block, &[]);

                    // Regular list path: store the inline bool, then release old.
                    switch_to_block_materialized(&mut *builder, vec_store_block);
                    seal_block_once(&mut *builder, sealed_blocks, vec_store_block);
                    let byte_offset = builder.ins().imul_imm(raw_idx, 8);
                    let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                    emit_regular_list_container_absorb_store(
                        &mut *builder,
                        sealed_blocks,
                        *obj,
                        elem_addr,
                        *val,
                        true,
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        nbc,
                        merge_block,
                    );
                } else {
                    // Value is not proven bool: if list is list_bool, fall to slow
                    // path (which handles type promotion). If regular list, inline store.
                    let vec_store_block = builder.create_block();
                    builder
                        .ins()
                        .brif(is_bool_check, slow_block, &[], vec_store_block, &[]);

                    switch_to_block_materialized(&mut *builder, vec_store_block);
                    seal_block_once(&mut *builder, sealed_blocks, vec_store_block);
                    let byte_offset = builder.ins().imul_imm(raw_idx, 8);
                    let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                    emit_regular_list_container_absorb_store(
                        &mut *builder,
                        sealed_blocks,
                        *obj,
                        elem_addr,
                        *val,
                        var_is_known_non_heap(&args[2]),
                        local_inc_ref_obj,
                        local_dec_ref_obj,
                        nbc,
                        merge_block,
                    );
                }

                // Slow path: safe runtime call
                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, sealed_blocks, slow_block);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_list_setitem_int_fast",
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                builder.ins().call(local_callee, &[*obj, *idx, *val]);
                jump_block(&mut *builder, merge_block, &[]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, sealed_blocks, merge_block);
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, *obj);
                }
            } // end else (non-bce_safe list setitem)
        } else {
            // No raw-int carrier: fall back to runtime call.
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_list_setitem_int_fast",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *idx, *val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
    } else {
        // Dispatch based on container specialization:
        // - dict: direct hash-table set
        // - int fast: list with proven-int index (no container_type proof)
        // - default: full type dispatch
        let fn_name = store_index_fallback_import_name(
            representation_plan,
            op,
            op_index_key_is_integer_family(op),
        );
        // Deferred overflow re-boxing at heap store (store_index).
        let safe_val = ensure_boxed_primitive_safe(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            import_refs,
            sealed_blocks,
            vars,
            nbc,
            representation_plan,
            &args[2],
        );
        let callee = SimpleBackend::import_func_id_split(
            &mut *module,
            &mut *import_ids,
            fn_name,
            &[types::I64, types::I64, types::I64],
            &[types::I64],
        );
        let local_callee = module.declare_func_in_func(callee, builder.func);
        let call = builder.ins().call(local_callee, &[*obj, *idx, safe_val]);
        let res = builder.inst_results(call)[0];
        if let Some(out__) = op.out.as_ref() {
            def_var_named(&mut *builder, vars, out__, res);
        }
    }
}
