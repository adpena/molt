use super::super::*;
use crate::tir::simple_def_use::simple_ir_out_result;
use molt_ir::OpKind;
use molt_ir::runtime_fn_generated::RuntimeFn;
use molt_codegen_abi::task_site::TaskConstructorLayout;

// Single-source kind authority for [`handle_memory_op`], consulted by
// `op_family::native_op_family`. Mirror the `match op.kind` arms below.
#[cfg(feature = "native-backend")]
define_native_op_kinds!(handled_kinds_pattern, HANDLED_KINDS;
    OpKind::Alloc,
    OpKind::StackAlloc,
    OpKind::AllocClass,
    OpKind::AllocTask,
    OpKind::Store,
    OpKind::Load,
    OpKind::ClosureLoad,
    OpKind::ClosureStore,
    OpKind::GuardedLoad,
    OpKind::GuardedFieldGet,
    OpKind::GuardedFieldSet,
    OpKind::GuardType,
    OpKind::GuardTag,
    OpKind::GuardLayout,
);
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// A proven field read through the shared generator
/// (`molt_codegen_abi::field_slot`); its miss calls
/// `molt_object_field_get_ptr(pointer, offset)`.
#[cfg(feature = "native-backend")]
fn emit_proven_field_get(
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    object: Value,
    offset: i64,
) -> Value {
    let offset = i32::try_from(offset).expect("a proven field offset fits an i32");
    let sites = super::calls::native_frame_sites(module, import_ids, builder, import_refs);
    let cold = import_runtime_func_ref(
        module,
        import_ids,
        builder,
        import_refs,
        RuntimeFn::ObjectFieldGetPtr,
    );
    crate::native_backend::simple_backend::emit_field_slot_get(
        builder,
        sealed_blocks,
        sites,
        object,
        offset,
        |builder, pointer| {
            let offset = builder.ins().iconst(types::I64, i64::from(offset));
            let call = builder.ins().call(cold, &[pointer, offset]);
            builder.inst_results(call)[0]
        },
    )
}

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
    module: &mut dyn Module,
    import_ids: &mut RuntimeImportIds,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut NativeFunctionImports,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    alias_roots: &BTreeMap<String, String>,
    last_use: &BTreeMap<String, usize>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    cleanup_roots: &mut NativeCleanupRoots,
    function_table: &molt_ir::FunctionTable,
    output_is_ptr: &mut bool,
    stateful: bool,
    _entry_block: Block,
    profile_enabled_val: Option<Value>,
    guard_profile_only: bool,
    local_inc_ref_obj: NativeRefcountSite,
    local_dec_ref_obj: NativeRefcountSite,
    rc_authority: NativeRcAuthority,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
    let var_get_boxed_overflow_safe = |module: &mut dyn Module,
                                       import_ids: &mut RuntimeImportIds,
                                       builder: &mut FunctionBuilder<'_>,
                                       import_refs: &mut NativeFunctionImports,
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

    match op.kind {
        OpKind::StackAlloc => panic!(
            "{}",
            crate::tir::target_info::BOXED_STACK_ALLOCATION_UNSUPPORTED
        ),
        OpKind::Alloc => {
            let size = op.value.unwrap_or(0);
            let iconst = builder.ins().iconst(types::I64, size);

            let callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                RuntimeFn::Alloc,
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[iconst]);
            let unpublished = builder.inst_results(call)[0];
            let publish_callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                RuntimeFn::ObjectPublishInitialized,
            );
            let local_publish = module.declare_func_in_func(publish_callee, builder.func);
            let publish_call = builder.ins().call(local_publish, &[unpublished]);
            let res = builder.inst_results(publish_call)[0];
            let Some(out_name) = simple_ir_out_result(op) else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        OpKind::AllocClass => {
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
                representation_plan,
            )
            .expect("Class not found");
            let iconst = builder.ins().iconst(types::I64, size);

            let callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                RuntimeFn::AllocClass,
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[iconst, *class_bits]);
            let unpublished = builder.inst_results(call)[0];
            let publish_callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                RuntimeFn::ObjectPublishInitialized,
            );
            let local_publish = module.declare_func_in_func(publish_callee, builder.func);
            let publish_call = builder.ins().call(local_publish, &[unpublished]);
            let res = builder.inst_results(publish_call)[0];
            let Some(out_name) = simple_ir_out_result(op) else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        OpKind::AllocTask => {
            use molt_codegen_abi::task_site::{PayloadCustody, TaskFrameContext};
            let closure_size = op.value.unwrap_or(0);
            let layout = TaskConstructorLayout::for_alloc_kind(op.task_kind.as_deref());
            let operands = op.args.as_deref().unwrap_or(&[]);
            let (function_name, payload_names) = molt_ir::tir::ops::alloc_task_operands(operands);
            layout.validate_closure_size(
                closure_size,
                payload_names.len(),
                false,
                GENERATOR_CONTROL_BYTES,
            );
            let out_name = op
                .out
                .as_ref()
                .expect("alloc_task requires an owned result");
            let poll_func_name = op
                .reference_symbol(function_table)
                .expect("alloc_task target missing");
            let poll_linkage = if op.references_body(function_table) {
                Linkage::Export
            } else {
                Linkage::Import
            };
            let poll_addr =
                SimpleBackend::task_poll_identity(module, builder, poll_func_name, poll_linkage);
            // A compiled poll's frame is its frame-size word: the lowering adds
            // spill slots past the frontend layout `value` records, as a task
            // trampoline reads. A runtime-native poll's frame is the layout.
            let frame_bytes = if op.references_body(function_table) {
                crate::native_backend::load_stateful_frame_size(module, builder, poll_func_name)
            } else {
                builder.ins().iconst(types::I64, closure_size)
            };
            let mut boxed = |builder: &mut FunctionBuilder<'_>, name: &str| {
                *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    name,
                    representation_plan,
                )
                .unwrap_or_else(|| panic!("alloc_task operand {name} not found"))
            };
            // The function is borrowed: the task retains its frame context.
            let function = boxed(builder, function_name);
            // A repeated value keeps one identity (`SameAs` its first slot); a
            // raw int is boxed by the task once it exists; every other word is
            // retained by its slot unless it owns nothing.
            let mut first_slot: BTreeMap<&str, usize> = BTreeMap::new();
            let mut payload: Vec<(Value, PayloadCustody)> = Vec::with_capacity(payload_names.len());
            for (slot, name) in payload_names.iter().enumerate() {
                if let Some(&first) = first_slot.get(name.as_str()) {
                    payload.push((payload[first].0, PayloadCustody::SameAs(first)));
                    continue;
                }
                first_slot.insert(name.as_str(), slot);
                if representation_plan.is_raw_int_carrier_name(name) {
                    let raw = *var_get(builder, vars, name)
                        .unwrap_or_else(|| panic!("alloc_task operand {name} not found"));
                    payload.push((raw, PayloadCustody::RawInt));
                    continue;
                }
                let custody = if representation_plan.boxed_refcount_is_noop(name) {
                    PayloadCustody::Unowned
                } else {
                    PayloadCustody::Borrowed
                };
                payload.push((boxed(builder, name), custody));
            }
            let sites = super::calls::native_frame_sites(module, import_ids, builder, import_refs);
            let tasks = super::calls::native_task_sites(module, import_ids, builder, import_refs);
            let tracking_origin = builder.current_block();
            let task = crate::native_backend::simple_backend::emit_task_construction(
                builder,
                sealed_blocks,
                sites,
                tasks,
                layout,
                poll_addr,
                frame_bytes,
                TaskFrameContext::Function(function),
                molt_codegen_abi::task_site::TaskPayload::Words(&payload),
            );
            if let Some(continuation) = builder.current_block() {
                carry_internal_cfg_tracking(
                    tracking_origin,
                    continuation,
                    block_tracked_obj,
                    block_tracked_ptr,
                );
            }
            *output_is_ptr = false;
            def_var_named(&mut *builder, vars, out_name, task);
        }
        OpKind::Store => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let origin_block = builder
                .current_block()
                .expect("store requires an active block");
            let mut origin_obj_live = block_tracked_obj.remove(&origin_block).unwrap_or_default();
            let origin_obj_cleanup = drain_cleanup_candidates(
                rc_authority,
                &mut origin_obj_live,
                last_use,
                op_idx,
                None,
            );
            let mut origin_ptr_live = block_tracked_ptr.remove(&origin_block).unwrap_or_default();
            let origin_ptr_cleanup = drain_cleanup_candidates(
                rc_authority,
                &mut origin_ptr_live,
                last_use,
                op_idx,
                None,
            );
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
            )
            .expect("Value not found");
            let offset =
                i32::try_from(op.value.unwrap_or(0)).expect("a proven field offset fits an i32");
            let sites = super::calls::native_frame_sites(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
            );
            let cold = import_runtime_func_ref(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                RuntimeFn::ObjectFieldSetPtr,
            );
            let value = *val;
            crate::native_backend::simple_backend::emit_field_slot_set(
                builder,
                sealed_blocks,
                sites,
                *obj,
                offset,
                value,
                |builder, pointer| {
                    let offset = builder.ins().iconst(types::I64, i64::from(offset));
                    builder.ins().call(cold, &[pointer, offset, value]);
                },
            );
            let merge_block = builder
                .current_block()
                .expect("a field store continues in a block");
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
                cleanup_roots.release(builder, local_dec_ref_obj, &name);
            }
            for name in origin_ptr_cleanup {
                cleanup_roots.release(builder, local_dec_ref_obj, &name);
            }
            if let Some(out_name) = simple_ir_out_result(op) {
                let none_val = builder.ins().iconst(types::I64, box_none());
                def_var_named(&mut *builder, vars, out_name, none_val);
            }
        }
        OpKind::Load => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Object not found");
            let res = emit_proven_field_get(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                *obj,
                op.value.unwrap_or(0),
            );
            let Some(out_name) = simple_ir_out_result(op) else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        OpKind::ClosureLoad => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
            let obj_ptr = if stateful && args.first().map(String::as_str) == Some("self") {
                compiled_argument(builder, 0)
            } else {
                let obj = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                )
                .expect("Object not found");
                unbox_ptr_value(&mut *builder, *obj)
            };
            let sites = super::calls::native_frame_sites(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
            );
            let res = crate::native_backend::simple_backend::emit_closure_load(
                &mut *builder,
                &mut *sealed_blocks,
                sites,
                obj_ptr,
                offset,
            );
            let Some(out_name) = simple_ir_out_result(op) else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        OpKind::ClosureStore => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Value not found");
            let offset = builder.ins().iconst(types::I64, op.value.unwrap_or(0));
            let obj_ptr = if stateful && args.first().map(String::as_str) == Some("self") {
                compiled_argument(builder, 0)
            } else {
                let obj = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                )
                .expect("Object not found");
                unbox_ptr_value(&mut *builder, *obj)
            };
            let callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                RuntimeFn::ClosureStore,
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let sites = super::calls::native_frame_sites(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
            );
            let value = *val;
            crate::native_backend::simple_backend::emit_closure_store(
                &mut *builder,
                &mut *sealed_blocks,
                sites,
                obj_ptr,
                offset,
                value,
                |builder| {
                    builder.ins().call(local_callee, &[obj_ptr, offset, value]);
                },
            );
            if let Some(out_name) = simple_ir_out_result(op) {
                let none = builder.ins().iconst(
                    types::I64,
                    (molt_codegen_abi::QNAN | molt_codegen_abi::TAG_NONE) as i64,
                );
                def_var_named(&mut *builder, vars, out_name, none);
            }
        }
        OpKind::GuardedLoad => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Object not found");
            let res = emit_proven_field_get(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                *obj,
                op.value.unwrap_or(0),
            );
            let Some(out_name) = simple_ir_out_result(op) else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        OpKind::GuardedFieldGet => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Object not found");
            let expected_version = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Expected version not found");
            let definition_slot = emit_class_definition_slot(op, module, builder);
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
            let callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                RuntimeFn::GuardedFieldGet,
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[
                    *obj,
                    definition_slot,
                    *expected_version,
                    offset,
                    attr_ptr,
                    attr_len,
                ],
            );
            let res = builder.inst_results(call)[0];
            let Some(out_name) = simple_ir_out_result(op) else {
                return OpFlow::Continue;
            };
            def_var_named(&mut *builder, vars, out_name, res);
        }
        OpKind::GuardedFieldSet => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Object not found");
            let expected_version = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Expected version not found");
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Value not found");
            let definition_slot = emit_class_definition_slot(op, module, builder);
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
            let callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                RuntimeFn::GuardedFieldSet,
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(
                local_callee,
                &[
                    *obj,
                    definition_slot,
                    *expected_version,
                    offset,
                    *val,
                    attr_ptr,
                    attr_len,
                ],
            );
            if let Some(out_name) = simple_ir_out_result(op) {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name, res);
            }
        }
        OpKind::GuardType | OpKind::GuardTag => {
            let args = op
                .args
                .as_ref()
                .expect("guard requires value and expected tag");
            // The runtime flag is frozen for an activation's runtime epoch.
            // Tag-valid guards only observe profiling; disabled profiling must
            // not materialize wide raw integers just to return them unchanged.
            let profile_join = if guard_profile_only {
                let enabled = profile_enabled_val.expect("guard profile flag");
                let origin = builder.current_block();
                let check = builder.create_block();
                let skip = builder.create_block();
                let join = builder.create_block();
                let enabled = builder.ins().icmp_imm_s(IntCC::NotEqual, enabled, 0);
                builder.ins().brif(enabled, check, &[], skip, &[]);
                switch_to_block_materialized(builder, skip);
                seal_block_once(builder, sealed_blocks, skip);
                if let Some(out) = simple_ir_out_result(op) {
                    super::value_transfer::define_alias_result(
                        op,
                        &args[0],
                        out,
                        None,
                        module,
                        import_ids,
                        builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        representation_plan,
                        alias_roots,
                        rc_authority,
                        local_inc_ref_obj,
                        nbc,
                    );
                }
                jump_block(builder, join, &[]);
                switch_to_block_materialized(builder, check);
                seal_block_once(builder, sealed_blocks, check);
                carry_internal_cfg_tracking(origin, check, block_tracked_obj, block_tracked_ptr);
                Some(join)
            } else {
                None
            };
            let mut transaction = NativeOperandTransaction::begin(
                builder,
                representation_plan,
                args.iter().map(String::as_str),
            );
            let value = transaction.operand(
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
            let expected = transaction.operand(
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
            let guard = import_runtime_func_ref(
                module,
                import_ids,
                builder,
                import_refs,
                RuntimeFn::GuardType,
            );
            builder.ins().call(guard, &[value, expected]);
            if let Some(out) = simple_ir_out_result(op) {
                super::value_transfer::define_alias_result(
                    op,
                    &args[0],
                    out,
                    Some(value),
                    module,
                    import_ids,
                    builder,
                    import_refs,
                    sealed_blocks,
                    vars,
                    representation_plan,
                    alias_roots,
                    rc_authority,
                    local_inc_ref_obj,
                    nbc,
                );
            }
            transaction.finish_operation(
                op,
                module,
                import_ids,
                builder,
                import_refs,
                sealed_blocks,
                vars,
                representation_plan,
                nbc,
                block_tracked_obj,
                block_tracked_ptr,
            );
            if let Some(join) = profile_join {
                carry_internal_cfg_tracking(
                    builder.current_block(),
                    join,
                    block_tracked_obj,
                    block_tracked_ptr,
                );
                jump_block(builder, join, &[]);
                switch_to_block_materialized(builder, join);
                seal_block_once(builder, sealed_blocks, join);
            }
        }
        OpKind::GuardLayout => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Guard object not found");
            let expected_version = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Guard version not found");
            let definition_slot = emit_class_definition_slot(op, module, builder);
            let callee = SimpleBackend::import_runtime_func_id_split(
                &mut *module,
                &mut *import_ids,
                RuntimeFn::GuardLayout,
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*obj, definition_slot, *expected_version]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = simple_ir_out_result(op) {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handle_memory_op received non-memory op `{}`", op.kind),
    }

    OpFlow::Proceed
}
