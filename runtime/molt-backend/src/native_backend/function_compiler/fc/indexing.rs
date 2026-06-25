use super::super::*;

/// Single-source kind authority for [`handle_indexing_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["index", "store_index", "del_index", "slice", "slice_new"];
use super::list_index_fast_path::{
    ListIndexFastPathState, emit_regular_list_container_absorb_store,
    generic_list_int_lane_eligible, index_fallback_import_name, store_index_fallback_import_name,
};
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for subscript/index/slice operations.
///
/// The main `index`/`store_index` arms live here with `del_index`, `slice`,
/// and `slice_new` so list/dict/tuple subscript lowering has one native
/// codegen authority. Bodies are moved from `compile_func_inner`; only
/// backend-field access paths and borrowed `op.out` reads change.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_indexing_op(
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
    scalarized_tuples: &BTreeMap<String, Vec<Value>>,
    int_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    float_like_vars: &BTreeSet<String>,
    str_like_vars: &BTreeSet<String>,
    none_like_vars: &BTreeSet<String>,
    list_index_fast_paths: &mut ListIndexFastPathState,
    scalar_fast_paths_enabled: bool,
    representation_plan: &ScalarRepresentationPlan,
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) {
    let ops = func_ops;
    let var_is_int = |name: &str| {
        scalar_fast_paths_enabled
            && (int_like_vars.contains(name) || int_primary_vars.contains(name))
    };
    let var_is_bool = |name: &str| scalar_fast_paths_enabled && bool_like_vars.contains(name);
    let var_is_str = |name: &str| scalar_fast_paths_enabled && str_like_vars.contains(name);
    let var_is_known_non_heap = |name: &str| {
        int_like_vars.contains(name)
            || int_primary_vars.contains(name)
            || bool_like_vars.contains(name)
            || bool_primary_vars.contains(name)
            || float_like_vars.contains(name)
            || float_primary_vars.contains(name)
            || none_like_vars.contains(name)
    };
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
                                       int_primary_vars: &BTreeSet<String>,
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
            int_primary_vars,
            float_primary_vars,
            bool_primary_vars,
            nbc,
        )
    };
    match op.kind.as_str() {
        "index" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Stack-tuple fast path: resolve element at compile time.
            let stack_resolved = scalarized_tuples.get(&args[0]).and_then(|elems| {
                SimpleBackend::resolve_const_int(ops, op_idx, &args[1]).and_then(|ci| {
                    let ui = ci as usize;
                    elems.get(ui).copied()
                })
            });
            if let Some(elem_val) = stack_resolved {
                // The element came from a non-escaping tuple; inc_ref
                // to keep refcount correct since the tuple itself was
                // never heap-allocated.
                emit_inc_ref_obj(&mut *builder, elem_val, local_inc_ref_obj, nbc);
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, elem_val);
                }
            } else {
                let obj = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    import_refs,
                    sealed_blocks,
                    vars,
                    &args[0],
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("Obj not found");
                let idx = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    import_refs,
                    sealed_blocks,
                    vars,
                    &args[1],
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("Index not found");
                let mut sig = module.make_signature();
                sig.params.push(AbiParam::new(types::I64));
                sig.params.push(AbiParam::new(types::I64));
                sig.returns.push(AbiParam::new(types::I64));
                if representation_plan.op_has_container_storage(
                    op_idx,
                    op,
                    ContainerStorageKind::FlatListInt,
                ) {
                    // Inline list[int] getitem — direct memory access using
                    // ListIntStorage (#[repr(C)]): [data@0, len@8, cap@16].
                    //
                    // Requires raw_int_shadow index for bounds-checked inline path.
                    // Falls back to the safe runtime function otherwise.
                    // Inside loops, use Variable-only shadows (phi-correct).
                    let raw_idx_lookup =
                        int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
                    if let Some(raw_idx) = raw_idx_lookup {
                        // Extract storage_ptr, data_ptr, len (cached across loop iterations).
                        let (data_ptr, len_val) = {
                            let dp = if let Some(&var) =
                                list_index_fast_paths.list_int_data_cache.get(&args[0])
                            {
                                builder.use_var(var)
                            } else {
                                let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                                let shifted = builder.ins().ishl_imm(masked, 16);
                                let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                                let storage_ptr = builder.ins().load(
                                    types::I64,
                                    MemFlagsData::trusted(),
                                    obj_ptr,
                                    0,
                                );
                                let dp = builder.ins().load(
                                    types::I64,
                                    MemFlagsData::trusted(),
                                    storage_ptr,
                                    LIST_INT_STORAGE_DATA_OFFSET,
                                );
                                let var = builder.declare_var(types::I64);
                                builder.def_var(var, dp);
                                list_index_fast_paths
                                    .list_int_data_cache
                                    .insert(args[0].clone(), var);
                                // Also cache len
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
                            let lv = if let Some(&var) =
                                list_index_fast_paths.list_int_len_cache.get(&args[0])
                            {
                                builder.use_var(var)
                            } else {
                                // Len not cached yet (data was cached in a prior op).
                                let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                                let shifted = builder.ins().ishl_imm(masked, 16);
                                let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                                let storage_ptr = builder.ins().load(
                                    types::I64,
                                    MemFlagsData::trusted(),
                                    obj_ptr,
                                    0,
                                );
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
                        let bce_safe = op.bce_safe == Some(true);
                        if bce_safe {
                            // BCE-proven safe: straight-line element access
                            // with no bounds check, no branch, no slow path.
                            let byte_offset = builder.ins().ishl_imm(raw_idx, 3);
                            let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                            let raw_result = builder.ins().load(
                                types::I64,
                                MemFlagsData::trusted(),
                                elem_addr,
                                0,
                            );
                            let boxed_res = box_int_value(&mut *builder, raw_result, nbc);
                            if let Some(out__) = op.out.as_ref() {
                                def_var_named(&mut *builder, vars, out__, boxed_res);
                            }
                        } else {
                            // Bounds check: 0 <= raw_idx < len.
                            // On failure, fall through to the safe runtime function.
                            let in_bounds =
                                builder
                                    .ins()
                                    .icmp(IntCC::UnsignedLessThan, raw_idx, len_val);
                            let fast_block = builder.create_block();
                            let slow_block = builder.create_block();
                            builder.set_cold_block(slow_block);
                            let merge_block = builder.create_block();
                            builder.append_block_param(merge_block, types::I64); // boxed result
                            builder.append_block_param(merge_block, types::I64); // raw result (valid only on fast path)
                            builder
                                .ins()
                                .brif(in_bounds, fast_block, &[], slow_block, &[]);

                            // Fast path: direct load
                            switch_to_block_materialized(&mut *builder, fast_block);
                            seal_block_once(&mut *builder, sealed_blocks, fast_block);
                            let byte_offset = builder.ins().imul_imm(raw_idx, 8);
                            let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                            let raw_result = builder.ins().load(
                                types::I64,
                                MemFlagsData::trusted(),
                                elem_addr,
                                0,
                            );
                            let boxed_res = box_int_value(&mut *builder, raw_result, nbc);
                            jump_block(&mut *builder, merge_block, &[boxed_res, raw_result]);

                            // Slow path: safe runtime call (handles negative index, IndexError)
                            switch_to_block_materialized(&mut *builder, slow_block);
                            seal_block_once(&mut *builder, sealed_blocks, slow_block);
                            let callee = SimpleBackend::import_func_id_split(
                                &mut *module,
                                &mut *import_ids,
                                "molt_list_int_getitem",
                                &[types::I64, types::I64],
                                &[types::I64],
                            );
                            let local_callee = module.declare_func_in_func(callee, builder.func);
                            let call = builder.ins().call(local_callee, &[*obj, *idx]);
                            let slow_res = builder.inst_results(call)[0];
                            // Unbox the runtime result to get the true raw i64.
                            // Using a 0 sentinel here would poison downstream shadows
                            // (e.g., lst[-1] used in a comparison would compare 0).
                            let slow_raw = unbox_int(&mut *builder, slow_res, nbc);
                            jump_block(&mut *builder, merge_block, &[slow_res, slow_raw]);

                            // Merge
                            switch_to_block_materialized(&mut *builder, merge_block);
                            seal_block_once(&mut *builder, sealed_blocks, merge_block);
                            let merged_boxed = builder.block_params(merge_block)[0];
                            let merged_raw = builder.block_params(merge_block)[1];
                            if let Some(out__) = op.out.as_ref() {
                                let merged = if int_primary_vars.contains(out__.as_str()) {
                                    merged_raw
                                } else {
                                    merged_boxed
                                };
                                def_var_named(&mut *builder, vars, out__, merged);
                            }
                        }
                    } else {
                        // Fallback: NaN-boxed index, call the standard variant.
                        let callee = SimpleBackend::import_func_id_split(
                            &mut *module,
                            &mut *import_ids,
                            "molt_list_int_getitem",
                            &[types::I64, types::I64],
                            &[types::I64],
                        );
                        let local_callee = module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*obj, *idx]);
                        let res = builder.inst_results(call)[0];
                        if let Some(out__) = op.out.as_ref() {
                            def_var_from_boxed_transport(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                vars,
                                int_primary_vars,
                                bool_primary_vars,
                                float_primary_vars,
                                nbc,
                                out__,
                                res,
                            );
                        }
                    }
                } else if generic_list_int_lane_eligible(
                    representation_plan,
                    op,
                    op_index_key_is_integer_family(op),
                ) {
                    // Inline list getitem — handles both TYPE_ID_LIST (Vec<u64>)
                    // and TYPE_ID_LIST_BOOL (ListBoolStorage, repr(C): [data@0, len@8, cap@16]).
                    //
                    // At cache-miss time we load the type_id from the object header
                    // and select the correct data/len offsets. The is_bool flag is
                    // cached alongside data_ptr/len so the fast-block element access
                    // can branch between u64-load (regular list) and u8-load+NaN-box
                    // (list_bool) without re-loading the header.
                    let raw_idx_lookup =
                        int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
                    if let Some(raw_idx) = raw_idx_lookup {
                        let vec_layout = vec_u64_layout();
                        // Determine output element type for specialization.
                        // When known, the cache-miss path skips the type_id
                        // check + dual-layout loads, and the fast path skips
                        // the per-access is_bool branch entirely.
                        let getitem_out_is_bool = op.out.as_ref().is_some_and(|o| var_is_bool(o));
                        let getitem_out_is_non_bool = op.out.as_ref().is_some_and(|o| {
                            var_is_int(o) || var_is_str(o) || float_like_vars.contains(o.as_str())
                        });
                        // Extract data_ptr, len, and is_bool flag (cached across loop iterations).
                        let (data_ptr, len_val, is_bool_val) = {
                            let dp = if let Some(&var) =
                                list_index_fast_paths.list_data_cache.get(&args[0])
                            {
                                builder.use_var(var)
                            } else {
                                let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                                let shifted = builder.ins().ishl_imm(masked, 16);
                                let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                                // obj_ptr[0] = storage pointer (Vec<u64> or ListBoolStorage)
                                let storage_ptr = builder.ins().load(
                                    types::I64,
                                    MemFlagsData::trusted(),
                                    obj_ptr,
                                    0,
                                );
                                if getitem_out_is_bool {
                                    // Proven bool list -- skip type_id check, use
                                    // ListBoolStorage layout (repr(C): data@0, len@8).
                                    let ibvar = builder.declare_var(types::I8);
                                    let const_true = builder.ins().iconst(types::I8, 1);
                                    builder.def_var(ibvar, const_true);
                                    list_index_fast_paths
                                        .list_is_bool_cache
                                        .insert(args[0].clone(), ibvar);
                                    let dp = builder.ins().load(
                                        types::I64,
                                        MemFlagsData::trusted(),
                                        storage_ptr,
                                        0i32,
                                    );
                                    let len = builder.ins().load(
                                        types::I64,
                                        MemFlagsData::trusted(),
                                        storage_ptr,
                                        8i32,
                                    );
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
                                } else if getitem_out_is_non_bool {
                                    // Proven non-bool list -- skip type_id check, use
                                    // Vec<u64> layout (repr(Rust), probed offsets).
                                    let ibvar = builder.declare_var(types::I8);
                                    let const_false = builder.ins().iconst(types::I8, 0);
                                    builder.def_var(ibvar, const_false);
                                    list_index_fast_paths
                                        .list_is_bool_cache
                                        .insert(args[0].clone(), ibvar);
                                    let dp = builder.ins().load(
                                        types::I64,
                                        MemFlagsData::trusted(),
                                        storage_ptr,
                                        vec_layout.data_offset,
                                    );
                                    let len = builder.ins().load(
                                        types::I64,
                                        MemFlagsData::trusted(),
                                        storage_ptr,
                                        vec_layout.len_offset,
                                    );
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
                                } else {
                                    // Unknown element type -- load type_id and both layouts.
                                    let tid = builder.ins().load(
                                        types::I32,
                                        MemFlagsData::trusted(),
                                        obj_ptr,
                                        HEADER_TYPE_ID_OFFSET,
                                    );
                                    let bool_tid =
                                        builder.ins().iconst(types::I32, JIT_TYPE_ID_LIST_BOOL);
                                    let is_bool = builder.ins().icmp(IntCC::Equal, tid, bool_tid);
                                    let ibvar = builder.declare_var(types::I8);
                                    builder.def_var(ibvar, is_bool);
                                    list_index_fast_paths
                                        .list_is_bool_cache
                                        .insert(args[0].clone(), ibvar);
                                    let dp_bool = builder.ins().load(
                                        types::I64,
                                        MemFlagsData::trusted(),
                                        storage_ptr,
                                        0i32,
                                    );
                                    let len_bool = builder.ins().load(
                                        types::I64,
                                        MemFlagsData::trusted(),
                                        storage_ptr,
                                        8i32,
                                    );
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
                                }
                            };
                            let lv = if let Some(&var) =
                                list_index_fast_paths.list_len_cache.get(&args[0])
                            {
                                builder.use_var(var)
                            } else {
                                // Len not cached yet (data was cached in a prior op).
                                let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                                let shifted = builder.ins().ishl_imm(masked, 16);
                                let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                                let storage_ptr = builder.ins().load(
                                    types::I64,
                                    MemFlagsData::trusted(),
                                    obj_ptr,
                                    0,
                                );
                                // Use is_bool_cache if available, otherwise re-probe.
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
                                    let bool_tid =
                                        builder.ins().iconst(types::I32, JIT_TYPE_ID_LIST_BOOL);
                                    let ib = builder.ins().icmp(IntCC::Equal, tid, bool_tid);
                                    let ibvar = builder.declare_var(types::I8);
                                    builder.def_var(ibvar, ib);
                                    list_index_fast_paths
                                        .list_is_bool_cache
                                        .insert(args[0].clone(), ibvar);
                                    ib
                                };
                                let len_bool = builder.ins().load(
                                    types::I64,
                                    MemFlagsData::trusted(),
                                    storage_ptr,
                                    8i32,
                                );
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
                            let ibv = if let Some(&v) =
                                list_index_fast_paths.list_is_bool_cache.get(&args[0])
                            {
                                builder.use_var(v)
                            } else {
                                // Fallback: assume regular list (is_bool = 0).
                                builder.ins().iconst(types::I8, 0)
                            };
                            (dp, lv, ibv)
                        };
                        let bce_safe_list = op.bce_safe == Some(true);
                        let out_is_bool = getitem_out_is_bool;
                        let out_is_non_bool = getitem_out_is_non_bool;
                        if bce_safe_list && out_is_bool {
                            // BCE-proven safe + proven bool list: straight-line
                            // u8-load, no bounds check, no inc_ref (bools are
                            // inline NaN-boxed values, not heap pointers).
                            let bool_elem_addr = builder.ins().iadd(data_ptr, raw_idx);
                            let byte_val = builder.ins().load(
                                types::I8,
                                MemFlagsData::trusted(),
                                bool_elem_addr,
                                0,
                            );
                            let byte_ext = builder.ins().uextend(types::I64, byte_val);
                            let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
                            let bool_elem = builder.ins().bor(bool_tag, byte_ext);
                            if let Some(out__) = op.out.as_ref() {
                                def_bool_result(
                                    &mut *builder,
                                    vars,
                                    bool_primary_vars,
                                    out__,
                                    bool_elem,
                                    Some(byte_ext),
                                );
                            }
                        } else if bce_safe_list && out_is_non_bool {
                            // BCE-proven safe + proven non-bool list: straight-line
                            // u64-load, no bounds check.
                            let byte_offset = builder.ins().ishl_imm(raw_idx, 3);
                            let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                            let elem = builder.ins().load(
                                types::I64,
                                MemFlagsData::trusted(),
                                elem_addr,
                                0,
                            );
                            emit_inc_ref_obj(&mut *builder, elem, local_inc_ref_obj, nbc);
                            if let Some(out__) = op.out.as_ref() {
                                def_var_named(&mut *builder, vars, out__, elem);
                            }
                        } else {
                            // Bounds check: 0 <= raw_idx < len.
                            // On failure, fall through to the safe runtime function.
                            let in_bounds =
                                builder
                                    .ins()
                                    .icmp(IntCC::UnsignedLessThan, raw_idx, len_val);
                            let fast_block = builder.create_block();
                            let slow_block = builder.create_block();
                            builder.set_cold_block(slow_block);
                            let merge_block = builder.create_block();
                            builder.append_block_param(merge_block, types::I64); // result
                            builder
                                .ins()
                                .brif(in_bounds, fast_block, &[], slow_block, &[]);

                            // Fast path: element access.
                            // When the output type is statically known we can
                            // skip the per-access is_bool branch entirely:
                            //   - bool output → always u8-load + NaN-box
                            //   - proven non-bool output → always u64-load + inc_ref
                            //   - unknown → branch on cached is_bool flag
                            switch_to_block_materialized(&mut *builder, fast_block);
                            seal_block_once(&mut *builder, sealed_blocks, fast_block);
                            // Carry a conditional bool payload through the merge block.
                            // For the "unknown" path: when the list IS list_bool,
                            // this shadow holds the raw byte (0 or 1) which lets
                            // downstream `if`/`br_if` consumers skip NaN-box tag
                            // extraction.
                            // For the "proven bool" path: the shadow is always the
                            // raw byte, enabling ZERO NaN-box overhead at consumers.
                            let has_raw_bool_carrier_unknown = !out_is_bool
                                && !out_is_non_bool
                                && list_index_fast_paths
                                    .list_is_bool_cache
                                    .contains_key(&args[0]);
                            let has_raw_bool_carrier = out_is_bool || has_raw_bool_carrier_unknown;
                            if has_raw_bool_carrier {
                                builder.append_block_param(merge_block, types::I64); // raw bool result
                            }
                            if out_is_bool {
                                // Proven bool list — emit u8-load directly, no branch.
                                let bool_elem_addr = builder.ins().iadd(data_ptr, raw_idx);
                                let byte_val = builder.ins().load(
                                    types::I8,
                                    MemFlagsData::trusted(),
                                    bool_elem_addr,
                                    0,
                                );
                                let byte_ext = builder.ins().uextend(types::I64, byte_val);
                                let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
                                let bool_elem = builder.ins().bor(bool_tag, byte_ext);
                                // Pass raw 0/1 shadow for downstream consumers.
                                jump_block(&mut *builder, merge_block, &[bool_elem, byte_ext]);
                            } else if out_is_non_bool {
                                // Proven non-bool list — emit u64-load directly, no branch.
                                let byte_offset = builder.ins().imul_imm(raw_idx, 8);
                                let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                                let elem = builder.ins().load(
                                    types::I64,
                                    MemFlagsData::trusted(),
                                    elem_addr,
                                    0,
                                );
                                emit_inc_ref_obj(&mut *builder, elem, local_inc_ref_obj, nbc);
                                jump_block(&mut *builder, merge_block, &[elem]);
                            } else {
                                // Unknown element type — branch on cached is_bool flag.
                                let zero_i8 = builder.ins().iconst(types::I8, 0);
                                let is_bool_check =
                                    builder.ins().icmp(IntCC::NotEqual, is_bool_val, zero_i8);
                                let bool_load_block = builder.create_block();
                                let vec_load_block = builder.create_block();
                                builder.ins().brif(
                                    is_bool_check,
                                    bool_load_block,
                                    &[],
                                    vec_load_block,
                                    &[],
                                );

                                // Bool list path: load u8, convert to NaN-boxed bool.
                                // No inc_ref needed — bools are inline NaN-boxed values.
                                switch_to_block_materialized(&mut *builder, bool_load_block);
                                seal_block_once(&mut *builder, sealed_blocks, bool_load_block);
                                let bool_elem_addr = builder.ins().iadd(data_ptr, raw_idx);
                                let byte_val = builder.ins().load(
                                    types::I8,
                                    MemFlagsData::trusted(),
                                    bool_elem_addr,
                                    0,
                                );
                                // NaN-box: result = (QNAN | TAG_BOOL) | (byte_val as u64)
                                let byte_ext = builder.ins().uextend(types::I64, byte_val);
                                let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
                                let bool_elem = builder.ins().bor(bool_tag, byte_ext);
                                if has_raw_bool_carrier {
                                    // Shadow carries raw 0/1 for downstream truthiness.
                                    jump_block(&mut *builder, merge_block, &[bool_elem, byte_ext]);
                                } else {
                                    jump_block(&mut *builder, merge_block, &[bool_elem]);
                                }

                                // Regular list path: load u64, inc_ref.
                                switch_to_block_materialized(&mut *builder, vec_load_block);
                                seal_block_once(&mut *builder, sealed_blocks, vec_load_block);
                                let byte_offset = builder.ins().imul_imm(raw_idx, 8);
                                let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                                let elem = builder.ins().load(
                                    types::I64,
                                    MemFlagsData::trusted(),
                                    elem_addr,
                                    0,
                                );
                                emit_inc_ref_obj(&mut *builder, elem, local_inc_ref_obj, nbc);
                                if has_raw_bool_carrier {
                                    // Non-bool path: shadow = NaN-boxed element (not a raw bool).
                                    jump_block(&mut *builder, merge_block, &[elem, elem]);
                                } else {
                                    jump_block(&mut *builder, merge_block, &[elem]);
                                }
                            }

                            // Slow path: safe runtime call (handles negative index, IndexError)
                            switch_to_block_materialized(&mut *builder, slow_block);
                            seal_block_once(&mut *builder, sealed_blocks, slow_block);
                            let callee = SimpleBackend::import_func_id_split(
                                &mut *module,
                                &mut *import_ids,
                                "molt_list_getitem_int_fast",
                                &[types::I64, types::I64],
                                &[types::I64],
                            );
                            let local_callee = module.declare_func_in_func(callee, builder.func);
                            let call = builder.ins().call(local_callee, &[*obj, *idx]);
                            let slow_res = builder.inst_results(call)[0];
                            if has_raw_bool_carrier {
                                if out_is_bool {
                                    // Proven bool: extract raw 0/1 from NaN-boxed bool.
                                    let raw_bit = builder.ins().band_imm(slow_res, 1);
                                    jump_block(&mut *builder, merge_block, &[slow_res, raw_bit]);
                                } else {
                                    // Unknown path: shadow = NaN-boxed element when not bool.
                                    jump_block(&mut *builder, merge_block, &[slow_res, slow_res]);
                                }
                            } else {
                                jump_block(&mut *builder, merge_block, &[slow_res]);
                            }

                            // Merge
                            switch_to_block_materialized(&mut *builder, merge_block);
                            seal_block_once(&mut *builder, sealed_blocks, merge_block);
                            let merged = builder.block_params(merge_block)[0];
                            if let Some(out__) = op.out.as_ref() {
                                // Store conditional bool payload so downstream `if`/`br_if`
                                // can skip NaN-box tag extraction for list_bool elements.
                                if has_raw_bool_carrier {
                                    let raw_carrier = builder.block_params(merge_block)[1];
                                    if out_is_bool {
                                        // Proven bool: raw_carrier is always 0/1.
                                        // Store directly — consumers can branch
                                        // with zero NaN-box overhead.
                                        def_bool_result(
                                            &mut *builder,
                                            vars,
                                            bool_primary_vars,
                                            out__,
                                            merged,
                                            Some(raw_carrier),
                                        );
                                    } else {
                                        def_var_named(&mut *builder, vars, out__, merged);
                                        // Unknown path: shadow is raw 0/1 when
                                        // list is bool, NaN-boxed otherwise.
                                        list_index_fast_paths.conditional_list_bool_shadows.insert(
                                            out__.to_string(),
                                            ConditionalListBoolShadow {
                                                list_name: args[0].clone(),
                                                payload: raw_carrier,
                                            },
                                        );
                                    }
                                } else {
                                    def_var_named(&mut *builder, vars, out__, merged);
                                }
                            }
                        }
                    } else {
                        // No raw_int_shadow — fall back to runtime call.
                        let callee = SimpleBackend::import_func_id_split(
                            &mut *module,
                            &mut *import_ids,
                            "molt_list_getitem_int_fast",
                            &[types::I64, types::I64],
                            &[types::I64],
                        );
                        let local_callee = module.declare_func_in_func(callee, builder.func);
                        let call = builder.ins().call(local_callee, &[*obj, *idx]);
                        let res = builder.inst_results(call)[0];
                        if let Some(out__) = op.out.as_ref() {
                            def_var_from_boxed_transport(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                vars,
                                int_primary_vars,
                                bool_primary_vars,
                                float_primary_vars,
                                nbc,
                                out__,
                                res,
                            );
                        }
                    }
                } else {
                    // Dispatch based on container specialization:
                    // - dict: direct hash-table lookup
                    // - tuple: direct element access
                    // - fast_int: generic list but index is known int (no container_type proof)
                    // - default: full type dispatch
                    let fn_name = index_fallback_import_name(
                        representation_plan,
                        op,
                        op_index_key_is_integer_family(op),
                    );
                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        fn_name,
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, *idx]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out.as_ref() {
                        def_var_from_boxed_transport(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            vars,
                            int_primary_vars,
                            bool_primary_vars,
                            float_primary_vars,
                            nbc,
                            out__,
                            res,
                        );
                    }
                }
            }
        }
        "store_index" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                import_refs,
                sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
            )
            .unwrap_or_else(|| panic!("Value not found in {} op {}", func_name, op_idx));
            if representation_plan.op_has_container_storage(
                op_idx,
                op,
                ContainerStorageKind::FlatListInt,
            ) {
                // Inline list[int] setitem with bounds check using
                // ListIntStorage (#[repr(C)]): [data@0, len@8, cap@16].
                // Inside loops, use Variable-only shadows (phi-correct).
                let raw_idx_opt = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
                let raw_val_opt = int_raw_value(&mut *builder, vars, int_primary_vars, &args[2]);
                if let (Some(raw_idx), Some(raw_val)) = (raw_idx_opt, raw_val_opt) {
                    // Extract storage_ptr, data_ptr, len (cached).
                    let (data_ptr, len_val) = {
                        let dp = if let Some(&var) =
                            list_index_fast_paths.list_int_data_cache.get(&args[0])
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
                        let lv = if let Some(&var) =
                            list_index_fast_paths.list_int_len_cache.get(&args[0])
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
                        let in_bounds =
                            builder
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
                let raw_idx_opt = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
                if let Some(raw_idx) = raw_idx_opt {
                    let vec_layout = vec_u64_layout();
                    let (data_ptr, len_val, is_bool_val) = {
                        let dp = if let Some(&var) =
                            list_index_fast_paths.list_data_cache.get(&args[0])
                        {
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
                            let dp_bool = builder.ins().load(
                                types::I64,
                                MemFlagsData::trusted(),
                                storage_ptr,
                                0i32,
                            );
                            let len_bool = builder.ins().load(
                                types::I64,
                                MemFlagsData::trusted(),
                                storage_ptr,
                                8i32,
                            );
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
                        let lv = if let Some(&var) =
                            list_index_fast_paths.list_len_cache.get(&args[0])
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
                                let bool_tid =
                                    builder.ins().iconst(types::I32, JIT_TYPE_ID_LIST_BOOL);
                                let ib = builder.ins().icmp(IntCC::Equal, tid, bool_tid);
                                let ibvar = builder.declare_var(types::I8);
                                builder.def_var(ibvar, ib);
                                list_index_fast_paths
                                    .list_is_bool_cache
                                    .insert(args[0].clone(), ibvar);
                                ib
                            };
                            let len_bool = builder.ins().load(
                                types::I64,
                                MemFlagsData::trusted(),
                                storage_ptr,
                                8i32,
                            );
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
                        let ibv = if let Some(&v) =
                            list_index_fast_paths.list_is_bool_cache.get(&args[0])
                        {
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
                        let is_bool_bce =
                            builder
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
                            bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[2])
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
                        let in_bounds =
                            builder
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
                        let is_bool_check =
                            builder.ins().icmp(IntCC::NotEqual, is_bool_val, zero_i8);

                        if setitem_val_is_bool {
                            // Value is a compile-time-proven bool: inline both paths.
                            let bool_store_block = builder.create_block();
                            let vec_store_block = builder.create_block();
                            builder.ins().brif(
                                is_bool_check,
                                bool_store_block,
                                &[],
                                vec_store_block,
                                &[],
                            );

                            // Bool list path: store bool as u8.
                            // No dec_ref/inc_ref needed — bools are inline values.
                            switch_to_block_materialized(&mut *builder, bool_store_block);
                            seal_block_once(&mut *builder, sealed_blocks, bool_store_block);
                            let bool_elem_addr = builder.ins().iadd(data_ptr, raw_idx);
                            let byte_val = if let Some(raw_val) =
                                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[2])
                            {
                                // Raw bool primary available — skip NaN-box extraction.
                                builder.ins().ireduce(types::I8, raw_val)
                            } else {
                                // Extract low bit from NaN-boxed bool.
                                let low_bit = builder.ins().band_imm(*val, 1);
                                builder.ins().ireduce(types::I8, low_bit)
                            };
                            builder.ins().store(
                                MemFlagsData::trusted(),
                                byte_val,
                                bool_elem_addr,
                                0,
                            );
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
                            builder.ins().brif(
                                is_bool_check,
                                slow_block,
                                &[],
                                vec_store_block,
                                &[],
                            );

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
                    // No raw_int_shadow — fall back to runtime call.
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
                    bool_like_vars,
                    bool_primary_vars,
                    vars,
                    nbc,
                    int_primary_vars,
                    float_primary_vars,
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
        "del_index" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
            )
            .unwrap_or_else(|| panic!("Obj not found in {} op {}", func_name, op_idx));
            let idx = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
            )
            .unwrap_or_else(|| panic!("Index not found in {} op {}", func_name, op_idx));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_del_index",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj, *idx]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "slice" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let target = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Slice target not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Slice start not found");
            let end = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Slice end not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_slice",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*target, *start, *end]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "slice_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Slice start not found");
            let stop = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Slice stop not found");
            let step = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
            )
            .expect("Slice step not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_slice_new",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*start, *stop, *step]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("unexpected indexing op kind: {}", op.kind),
    }
}
