use super::super::*;

/// Single-source kind authority for [`handle_class_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "class_new",
    "class_def",
    "class_layout_version",
    "class_set_layout_version",
    "class_merge_layout",
    "class_set_base",
    "class_apply_set_name",
    "object_set_class",
];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for class-object ops: `class_new`/`class_def`/`set_base`/`apply_set_name`/`layout_version`/`set_layout_version`/`merge_layout` and `object_set_class`.
///
/// Extracted verbatim from `compile_func_inner`'s per-op dispatch (M1).
/// Each arm body is byte-for-byte identical to the original; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed params).
/// The op-local closure `var_get_boxed_overflow_safe` is reconstructed with the
/// same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_class_op(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
    // Reconstruct the original op-local closure (captures representation_plan +
    // nbc; all other state threads through explicit params) so the moved arm
    // bodies call it exactly as they did inline.
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
        "class_new" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let name_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Class name not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_class_new",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*name_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "class_def" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let meta = op.s_value.as_ref().expect("class_def needs s_value");
            let parts: Vec<&str> = meta.split(',').collect();
            let nbases: usize = parts[0].parse().unwrap();
            let nattrs: usize = parts[1].parse().unwrap();
            let layout_size: i64 = parts[2].parse().unwrap();
            let layout_version: i64 = parts[3].parse().unwrap();
            let flags: i64 = parts[4].parse().unwrap();
            let name_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Class name not found");
            let bases_slot_size = std::cmp::max(nbases, 1) * 8;
            let bases_slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                bases_slot_size as u32,
                3,
            ));
            for i in 0..nbases {
                let base = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[1 + i],
                    representation_plan,
                )
                .expect("Base class not found");
                builder.ins().stack_store(*base, bases_slot, (i * 8) as i32);
            }
            let bases_ptr = builder.ins().stack_addr(types::I64, bases_slot, 0);
            let attrs_slot_size = std::cmp::max(nattrs * 2, 1) * 8;
            let attrs_slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                attrs_slot_size as u32,
                3,
            ));
            let attrs_base = 1 + nbases;
            for i in 0..nattrs {
                let key = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[attrs_base + i * 2],
                    representation_plan,
                )
                .expect("Attr key not found");
                let val = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[attrs_base + i * 2 + 1],
                    representation_plan,
                )
                .expect("Attr value not found");
                builder
                    .ins()
                    .stack_store(*key, attrs_slot, (i * 2 * 8) as i32);
                builder
                    .ins()
                    .stack_store(*val, attrs_slot, ((i * 2 + 1) * 8) as i32);
            }
            let attrs_ptr = builder.ins().stack_addr(types::I64, attrs_slot, 0);
            let nbases_val = builder.ins().iconst(types::I64, nbases as i64);
            let nattrs_val = builder.ins().iconst(types::I64, nattrs as i64);
            let layout_size_val = builder.ins().iconst(types::I64, layout_size);
            let layout_version_val = builder.ins().iconst(types::I64, layout_version);
            let flags_val = builder.ins().iconst(types::I64, flags);
            let cd_callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_guarded_class_def",
                &[
                    types::I64,
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
            let cd_local = module.declare_func_in_func(cd_callee, builder.func);
            let cd_call = builder.ins().call(
                cd_local,
                &[
                    *name_bits,
                    bases_ptr,
                    nbases_val,
                    attrs_ptr,
                    nattrs_val,
                    layout_size_val,
                    layout_version_val,
                    flags_val,
                ],
            );
            let res = builder.inst_results(cd_call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "class_layout_version" => {
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
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_class_layout_version",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*class_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "class_set_layout_version" => {
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
            let version_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Version not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_class_set_layout_version",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*class_bits, *version_bits]);
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name.clone(), res);
            }
        }
        "class_merge_layout" => {
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
            let offsets_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Offsets not found");
            let size_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Size not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_class_merge_layout",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*class_bits, *offsets_bits, *size_bits]);
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                let res = builder.inst_results(call)[0];
                def_var_named(&mut *builder, vars, out_name.clone(), res);
            }
        }
        "class_set_base" => {
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
            let base_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Base class not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_class_set_base",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*class_bits, *base_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "class_apply_set_name" => {
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
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_class_apply_set_name",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*class_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "object_set_class" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let obj_bits = var_get_boxed_overflow_safe(
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
            let obj_ptr = unbox_ptr_value(&mut *builder, *obj_bits, nbc);
            let class_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Class not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_object_set_class",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[obj_ptr, *class_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
