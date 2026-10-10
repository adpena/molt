use super::super::*;
use super::list_index_fast_path::{
    ListIndexFastPathState, ListStorageField, generic_list_int_lane_eligible,
    index_fallback_import_name, observe_generic_list_storage,
};

#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &["index"];

/// Publish an owned index result through boxed transport; a discarded result
/// is released by the owned-result sink.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
fn bind_index_result(
    out: Option<&str>,
    result: Value,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
    match out {
        Some(out) => def_var_from_boxed_transport(
            module,
            import_ids,
            builder,
            import_refs,
            vars,
            representation_plan,
            nbc,
            out,
            result,
        ),
        None => bind_owned_runtime_result_name(None, result, module, import_ids, builder, vars),
    }
}

/// A proven list container is borrowed from its own boxed home: list facts
/// exclude scalar carriers, so reading it mints no temporary box.
#[cfg(feature = "native-backend")]
fn proven_list_container(
    builder: &mut FunctionBuilder<'_>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    name: &str,
) -> crate::VarValue {
    debug_assert!(
        !representation_plan.name_is_non_heap_scalar(name),
        "proven list container {name} has a scalar carrier"
    );
    var_get(builder, vars, name).expect("Obj not found")
}

/// Cranelift codegen for subscript read (`index`). Proven list lanes read a
/// plan-owned raw-int index without boxing it; runtime lanes borrow the
/// container and key through one operand transaction. The owned result is
/// bound through boxed transport, or released when discarded.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_subscript_get_op(
    op: &OpIR,
    op_idx: usize,
    const_int_map: &BTreeMap<String, i64>,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    scalarized_tuples: &BTreeMap<String, Vec<Value>>,
    representation_plan: &ScalarRepresentationPlan,
    list_index_fast_paths: &mut ListIndexFastPathState,
    scalar_fast_paths_enabled: bool,
    local_inc_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
) {
    let op_index_key_is_integer_family = |op: &OpIR| {
        scalar_fast_paths_enabled && representation_plan.op_index_key_is_integer_family(op)
    };
    let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
    let out = crate::tir::simple_def_use::simple_ir_out_result(op);
    let origin = builder.current_block();
    // Stack-tuple fast path: resolve element at compile time.
    let stack_resolved = scalarized_tuples.get(&args[0]).and_then(|elems| {
        const_int_map.get(&args[1]).and_then(|&ci| {
            let ui = ci as usize;
            elems.get(ui).copied()
        })
    });
    let flat_list_int =
        representation_plan.op_has_container_storage(op_idx, op, ContainerStorageKind::FlatListInt);
    let generic_list_lane = !flat_list_int
        && generic_list_int_lane_eligible(
            representation_plan,
            op,
            op_index_key_is_integer_family(op),
        );
    if let Some(elem_val) = stack_resolved {
        // The constructor published this borrowed view only on success (None
        // on failure). Retain it only for an observable index result owner.
        if let Some(out__) = out {
            emit_inc_ref_obj(&mut *builder, elem_val, local_inc_ref_obj);
            bind_index_result(
                Some(out__),
                elem_val,
                module,
                import_ids,
                builder,
                import_refs,
                vars,
                representation_plan,
                nbc,
            );
        }
    } else {
        // Proven list lanes read a plan-owned raw-int index without boxing it;
        // every other shape is a runtime lane.
        let raw_idx_lookup = if flat_list_int || generic_list_lane {
            int_raw_value(&mut *builder, vars, representation_plan, &args[1])
        } else {
            None
        };
        match raw_idx_lookup {
            Some(raw_idx) if flat_list_int => {
                // Inline list[int] getitem — direct memory access using
                // ListIntStorage (#[repr(C)]): [data@0, len@8, cap@16].
                // Inside loops, use Variable-only shadows (phi-correct).
                let obj = proven_list_container(builder, vars, representation_plan, &args[0]);
                // Extract storage_ptr, data_ptr, len (cached across loop iterations).
                let (data_ptr, len_val) = {
                    let dp = if let Some(var) =
                        list_index_fast_paths.get(ListStorageField::IntData, &args[0], builder)
                    {
                        builder.use_var(var)
                    } else {
                        let obj_ptr = unbox_ptr_value(builder, *obj);
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
                        let var = builder.declare_var(types::I64);
                        builder.def_var(var, dp);
                        list_index_fast_paths.insert(
                            ListStorageField::IntData,
                            args[0].clone(),
                            var,
                            builder,
                        );
                        // Also cache len
                        let len = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            LIST_INT_STORAGE_LEN_OFFSET,
                        );
                        let lvar = builder.declare_var(types::I64);
                        builder.def_var(lvar, len);
                        list_index_fast_paths.insert(
                            ListStorageField::IntLen,
                            args[0].clone(),
                            lvar,
                            builder,
                        );
                        dp
                    };
                    let lv = if let Some(var) =
                        list_index_fast_paths.get(ListStorageField::IntLen, &args[0], builder)
                    {
                        builder.use_var(var)
                    } else {
                        // Len not cached yet (data was cached in a prior op).
                        let obj_ptr = unbox_ptr_value(builder, *obj);
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
                        list_index_fast_paths.insert(
                            ListStorageField::IntLen,
                            args[0].clone(),
                            lvar,
                            builder,
                        );
                        len
                    };
                    (dp, lv)
                };
                let bce_safe = op.bce_safe == Some(true);
                // A discarded element stays raw: the checked raw lanes keep
                // IndexError without materializing a result object.
                let out_is_raw =
                    out.is_none_or(|out| representation_plan.is_raw_int_carrier_name(out));
                if bce_safe {
                    // BCE-proven safe: straight-line element access
                    // with no bounds check, no branch, no slow path.
                    let byte_offset = builder.ins().ishl_imm(raw_idx, 3);
                    let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                    let raw_result =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), elem_addr, 0);
                    if let Some(out__) = out {
                        let result = if out_is_raw {
                            raw_result
                        } else {
                            box_raw_i64_value_overflow_safe(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                raw_result,
                            )
                        };
                        def_var_named(&mut *builder, vars, out__, result);
                    }
                } else {
                    // Bounds check: 0 <= raw_idx < len.
                    // On failure, fall through to the safe runtime function.
                    let in_bounds = builder
                        .ins()
                        .icmp(IntCC::UnsignedLessThan, raw_idx, len_val);
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);
                    builder
                        .ins()
                        .brif(in_bounds, fast_block, &[], slow_block, &[]);

                    // Fast path: direct load
                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, sealed_blocks, fast_block);
                    let byte_offset = builder.ins().imul_imm(raw_idx, 8);
                    let elem_addr = builder.ins().iadd(data_ptr, byte_offset);
                    let raw_result =
                        builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), elem_addr, 0);
                    let fast_result = if out_is_raw {
                        raw_result
                    } else {
                        box_raw_i64_value_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            sealed_blocks,
                            raw_result,
                        )
                    };
                    jump_block(&mut *builder, merge_block, &[fast_result]);

                    // Slow path: the checked raw runtime call handles negative
                    // indices and raises IndexError, so the proven raw index is
                    // never boxed.
                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, sealed_blocks, slow_block);
                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_list_int_getitem_raw_checked",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*obj, raw_idx]);
                    let raw_slow = builder.inst_results(call)[0];
                    let slow_res = if out_is_raw {
                        raw_slow
                    } else {
                        // A failed checked read has no element to materialize.
                        // Keep the original error and join with boxed None.
                        let mut result =
                            NativeOperandTransaction::begin(builder, representation_plan, []);
                        result.continue_unless_pending(
                            module,
                            import_ids,
                            builder,
                            import_refs,
                            sealed_blocks,
                        );
                        let boxed = box_raw_i64_value_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            sealed_blocks,
                            raw_slow,
                        );
                        result.finish(
                            boxed,
                            None,
                            module,
                            import_ids,
                            builder,
                            import_refs,
                            sealed_blocks,
                            block_tracked_obj,
                            block_tracked_ptr,
                        )
                    };
                    jump_block(&mut *builder, merge_block, &[slow_res]);

                    // Merge
                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, sealed_blocks, merge_block);
                    let merged = builder.block_params(merge_block)[0];
                    if let Some(out__) = out {
                        def_var_named(&mut *builder, vars, out__, merged);
                    }
                }
            }
            Some(raw_idx) => {
                // Exact builtin class provenance admits source-slot bypass.
                // The live heap kind, never the inferred output type, chooses
                // Vec<u64>, compact integers, or compact booleans.
                let obj = proven_list_container(builder, vars, representation_plan, &args[0]);
                let storage =
                    observe_generic_list_storage(builder, list_index_fast_paths, &args[0], *obj);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64); // boxed result
                builder.append_block_param(merge_block, types::I64); // conditional bool payload
                let slow_block = if op.bce_safe == Some(true) {
                    None
                } else {
                    let in_bounds =
                        builder
                            .ins()
                            .icmp(IntCC::UnsignedLessThan, raw_idx, storage.len);
                    let fast_block = builder.create_block();
                    let slow = builder.create_block();
                    builder.set_cold_block(slow);
                    builder.ins().brif(in_bounds, fast_block, &[], slow, &[]);
                    switch_to_block_materialized(builder, fast_block);
                    seal_block_once(builder, sealed_blocks, fast_block);
                    Some(slow)
                };
                let bool_block = builder.create_block();
                let word_block = builder.create_block();
                builder
                    .ins()
                    .brif(storage.is_bool, bool_block, &[], word_block, &[]);

                switch_to_block_materialized(builder, bool_block);
                seal_block_once(builder, sealed_blocks, bool_block);
                let address = builder.ins().iadd(storage.data, raw_idx);
                let byte = builder
                    .ins()
                    .load(types::I8, MemFlagsData::trusted(), address, 0);
                let raw_bool = builder.ins().uextend(types::I64, byte);
                let bool_tag = builder.ins().iconst(types::I64, nbc.qnan_tag_bool);
                let boxed_bool = builder.ins().bor(bool_tag, raw_bool);
                jump_block(builder, merge_block, &[boxed_bool, raw_bool]);

                switch_to_block_materialized(builder, word_block);
                seal_block_once(builder, sealed_blocks, word_block);
                let offset = builder.ins().ishl_imm(raw_idx, 3);
                let address = builder.ins().iadd(storage.data, offset);
                let word = builder
                    .ins()
                    .load(types::I64, MemFlagsData::trusted(), address, 0);
                // Compact integers are guaranteed inline by their physical
                // storage authority. Boxed elements keep their original owner.
                let boxed_int = box_int_value(builder, word, nbc);
                let boxed = builder.ins().select(storage.is_int, boxed_int, word);
                emit_inc_ref_obj(builder, boxed, local_inc_ref_obj);
                jump_block(builder, merge_block, &[boxed, boxed]);

                if let Some(slow) = slow_block {
                    switch_to_block_materialized(builder, slow);
                    seal_block_once(builder, sealed_blocks, slow);
                    let callee = import_func_ref(
                        module,
                        import_ids,
                        builder,
                        import_refs,
                        "molt_list_getitem_raw_idx",
                        &[types::I64, types::I64],
                        &[types::I64],
                    );
                    let call = builder.ins().call(callee, &[*obj, raw_idx]);
                    let result = builder.inst_results(call)[0];
                    let shadow =
                        ConditionalListBoolShadow::from_boxed(builder, storage.is_bool, result);
                    jump_block(builder, merge_block, &[result, shadow.payload]);
                }

                switch_to_block_materialized(builder, merge_block);
                seal_block_once(builder, sealed_blocks, merge_block);
                let result = builder.block_params(merge_block)[0];
                let payload = builder.block_params(merge_block)[1];
                bind_index_result(
                    out,
                    result,
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    vars,
                    representation_plan,
                    nbc,
                );
                if let Some(name) = out
                    && representation_plan.name_scalar_kind(name).is_none()
                {
                    list_index_fast_paths.insert_bool_shadow(
                        name.to_string(),
                        ConditionalListBoolShadow {
                            is_bool: storage.is_bool,
                            payload,
                        },
                        builder,
                    );
                }
            }
            None => {
                // Runtime lanes borrow the container and key through one
                // operand transaction. Dispatch follows container
                // specialization: list[int] storage, dict, tuple, a known-int
                // key on a generic list, or full type dispatch.
                let mut operands = NativeOperandTransaction::begin(
                    builder,
                    representation_plan,
                    args[..2].iter().map(String::as_str),
                );
                let obj = operands.operand(
                    &args[0],
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    vars,
                    representation_plan,
                    nbc,
                );
                let idx = operands.operand(
                    &args[1],
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    vars,
                    representation_plan,
                    nbc,
                );
                let fn_name = if flat_list_int {
                    "molt_list_int_getitem"
                } else {
                    index_fallback_import_name(
                        representation_plan,
                        op,
                        op_index_key_is_integer_family(op),
                    )
                };
                let callee = import_func_ref(
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    fn_name,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let call = builder.ins().call(callee, &[obj, idx]);
                let res = builder.inst_results(call)[0];
                let res = operands.finish(
                    res,
                    None,
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    block_tracked_obj,
                    block_tracked_ptr,
                );
                bind_index_result(
                    out,
                    res,
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    vars,
                    representation_plan,
                    nbc,
                );
            }
        }
    }
    if let Some(current) = builder.current_block() {
        carry_internal_cfg_tracking(origin, current, block_tracked_obj, block_tracked_ptr);
    }
}
