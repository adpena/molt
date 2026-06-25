use super::super::*;

/// Single-source kind authority for [`handle_dict_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "dict_new",
    "dict_from_obj",
    "dict_get",
    "dict_set",
    "dict_update_missing",
    "dict_inc",
    "dict_str_int_inc",
    "string_split_ws_dict_inc",
    "taq_ingest_line",
    "string_split_sep_dict_inc",
    "dict_pop",
    "dict_setdefault",
    "dict_setdefault_empty_list",
    "dict_update",
    "dict_clear",
    "dict_copy",
    "dict_popitem",
    "dict_update_kwstar",
    "dict_keys",
    "dict_values",
    "dict_items",
];
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for `dict` ops: construction (`dict_new`/`dict_from_obj`), access/mutation (`get`/`set`/`pop`/`setdefault`/`update`/`update_missing`/`clear`/`copy`/`popitem`), counting builders (`dict_inc`/`dict_str_int_inc`/`string_split_*_dict_inc`/`taq_ingest_line`), and views (`keys`/`values`/`items`).
///
/// Extracted verbatim from `compile_func_inner`'s per-op dispatch (M1).
/// Each arm body is byte-for-byte identical to the original; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed params,
/// outer-loop `continue`/`break` -> `OpFlow` returns).
/// The op-local closure `var_get_boxed_overflow_safe` is reconstructed with the
/// same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_dict_op(
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
    scalar_fast_paths_enabled: bool,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
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
    let op_index_key_is_integer_family = |op: &OpIR| {
        scalar_fast_paths_enabled && representation_plan.op_index_key_is_integer_family(op)
    };
    match op.kind.as_str() {
        "dict_new" => {
            let empty_args: Vec<String> = Vec::new();
            let args = op.args.as_ref().unwrap_or(&empty_args);
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            let size = builder.ins().iconst(types::I64, (args.len() / 2) as i64);

            let new_callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_new",
                &[types::I64],
                &[types::I64],
            );
            let new_local = module.declare_func_in_func(new_callee, builder.func);
            let new_call = builder.ins().call(new_local, &[size]);
            let dict_bits = builder.inst_results(new_call)[0];

            let set_callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_set",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let set_local = module.declare_func_in_func(set_callee, builder.func);
            let mut current = dict_bits;
            for pair in args.chunks(2) {
                let key = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &pair[0],
                    representation_plan,
                )
                .expect("Dict key not found");
                let val = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &pair[1],
                    representation_plan,
                )
                .expect("Dict val not found");
                let set_call = builder.ins().call(set_local, &[current, *key, *val]);
                current = builder.inst_results(set_call)[0];
            }
            def_var_named(&mut *builder, vars, out_name, current);
        }
        "dict_from_obj" => {
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
            .expect("Dict source not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_from_obj",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*obj]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_get" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let key = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict key not found");
            let default = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Dict default not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_get",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict, *key, *default]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_inc" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let key = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict key not found");
            let delta = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Dict increment value not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_inc",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict, *key, *delta]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_str_int_inc" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let key = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict key not found");
            let delta = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Dict increment value not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_str_int_inc",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict, *key, *delta]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_split_ws_dict_inc" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let line = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Line not found");
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict not found");
            let delta = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Delta not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_split_ws_dict_inc",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*line, *dict, *delta]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "taq_ingest_line" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let line = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Line not found");
            let bucket_size = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Bucket size not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_taq_ingest_line",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*dict, *line, *bucket_size]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "string_split_sep_dict_inc" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let line = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Line not found");
            let sep = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Separator not found");
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Dict not found");
            let delta = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                representation_plan,
            )
            .expect("Delta not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_split_sep_dict_inc",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*line, *sep, *dict, *delta]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_pop" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let key = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict key not found");
            let default = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Dict default not found");
            let has_default = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[3],
                representation_plan,
            )
            .expect("Dict default flag not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_pop",
                &[types::I64, types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*dict, *key, *default, *has_default]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_setdefault" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let key = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict key not found");
            let default = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Dict default not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_setdefault",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict, *key, *default]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_setdefault_empty_list" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let key = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict key not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_setdefault_empty_list",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict, *key]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_update" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let other = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict update iterable not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_update",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict, *other]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_clear" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_clear",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_copy" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_copy",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_popitem" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_popitem",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_update_kwstar" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let other = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Dict update mapping not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_update_kwstar",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict, *other]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_keys" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_keys",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_values" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_values",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_items" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Dict not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_items",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*dict]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "dict_set" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Dict not found in {} op {}", func_name, op_idx));
            let key_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Key not found in {} op {}", func_name, op_idx));
            let val_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Value not found in {} op {}", func_name, op_idx));
            // Dispatch based on container specialization:
            // - list_int: flat i64 storage (Codon-style)
            // - fast_int: generic list but key is known int
            // - default: full dict/generic dispatch
            if representation_plan.op_has_container_storage(
                op_idx,
                op,
                ContainerStorageKind::FlatListInt,
            ) {
                // raw_int_shadow fast path for list_int dict_set.
                // Inside loops, use Variable-only shadows (phi-correct).
                let raw_key_opt = int_raw_value(&mut *builder, vars, representation_plan, &args[1]);
                let raw_val_opt = int_raw_value(&mut *builder, vars, representation_plan, &args[2]);
                if let (Some(raw_key), Some(raw_val)) = (raw_key_opt, raw_val_opt) {
                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_list_int_setitem_unchecked",
                        &[types::I64, types::I64, types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict_bits, raw_key, raw_val]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out.as_ref() {
                        def_var_named(&mut *builder, vars, out__, res);
                    }
                } else {
                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_list_int_setitem",
                        &[types::I64, types::I64, types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let call = builder
                        .ins()
                        .call(local_callee, &[*dict_bits, *key_bits, *val_bits]);
                    let res = builder.inst_results(call)[0];
                    if let Some(out__) = op.out.as_ref() {
                        def_var_named(&mut *builder, vars, out__, res);
                    }
                }
            } else {
                let fn_name = if op_index_key_is_integer_family(op) {
                    "molt_list_setitem_int_fast"
                } else {
                    "molt_dict_set"
                };
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    fn_name,
                    &[types::I64, types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder
                    .ins()
                    .call(local_callee, &[*dict_bits, *key_bits, *val_bits]);
                let res = builder.inst_results(call)[0];
                if let Some(out__) = op.out.as_ref() {
                    def_var_named(&mut *builder, vars, out__, res);
                }
            }
        }
        "dict_update_missing" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let dict_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Dict not found in {} op {}", func_name, op_idx));
            let key_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Key not found in {} op {}", func_name, op_idx));
            let val_bits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .unwrap_or_else(|| panic!("Value not found in {} op {}", func_name, op_idx));
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_dict_update_missing",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*dict_bits, *key_bits, *val_bits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
    OpFlow::Proceed
}
