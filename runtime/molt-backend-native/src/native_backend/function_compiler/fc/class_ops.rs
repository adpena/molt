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
/// Constructor operands and results follow the shared fixed-constructor
/// ownership transaction, including the first failed materialization.
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
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
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
            emit_fixed_aggregate_constructor(
                op,
                FixedAggregateConstructor::ClassNew,
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
        }
        "class_def" => {
            let meta = op.s_value.as_deref().expect("class_def needs s_value");
            let mut parts = meta.split(',');
            let nbases = parts
                .next()
                .expect("class_def needs base count")
                .parse()
                .expect("class_def base count must fit usize");
            let nattrs = parts
                .next()
                .expect("class_def needs attribute count")
                .parse()
                .expect("class_def attribute count must fit usize");
            let layout_size = parts
                .next()
                .expect("class_def needs layout size")
                .parse()
                .expect("class_def layout size must fit i64");
            let layout_version = parts
                .next()
                .expect("class_def needs layout version")
                .parse()
                .expect("class_def layout version must fit i64");
            let flags = parts
                .next()
                .expect("class_def needs flags")
                .parse()
                .expect("class_def flags must fit i64");
            assert!(
                parts.next().is_none(),
                "class_def has extra metadata fields"
            );
            emit_fixed_aggregate_constructor(
                op,
                FixedAggregateConstructor::ClassDefinition {
                    nbases,
                    nattrs,
                    layout_size,
                    layout_version,
                    flags,
                },
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
