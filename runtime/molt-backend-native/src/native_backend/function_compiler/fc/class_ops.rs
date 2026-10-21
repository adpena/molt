use super::super::*;

/// Single-source kind authority for [`handle_class_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["class_def", "class_set_definition"];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for the class-statement ops `class_def` and `class_set_definition`.
///
/// Constructor operands and results follow the shared fixed-constructor
/// ownership transaction, including the first failed materialization.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_class_op(
    op: &OpIR,
    module: &mut dyn Module,
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
    let var_get_boxed_overflow_safe = |module: &mut dyn Module,
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
        "class_set_definition" => {
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
            let definition_slot = emit_class_definition_slot(op, module, builder);
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_class_set_definition",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let _ = builder
                .ins()
                .call(local_callee, &[*class_bits, definition_slot]);
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
