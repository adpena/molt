use super::super::*;
use super::list_index_fast_path::{ListIndexFastPathState, store_index_fallback_import_name};
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
    nbc: &crate::NanBoxConsts,
) {
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
    // Runtime dispatch is the live representation authority: compact int/bool
    // lists store directly only while their physical type remains specialized;
    // ABI publication promotes them to the generic transactional list.
    let fn_name = store_index_fallback_import_name(representation_plan, op);
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
