use super::super::*;

/// Single-source kind authority for [`handle_text_transform`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "bytearray_fill_range",
    "string_format",
    "string_join",
    "string_split",
    "string_split_validate",
    "string_split_field",
    "string_split_field_len",
    "string_split_field_eq",
    "string_split_field_start",
    "string_split_field_end",
    "string_split_field_is_ascii",
    "string_split_field_len_from_bounds",
    "string_split_field_ord_at_bounds",
    "string_split_field_to_int",
    "string_split_max",
    "string_lower",
    "string_upper",
    "string_capitalize",
    "string_strip",
    "string_lstrip",
    "string_rstrip",
    "string_replace",
    "bytes_split",
    "bytes_split_max",
    "bytearray_split",
    "bytearray_split_max",
    "bytes_replace",
    "bytearray_replace",
];
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
    // Compact emitter for the split-field-deforestation runtime helpers
    // (`string_split_field_bounds`/`..._start`/`..._end`/`..._is_ascii`/
    // `..._len_from_bounds`/`..._ord_at_bounds`/`..._to_int`): each reads N boxed
    // operands, calls the same-named `molt_` symbol with N `I64` args + one `I64`
    // result, and binds the single result. Their arg-arities differ but the
    // shape is identical, so one closure removes the ~30-line-per-op boilerplate.
    let emit_split_field_op = |module: &mut ObjectModule,
                               import_ids: &mut BTreeMap<
        &'static str,
        (cranelift_module::FuncId, ImportSignatureShape),
    >,
                               builder: &mut FunctionBuilder<'_>,
                               import_refs: &mut BTreeMap<&'static str, FuncRef>,
                               sealed_blocks: &mut BTreeSet<Block>,
                               symbol: &'static str,
                               op: &OpIR| {
        let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
        let mut arg_vals = Vec::with_capacity(args.len());
        let mut sig = Vec::with_capacity(args.len());
        for name in args {
            let v = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                name,
                int_primary_vars,
                float_primary_vars,
            )
            .expect("split-field deforestation operand not found");
            arg_vals.push(*v);
            sig.push(types::I64);
        }
        let callee = SimpleBackend::import_func_id_split(
            &mut *module,
            &mut *import_ids,
            symbol,
            &sig,
            &[types::I64],
        );
        let local_callee = module.declare_func_in_func(callee, builder.func);
        let call = builder.ins().call(local_callee, &arg_vals);
        let res = builder.inst_results(call)[0];
        if let Some(out__) = op.out.as_ref() {
            def_var_named(&mut *builder, vars, out__, res);
        }
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
        "string_split_field_start" => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_start",
            op,
        ),
        "string_split_field_end" => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_end",
            op,
        ),
        "string_split_field_is_ascii" => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_is_ascii",
            op,
        ),
        "string_split_field_len_from_bounds" => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_len_from_bounds",
            op,
        ),
        "string_split_field_ord_at_bounds" => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_ord_at_bounds",
            op,
        ),
        "string_split_field_to_int" => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_to_int",
            op,
        ),
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
