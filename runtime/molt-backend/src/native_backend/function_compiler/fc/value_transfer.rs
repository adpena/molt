use super::super::*;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for value-custody transfer ops: `inc_ref`,
/// `borrow`, `dec_ref`, `release`, `box`, `unbox`, `cast`, `widen`, and
/// retained alias ops. This owns alias-preserving refcount adjustment and
/// tracked cleanup-root scrubbing for explicit release operations.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_value_transfer_op(
    op: &OpIR,
    op_idx: usize,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    tracked_obj_vars: &mut Vec<String>,
    tracked_vars: &mut Vec<String>,
    tracked_obj_vars_set: &mut std::collections::HashSet<String>,
    tracked_vars_set: &mut std::collections::HashSet<String>,
    alias_roots: &BTreeMap<String, String>,
    entry_vars: &mut BTreeMap<String, Value>,
    already_decrefed: &mut BTreeSet<String>,
    rc_skip_inc: &std::collections::HashSet<usize>,
    local_inc_ref_obj: FuncRef,
    local_dec_ref_obj: FuncRef,
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
        "inc_ref" | "borrow" => {
            if !rc_skip_inc.contains(&op_idx) {
                let args_names = op.args.as_ref().expect("inc_ref/borrow args missing");
                let src_name = args_names
                    .first()
                    .expect("inc_ref/borrow requires one source arg");
                let src = *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    src_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("inc_ref/borrow source not found");
                emit_inc_ref_obj(&mut *builder, src, local_inc_ref_obj, nbc);
                if let Some(out_name) = op.out.as_ref()
                    && out_name != "none"
                {
                    def_var_named(&mut *builder, vars, out_name.clone(), src);
                }
            } else if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                // RC coalesced: still define the output variable as an
                // alias of the input so downstream ops can read it.
                let args_names = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                let src_name = args_names.first().unwrap();
                let src = *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    src_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("inc_ref/borrow source not found (coalesced)");
                def_var_named(&mut *builder, vars, out_name.clone(), src);
            }
        }
        "dec_ref" | "release" => {
            let args_names = op.args.as_ref().expect("dec_ref/release args missing");
            let src_name = args_names
                .first()
                .expect("dec_ref/release requires one source arg");
            if rc_skip_inc.contains(&op_idx) {
                // No runtime call needed.  Still define the output
                // variable so downstream SSA reads succeed.
                if let Some(out_name) = op.out.as_ref()
                    && out_name != "none"
                {
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    def_var_named(&mut *builder, vars, out_name.clone(), none_bits);
                }
            } else {
                let src = *var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    src_name,
                    int_primary_vars,
                    float_primary_vars,
                )
                .expect("dec_ref/release source not found");
                builder.ins().call(local_dec_ref_obj, &[src]);
                let consumed_root = alias_root_name(alias_roots, src_name).to_string();
                already_decrefed.insert(consumed_root.clone());
                let consumed_roots = BTreeSet::from([consumed_root]);
                scrub_tracked_roots(
                    &consumed_roots,
                    alias_roots,
                    tracked_vars,
                    tracked_obj_vars,
                    tracked_vars_set,
                    tracked_obj_vars_set,
                    entry_vars,
                    block_tracked_obj,
                    block_tracked_ptr,
                );
                if let Some(out_name) = op.out.as_ref()
                    && out_name != "none"
                {
                    let none_bits = builder.ins().iconst(types::I64, box_none());
                    def_var_named(&mut *builder, vars, out_name.clone(), none_bits);
                }
            }
        }
        "box" | "unbox" | "cast" | "widen" => {
            let args_names = op.args.as_ref().expect("conversion args missing");
            let src_name = args_names
                .first()
                .expect("conversion op requires one source arg");
            let src = *var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                src_name,
                int_primary_vars,
                float_primary_vars,
            )
            .expect("conversion source not found");
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                // Output aliases input bits — inc_ref to prevent
                // use-after-free when the input name is dec_ref'd
                // independently by tracking/check_exception cleanup.
                emit_inc_ref_obj(&mut *builder, src, local_inc_ref_obj, nbc);
                def_var_named(&mut *builder, vars, out_name.clone(), src);
            }
        }
        // `copy` is the frontend's args-based pure SSA value move
        // (`{kind:"copy", args:[src], out:result}`). It survives
        // `rewrite_copy_aliases` whenever its result/source is a mutable-storage
        // (reassigned-local) name, so it reaches codegen and must be lowered
        // here rather than silently dropped. It shares the alias lowering:
        // result = inc_ref'd alias of args[0]. The TIR ownership model classifies
        // `copy`/`identity_alias`/`binding_alias` identically as
        // `CopyLowering::TransparentAlias` (alias_analysis.rs), and WASM/Luau
        // group it with the alias ops the same way — the inc_ref + alias here is
        // the RC-correct, cross-backend-symmetric lowering.
        "copy" | "identity_alias" | "binding_alias" => {
            let args_names = op.args.as_ref().expect("alias args missing");
            let src_name = args_names
                .first()
                .expect("alias op requires one source arg");
            if let Some(out_name) = op.out.as_ref()
                && out_name != "none"
            {
                if float_primary_vars.contains(out_name) {
                    // Float-primary: transfer raw f64 directly.
                    let raw_f64 =
                        float_value_for(&mut *builder, vars, float_primary_vars, src_name)
                            .unwrap_or_else(|| {
                                let boxed = var_get_boxed_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    &mut *import_refs,
                                    &mut *sealed_blocks,
                                    vars,
                                    src_name,
                                    int_primary_vars,
                                    float_primary_vars,
                                )
                                .expect("alias source not found");
                                builder.ins().bitcast(types::F64, MemFlags::new(), *boxed)
                            });
                    def_var_named(&mut *builder, vars, out_name.clone(), raw_f64);
                } else {
                    let src = *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        src_name,
                        int_primary_vars,
                        float_primary_vars,
                    )
                    .expect("alias source not found");
                    // Same aliasing hazard as box/unbox/cast/widen above.
                    emit_inc_ref_obj(&mut *builder, src, local_inc_ref_obj, nbc);
                    def_var_named(&mut *builder, vars, out_name.clone(), src);
                }
            }
        }
        _ => unreachable!("non-value-transfer op routed to handle_value_transfer_op"),
    }
}
