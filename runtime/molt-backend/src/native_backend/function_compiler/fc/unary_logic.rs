use super::super::*;
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for unary and boolean logic ops: identity,
/// logical not, numeric unary operators, truth conversion, short-circuit value
/// selection, and containment. Extracted from `compile_func_inner` as a
/// move-only function split; only split-borrow access paths and outer-loop flow
/// signals changed.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_unary_logic_op(
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
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    local_inc_ref_obj: FuncRef,
    scalar_fast_paths_enabled: bool,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
    let name_is_integer_scalar = |name: &str| {
        int_like_vars.contains(name)
            || bool_like_vars.contains(name)
            || int_primary_vars.contains(name)
            || bool_primary_vars.contains(name)
    };
    let op_args_are_integer_scalar = |op: &OpIR| {
        op.args.as_ref().is_some_and(|args| {
            !args.is_empty() && args.iter().all(|arg| name_is_integer_scalar(arg))
        })
    };
    let op_prefers_int_lane = |op: &OpIR| {
        scalar_fast_paths_enabled
            && (representation_plan.op_scalar_lane(op) == Some(ScalarKind::Int)
                || (matches!(
                    op.kind.as_str(),
                    "add"
                        | "inplace_add"
                        | "sub"
                        | "inplace_sub"
                        | "mul"
                        | "inplace_mul"
                        | "floordiv"
                        | "inplace_floordiv"
                        | "mod"
                        | "mod_"
                        | "inplace_mod"
                        | "lt"
                        | "le"
                        | "gt"
                        | "ge"
                        | "eq"
                        | "ne"
                ) && op_args_are_integer_scalar(op)))
    };
    let op_prefers_integer_runtime_lane = |op: &OpIR| {
        scalar_fast_paths_enabled && representation_plan.op_prefers_integer_runtime_lane(op)
    };
    let op_prefers_bool_lane = |op: &OpIR| {
        scalar_fast_paths_enabled
            && representation_plan.op_scalar_lane(op) == Some(ScalarKind::Bool)
    };
    let op_prefers_float_lane = |op: &OpIR| {
        scalar_fast_paths_enabled
            && !op_prefers_integer_runtime_lane(op)
            && representation_plan.op_scalar_lane(op) == Some(ScalarKind::Float)
    };
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
        "is" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let lhs = var_get_boxed_overflow_safe(
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
            .expect("LHS not found");
            let rhs = var_get_boxed_overflow_safe(
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
            .expect("RHS not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_is",
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_bool_result(&mut *builder, vars, bool_primary_vars, out__, res, None);
            }
        }
        "not" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let mut not_raw: Option<Value> = None;
            let res = if let Some(raw_val) =
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[0])
            {
                // Raw bool: not(x) = bool(x == 0). Skip runtime call.
                let is_zero = builder.ins().icmp_imm(IntCC::Equal, raw_val, 0);
                let result = box_bool_value(&mut *builder, is_zero, nbc);
                // The result of `not` is also a bool — store raw shadow.
                let negated = builder.ins().bxor_imm(raw_val, 1);
                let negated_masked = builder.ins().band_imm(negated, 1);
                not_raw = Some(negated_masked);
                result
            } else if op_prefers_bool_lane(op) {
                // NaN-boxed bool: extract bit 0 and flip.
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
                .expect("Value not found");
                let one = builder.ins().iconst(types::I64, 1);
                let bit0 = builder.ins().band(*val, one);
                let is_zero = builder.ins().icmp_imm(IntCC::Equal, bit0, 0);
                not_raw = Some(builder.ins().bxor_imm(bit0, 1));
                box_bool_value(&mut *builder, is_zero, nbc)
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_not",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
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
                .expect("Value not found");
                let call = builder.ins().call(local_callee, &[*val]);
                builder.inst_results(call)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_bool_result(&mut *builder, vars, bool_primary_vars, out__, res, not_raw);
            }
        }
        "neg" | "unary_neg" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_float_lane(op) {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let src_f = float_value_for(&mut *builder, vars, float_primary_vars, &args[0])
                    .unwrap_or_else(|| {
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
                        .expect("Value not found");
                        builder.ins().bitcast(types::F64, MemFlags::new(), *val)
                    });
                let neg_f = builder.ins().fneg(src_f);
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| float_primary_vars.contains(o))
                {
                    neg_f
                } else {
                    box_float_value(&mut *builder, neg_f, nbc)
                }
            } else if op_prefers_int_lane(op) {
                // -x == 0 - x; overflow deferred to boxing escape.
                let src_name = &args[0];
                let src_raw = int_raw_value(&mut *builder, vars, int_primary_vars, src_name);

                if let Some(src_raw) = src_raw {
                    // Raw i64 primary negation: branchless.
                    let zero = builder.ins().iconst(types::I64, 0);
                    let negated = builder.ins().isub(zero, src_raw);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, negated);
                    }
                    return OpFlow::Continue;
                } else {
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
                    .expect("Value not found");
                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_neg",
                        &[types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);

                    // op_prefers_int_lane only proves Python-`int` type,
                    // which includes heap BigInts (TAG_PTR). Guard the raw
                    // negate on a runtime inline-int tag check so a BigInt
                    // operand routes to `molt_neg` instead of being
                    // truncated by the trusted unbox.
                    let (val_xored, int_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *val, nbc);
                    let is_int = fused_both_int_check(&mut *builder, val_xored, val_xored, nbc);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let negated = builder.ins().isub(zero, int_val);
                    let fits_inline = int_value_fits_inline(&mut *builder, negated);
                    let take_fast = builder.ins().band(is_int, fits_inline);
                    builder
                        .ins()
                        .brif(take_fast, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    let fast_res = box_int_value(&mut *builder, negated, nbc);
                    jump_block(&mut *builder, merge_block, &[fast_res]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let slow_res = builder.inst_results(call)[0];
                    jump_block(&mut *builder, merge_block, &[slow_res]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                    builder.block_params(merge_block)[0]
                }
            } else {
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
                .expect("Value not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_neg",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*val]);
                builder.inst_results(call)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "pos" | "unary_pos" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_float_lane(op) {
                let src_f = float_value_for(&mut *builder, vars, float_primary_vars, &args[0])
                    .unwrap_or_else(|| {
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
                        .expect("Value not found");
                        builder.ins().bitcast(types::F64, MemFlags::new(), *val)
                    });
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| float_primary_vars.contains(o))
                {
                    src_f
                } else {
                    box_float_value(&mut *builder, src_f, nbc)
                }
            } else if op_prefers_int_lane(op) {
                let src_name = &args[0];
                if let Some(src_raw) =
                    int_raw_value(&mut *builder, vars, int_primary_vars, src_name)
                {
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, src_raw);
                    }
                    return OpFlow::Continue;
                }
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
                .expect("Value not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_pos",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*val]);
                builder.inst_results(call)[0]
            } else {
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
                .expect("Value not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_pos",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*val]);
                builder.inst_results(call)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "abs" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                // abs(x): select(x < 0, -x, x). Overflow deferred.
                let src_name = &args[0];
                let src_raw = int_raw_value(&mut *builder, vars, int_primary_vars, src_name);

                if let Some(src_raw) = src_raw {
                    // Raw i64 primary abs: branchless select.
                    let zero = builder.ins().iconst(types::I64, 0);
                    let is_neg = builder.ins().icmp(IntCC::SignedLessThan, src_raw, zero);
                    let negated = builder.ins().isub(zero, src_raw);
                    let abs_val = builder.ins().select(is_neg, negated, src_raw);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, abs_val);
                    }
                    return OpFlow::Continue;
                } else {
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
                    .expect("Value not found");
                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_abs_builtin",
                        &[types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);

                    // op_prefers_int_lane only proves Python-`int` type,
                    // which includes heap BigInts (TAG_PTR). Guard the raw
                    // abs on a runtime inline-int tag check so a BigInt
                    // operand routes to the runtime helper instead of being
                    // truncated by the trusted unbox.
                    let (val_xored, int_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *val, nbc);
                    let is_int = fused_both_int_check(&mut *builder, val_xored, val_xored, nbc);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let is_neg = builder.ins().icmp(IntCC::SignedLessThan, int_val, zero);
                    let negated = builder.ins().isub(zero, int_val);
                    let abs_val = builder.ins().select(is_neg, negated, int_val);
                    let fits_inline = int_value_fits_inline(&mut *builder, abs_val);
                    let take_fast = builder.ins().band(is_int, fits_inline);
                    builder
                        .ins()
                        .brif(take_fast, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    let fast_res = box_int_value(&mut *builder, abs_val, nbc);
                    jump_block(&mut *builder, merge_block, &[fast_res]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let call = builder.ins().call(local_callee, &[*val]);
                    let slow_res = builder.inst_results(call)[0];
                    jump_block(&mut *builder, merge_block, &[slow_res]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                    builder.block_params(merge_block)[0]
                }
            } else {
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
                .expect("Value not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_abs_builtin",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*val]);
                builder.inst_results(call)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "invert" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                // ~x == x ^ -1 for integers; magnitude changes by at most 1.
                let src_name = &args[0];
                let src_raw = int_raw_value(&mut *builder, vars, int_primary_vars, src_name);

                if let Some(src_raw) = src_raw {
                    // Raw i64 primary invert: branchless, no overflow.
                    let minus_one = builder.ins().iconst(types::I64, -1i64);
                    let inverted = builder.ins().bxor(src_raw, minus_one);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, inverted);
                    }
                    return OpFlow::Continue;
                } else {
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
                    .expect("Value not found");
                    // op_prefers_int_lane only proves Python-`int` type,
                    // which includes heap BigInts (TAG_PTR). Guard the raw
                    // bitwise-not on a runtime inline-int tag check so a
                    // BigInt operand routes to `molt_invert` instead of
                    // being truncated by the trusted unbox. `~x` of an
                    // inline int never overflows the inline range, so no
                    // fits-inline check is needed on the fast path.
                    let invert_callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_invert",
                        &[types::I64],
                        &[types::I64],
                    );
                    let invert_local_callee =
                        module.declare_func_in_func(invert_callee, builder.func);
                    let (val_xored, int_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *val, nbc);
                    let is_int = fused_both_int_check(&mut *builder, val_xored, val_xored, nbc);
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);
                    builder.ins().brif(is_int, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    let minus_one = builder.ins().iconst(types::I64, -1i64);
                    let inverted = builder.ins().bxor(int_val, minus_one);
                    let fast_res = box_int_value(&mut *builder, inverted, nbc);
                    jump_block(&mut *builder, merge_block, &[fast_res]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let call = builder.ins().call(invert_local_callee, &[*val]);
                    let slow_res = builder.inst_results(call)[0];
                    jump_block(&mut *builder, merge_block, &[slow_res]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                    builder.block_params(merge_block)[0]
                }
            } else {
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
                .expect("Value not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_invert",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*val]);
                builder.inst_results(call)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "bool" | "cast_bool" | "builtin_bool" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let (res, bool_raw) = if let Some(raw_val) =
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[0])
            {
                // Raw bool from proven list_bool getitem or const_bool.
                // bool(x) where x is already raw 0/1 — just re-box.
                // Propagate the raw shadow directly (already 0/1).
                let is_nonzero = builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0);
                (
                    box_bool_value(&mut *builder, is_nonzero, nbc),
                    Some(raw_val),
                )
            } else if let Some(raw_shadow) =
                int_raw_value(&mut *builder, vars, int_primary_vars, &args[0])
            {
                // Proven raw i64 carrier: bool(x) is `x != 0`.
                let zero = builder.ins().iconst(types::I64, 0);
                let is_nonzero = builder.ins().icmp(IntCC::NotEqual, raw_shadow, zero);
                let raw_bool = builder.ins().uextend(types::I64, is_nonzero);
                (
                    box_bool_value(&mut *builder, is_nonzero, nbc),
                    Some(raw_bool),
                )
            } else if op_prefers_int_lane(op) {
                // op_prefers_int_lane only proves Python-`int` type, which
                // includes heap BigInts (TAG_PTR). The trusted unbox would
                // truncate a BigInt pointer (e.g. `bool(1 << 47)` has low 47
                // bits zero and would be wrongly False). Guard on a runtime
                // inline-int tag check: inline TAG_INT/TAG_BOOL use
                // `unbox != 0`; any heap int (BigInt) is non-zero by
                // construction, hence always truthy.
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
                .expect("Value not found");
                let int_val = unbox_int_or_bool(&mut *builder, *val, nbc);
                let is_inline_int = fused_is_int_or_bool(&mut *builder, *val, nbc);
                let zero = builder.ins().iconst(types::I64, 0);
                let inline_nonzero = builder.ins().icmp(IntCC::NotEqual, int_val, zero);
                let true_val = builder.ins().iconst(types::I8, 1);
                let is_nonzero = builder
                    .ins()
                    .select(is_inline_int, inline_nonzero, true_val);
                let raw_bool = builder.ins().uextend(types::I64, is_nonzero);
                (
                    box_bool_value(&mut *builder, is_nonzero, nbc),
                    Some(raw_bool),
                )
            } else if op_prefers_bool_lane(op) {
                // For known bools, extract bit 0 directly — no function call.
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
                .expect("Value not found");
                let one = builder.ins().iconst(types::I64, 1);
                let bit0 = builder.ins().band(*val, one);
                let is_nonzero = builder.ins().icmp_imm(IntCC::NotEqual, bit0, 0);
                (box_bool_value(&mut *builder, is_nonzero, nbc), Some(bit0))
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_is_truthy",
                    &[types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
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
                .expect("Value not found");
                let call = builder.ins().call(local_callee, &[*val]);
                let truthy = builder.inst_results(call)[0];
                let cond = builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0);
                (box_bool_value(&mut *builder, cond, nbc), Some(truthy))
            };
            if let Some(ref out__) = op.out {
                def_bool_result(&mut *builder, vars, bool_primary_vars, out__, res, bool_raw);
            }
        }
        "and" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Raw-bool fast lane: a Python value-select over two raw
            // 0/1 bools IS the boolean AND — a bare `band`, no boxing,
            // no refcount, no truthiness call. This is the lane the
            // overflow_peel break-condition chain rides
            // (`And(cond, Not(of))` every iteration).
            if let (Some(lhs_raw), Some(rhs_raw), Some(out__)) = (
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[0]),
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[1]),
                op.out.as_deref(),
            ) {
                let res = builder.ins().band(lhs_raw, rhs_raw);
                def_raw_bool_value(&mut *builder, vars, bool_primary_vars, out__, res, nbc);
                return OpFlow::Continue;
            }
            let lhs = var_get_boxed_overflow_safe(
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
            .expect("LHS not found");
            let rhs = var_get_boxed_overflow_safe(
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
            .expect("RHS not found");
            let cond = if let Some(raw_val) =
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[0])
            {
                // Raw bool from proven list_bool getitem or const_bool.
                builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0)
            } else if op_prefers_int_lane(op) {
                // Known int: inline unbox + compare, no function call.
                let raw_val = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0])
                    .unwrap_or_else(|| unbox_int(&mut *builder, *lhs, nbc));
                builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0)
            } else if op_prefers_bool_lane(op) {
                // Known bool: extract bit 0.
                let one = builder.ins().iconst(types::I64, 1);
                let bit0 = builder.ins().band(*lhs, one);
                builder.ins().icmp_imm(IntCC::NotEqual, bit0, 0)
            } else {
                // Unknown type: GIL-wrapped truthy check.
                let truthy = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_is_truthy",
                    &[types::I64],
                    &[types::I64],
                );
                let truthy_ref = module.declare_func_in_func(truthy, builder.func);
                let lhs_call = builder.ins().call(truthy_ref, &[*lhs]);
                let lhs_val = builder.inst_results(lhs_call)[0];
                builder.ins().icmp_imm(IntCC::NotEqual, lhs_val, 0)
            };
            let res = builder.ins().select(cond, *rhs, *lhs);
            if let Some(out__) = op.out.as_deref() {
                debug_assert!(
                    crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table(
                        "and"
                    )
                );
                // The `select` result aliases one of the inputs (same
                // NaN-boxed bits).  The generated result-ownership table
                // classifies boxed `and` as minting a new owned selected
                // operand, so retain exactly when an output name is bound.
                emit_inc_ref_obj(&mut *builder, res, local_inc_ref_obj, nbc);
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "or" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Raw-bool fast lane: value-select over two raw 0/1
            // bools IS the boolean OR — a bare `bor` (see the
            // matching `and` lane above; this is the overflow_peel
            // flag fan-in `Or(f1, f2)` every iteration).
            if let (Some(lhs_raw), Some(rhs_raw), Some(out__)) = (
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[0]),
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[1]),
                op.out.as_deref(),
            ) {
                let res = builder.ins().bor(lhs_raw, rhs_raw);
                def_raw_bool_value(&mut *builder, vars, bool_primary_vars, out__, res, nbc);
                return OpFlow::Continue;
            }
            let lhs = var_get_boxed_overflow_safe(
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
            .expect("LHS not found");
            let rhs = var_get_boxed_overflow_safe(
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
            .expect("RHS not found");
            let cond = if let Some(raw_val) =
                bool_raw_value(&mut *builder, vars, bool_primary_vars, &args[0])
            {
                // Raw bool from proven list_bool getitem or const_bool.
                builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0)
            } else if op_prefers_int_lane(op) {
                // Known int: inline unbox + compare, no function call.
                let raw_val = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0])
                    .unwrap_or_else(|| unbox_int(&mut *builder, *lhs, nbc));
                builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0)
            } else if op_prefers_bool_lane(op) {
                // Known bool: extract bit 0.
                let one = builder.ins().iconst(types::I64, 1);
                let bit0 = builder.ins().band(*lhs, one);
                builder.ins().icmp_imm(IntCC::NotEqual, bit0, 0)
            } else {
                // Unknown type: GIL-wrapped truthy check.
                let truthy = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_is_truthy",
                    &[types::I64],
                    &[types::I64],
                );
                let truthy_ref = module.declare_func_in_func(truthy, builder.func);
                let lhs_call = builder.ins().call(truthy_ref, &[*lhs]);
                let lhs_val = builder.inst_results(lhs_call)[0];
                builder.ins().icmp_imm(IntCC::NotEqual, lhs_val, 0)
            };
            let res = builder.ins().select(cond, *lhs, *rhs);
            if let Some(out__) = op.out.as_deref() {
                debug_assert!(
                    crate::tir::op_kinds_generated::kind_result_mints_owned_selected_operand_table(
                        "or"
                    )
                );
                // Same selected-alias ownership contract as `and`.
                emit_inc_ref_obj(&mut *builder, res, local_inc_ref_obj, nbc);
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "contains" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let container = var_get_boxed_overflow_safe(
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
            .expect("Container not found");
            let item = var_get_boxed_overflow_safe(
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
            .expect("Item not found");
            let func_name = match representation_plan.name_container_kind(&args[0]) {
                Some(ContainerKind::Set) => "molt_set_contains",
                Some(ContainerKind::Dict) => "molt_dict_contains",
                Some(ContainerKind::List) => "molt_list_contains",
                Some(ContainerKind::Str) => "molt_str_contains",
                _ => "molt_contains",
            };
            let mut sig = module.make_signature();
            sig.params.push(AbiParam::new(types::I64));
            sig.params.push(AbiParam::new(types::I64));
            sig.returns.push(AbiParam::new(types::I64));
            let callee = module
                .declare_function(func_name, Linkage::Import, &sig)
                .unwrap();
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*container, *item]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("non-unary/logic op routed to handle_unary_logic_op"),
    }
    OpFlow::Proceed
}
