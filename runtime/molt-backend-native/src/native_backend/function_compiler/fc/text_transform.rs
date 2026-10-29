use super::super::*;
use molt_ir::OpKind;

// Single-source kind authority for [`handle_text_transform`], consulted by
// `op_family::native_op_family`. Mirror the `match op.kind` arms below.
#[cfg(feature = "native-backend")]
define_native_op_kinds!(handled_kinds_pattern, HANDLED_KINDS;
    OpKind::BytearrayFillRange,
    OpKind::StringFormat,
    OpKind::StringJoin,
    OpKind::StringSplit,
    OpKind::StringSplitValidate,
    OpKind::StringSplitField,
    OpKind::StringSplitFieldLen,
    OpKind::StringSplitFieldEq,
    OpKind::StringSplitFieldStart,
    OpKind::StringSplitFieldEnd,
    OpKind::StringSplitFieldIsAscii,
    OpKind::StringSplitFieldLenFromBounds,
    OpKind::StringSplitFieldOrdAtBounds,
    OpKind::StringSplitFieldToInt,
    OpKind::StringSplitMax,
    OpKind::StringLower,
    OpKind::StringUpper,
    OpKind::StringCapitalize,
    OpKind::StringStrip,
    OpKind::StringLstrip,
    OpKind::StringRstrip,
    OpKind::StringReplace,
    OpKind::BytesSplit,
    OpKind::BytesSplitMax,
    OpKind::BytearraySplit,
    OpKind::BytearraySplitMax,
    OpKind::BytesReplace,
    OpKind::BytearrayReplace,
);
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
    module: &mut dyn Module,
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
    // Compact emitter for the split-field-deforestation runtime helpers
    // (`string_split_field_bounds`/`..._start`/`..._end`/`..._is_ascii`/
    // `..._len_from_bounds`/`..._ord_at_bounds`/`..._to_int`): each reads N boxed
    // operands, calls the same-named `molt_` symbol with N `I64` args + one `I64`
    // result, and binds the single result. Their arg-arities differ but the
    // shape is identical, so one closure removes the ~30-line-per-op boilerplate.
    let emit_split_field_op = |module: &mut dyn Module,
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
                representation_plan,
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
    match op.kind {
        OpKind::BytearrayFillRange => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let bytearray = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::StringFormat => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let val = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::StringJoin => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let sep = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::StringSplit => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::StringSplitValidate => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::StringSplitField => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::StringSplitFieldLen => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::StringSplitFieldEq => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::StringSplitMax => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::StringLower => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
        OpKind::StringUpper => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
        OpKind::StringCapitalize => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
        OpKind::StringStrip => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::StringLstrip => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::StringRstrip => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::StringReplace => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::BytesSplit => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::BytesSplitMax => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::BytearraySplit => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
        OpKind::BytearraySplitMax => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::BytesReplace => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::BytearrayReplace => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let hay = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
                representation_plan,
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
        OpKind::StringSplitFieldStart => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_start",
            op,
        ),
        OpKind::StringSplitFieldEnd => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_end",
            op,
        ),
        OpKind::StringSplitFieldIsAscii => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_is_ascii",
            op,
        ),
        OpKind::StringSplitFieldLenFromBounds => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_len_from_bounds",
            op,
        ),
        OpKind::StringSplitFieldOrdAtBounds => emit_split_field_op(
            &mut *module,
            &mut *import_ids,
            &mut *builder,
            &mut *import_refs,
            &mut *sealed_blocks,
            "molt_string_split_field_ord_at_bounds",
            op,
        ),
        OpKind::StringSplitFieldToInt => emit_split_field_op(
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
