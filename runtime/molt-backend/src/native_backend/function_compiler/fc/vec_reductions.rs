use super::super::*;
use super::var_get_boxed_overflow_safe_fn;

/// The 24 `vec_*` reduction kinds — the SINGLE authority for this family, routed
/// to `NativeOpFamily::Arith` (whose [`super::arith::handle_arith_op`] delegates
/// here). Dropping the dispatch's separate copy of this list was the
/// `8b5773878` regression fixed in `0323ad28c`; with one authority that drift is
/// unexpressible. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "vec_sum_int",
    "vec_sum_int_trusted",
    "vec_sum_int_range",
    "vec_sum_int_range_trusted",
    "vec_sum_int_range_iter",
    "vec_sum_int_range_iter_trusted",
    "vec_sum_float",
    "vec_sum_float_trusted",
    "vec_sum_float_range",
    "vec_sum_float_range_trusted",
    "vec_sum_float_range_iter",
    "vec_sum_float_range_iter_trusted",
    "vec_prod_int",
    "vec_prod_int_trusted",
    "vec_prod_int_range",
    "vec_prod_int_range_trusted",
    "vec_min_int",
    "vec_min_int_trusted",
    "vec_min_int_range",
    "vec_min_int_range_trusted",
    "vec_max_int",
    "vec_max_int_trusted",
    "vec_max_int_range",
    "vec_max_int_range_trusted",
];

/// Cranelift codegen handlers for the `vec_*` reduction op family (sum/prod/min/max over int and float sequences, plus their `_trusted`/`_range`/`_range_iter` variants).
///
/// Extracted verbatim from `compile_func_inner`'s per-op dispatch (M1 phase 1).
/// Each arm body is byte-for-byte identical to the original; only the access
/// path to the backend's split-borrowed fields changed (`self.module` ->
/// `module`, `Self::` -> `SimpleBackend::`, owned locals -> reborrowed `&mut`
/// params). The op-local closure `var_get_boxed_overflow_safe` is reconstructed
/// with the same capture so the arm bodies are unchanged.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_vec_reduction(
    op: &OpIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    int_carriers_plan: &ScalarRepresentationPlan,
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
                                       int_carriers_plan: &ScalarRepresentationPlan,
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
            int_carriers_plan,
            float_primary_vars,
            bool_primary_vars,
            nbc,
        )
    };
    match op.kind.as_str() {
        "vec_sum_int" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_int",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_int_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_int_trusted",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_int_range" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_int_range",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_int_range_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_int_range_trusted",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_int_range_iter" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_int_range_iter",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_int_range_iter_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_int_range_iter_trusted",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_float" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_float",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_float_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_float_trusted",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_float_range" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_float_range",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_float_range_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_float_range_trusted",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_float_range_iter" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_float_range_iter",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_sum_float_range_iter_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_sum_float_range_iter_trusted",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_prod_int" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_prod_int",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_prod_int_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_prod_int_trusted",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_prod_int_range" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_prod_int_range",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_prod_int_range_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_prod_int_range_trusted",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_min_int" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_min_int",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_min_int_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_min_int_trusted",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_min_int_range" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_min_int_range",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_min_int_range_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_min_int_range_trusted",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_max_int" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_max_int",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_max_int_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_max_int_trusted",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_max_int_range" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_max_int_range",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "vec_max_int_range_trusted" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let seq = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Seq arg not found");
            let acc = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Acc arg not found");
            let start = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                int_carriers_plan,
                float_primary_vars,
            )
            .expect("Start arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_vec_max_int_range_trusted",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*seq, *acc, *start]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("handler invoked with non-matching op.kind"),
    }
}
