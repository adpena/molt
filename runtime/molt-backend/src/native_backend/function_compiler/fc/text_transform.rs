use super::super::*;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for text transform ops: `format`/`join`/`split`(+field/max/validate)/`lower`/`upper`/`capitalize`/`strip`/`replace` over `str`, plus `split`/`replace`/`fill_range` over `bytes`/`bytearray`.
///
/// Extracted verbatim from `compile_func_inner`'s per-op dispatch (M1 phase 1).
/// Each arm body is byte-for-byte identical to the original; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed `&mut`
/// params). The op-local closure `var_get_boxed_overflow_safe` is reconstructed
/// with the same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_text_transform(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    int_primary_vars: &BTreeSet<String>,
    float_primary_vars: &BTreeSet<String>,
    bool_primary_vars: &BTreeSet<String>,
    nbc: &crate::NanBoxConsts,
    box_int_mask_var: Variable,
    box_int_tag_var: Variable,
) {
    // Reconstruct the original op-local closure (captures bool_primary_vars +
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
                                       int_primary_vars: &BTreeSet<String>,
                                       float_primary_vars: &BTreeSet<String>,
                                       box_int_mask_var: Variable,
                                       box_int_tag_var: Variable|
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
            box_int_mask_var,
            box_int_tag_var,
        )
    };
    match op.kind.as_str() {
        "bytearray_fill_range" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let bytearray = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("bytearray fill target not found");
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
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("bytearray fill start not found");
            let stop = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("bytearray fill stop not found");
            let value = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("bytearray fill value not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytearray_fill_range",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*bytearray, *start, *stop, *value]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref().filter(|out__| **out__ != "none") {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_format" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Format value not found");
            let spec = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Format spec not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_format_builtin",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*val, *spec]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_join" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let sep = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Join separator not found");
            let items = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Join items not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_join",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*sep, *items]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_split" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_split",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *needle]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_split_validate" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_split_validate",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *needle]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_split_field" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let index = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split field index not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_split_field",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *needle, *index]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_split_field_len" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let index = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split field index not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_split_field_len",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *needle, *index]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_split_field_eq" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let index = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split field index not found");
            let expected = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split field expected string not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_split_field_eq",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*hay, *needle, *index, *expected]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_split_max" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let maxsplit = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split maxsplit not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_split_max",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*hay, *needle, *maxsplit]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_lower" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Lower string not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_lower",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_upper" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Upper string not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_upper",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_capitalize" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Capitalize string not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_capitalize",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_strip" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Strip string not found");
            let chars = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Strip chars not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_strip",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *chars]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_lstrip" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Lstrip string not found");
            let chars = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Lstrip chars not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_lstrip",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *chars]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_rstrip" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Rstrip string not found");
            let chars = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Rstrip chars not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_rstrip",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *chars]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_replace" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace needle not found");
            let replacement = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace replacement not found");
            let count = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace count not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_replace",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*hay, *needle, *replacement, *count]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "bytes_split" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytes_split",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *needle]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "bytes_split_max" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let maxsplit = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split maxsplit not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytes_split_max",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*hay, *needle, *maxsplit]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "bytearray_split" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytearray_split",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*hay, *needle]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "bytearray_split_max" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split needle not found");
            let maxsplit = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Split maxsplit not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytearray_split_max",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*hay, *needle, *maxsplit]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "bytes_replace" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace needle not found");
            let replacement = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace replacement not found");
            let count = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace count not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytes_replace",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*hay, *needle, *replacement, *count]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "bytearray_replace" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace haystack not found");
            let needle = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace needle not found");
            let replacement = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace replacement not found");
            let count = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                int_primary_vars,
                float_primary_vars,
                box_int_mask_var,
                box_int_tag_var,
            )
            .expect("Replace count not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_bytearray_replace",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*hay, *needle, *replacement, *count]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
