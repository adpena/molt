use super::super::*;
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Division, modulo, power, and numeric rounding codegen authority.
///
/// These ops share the quotient/remainder runtime-symbol split, raw-primary
/// int lanes, float division zero checks, and pow/round/trunc boxed fallbacks.
/// Keeping them in their own handler gives rustc a smaller codegen unit while
/// keeping one handled-kind authority for the full quotient/power cluster.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "div",
    "inplace_div",
    "floordiv",
    "inplace_floordiv",
    "mod",
    "inplace_mod",
    "floor_div",
    "binop_floor_div",
    "pow",
    "inplace_pow",
    "pow_mod",
    "round",
    "trunc",
];

#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_arith_division_op(
    op: &OpIR,
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
    let op_prefers_int_lane =
        |op: &OpIR| super::op_prefers_int_lane(scalar_fast_paths_enabled, representation_plan, op);
    let op_prefers_integer_runtime_lane = |op: &OpIR| {
        scalar_fast_paths_enabled && representation_plan.op_prefers_integer_runtime_lane(op)
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
        "div" | "inplace_div" => {
            // `/` and `/=`.  Int/float fast lanes are identical (builtin
            // numerics have no __itruediv__); only the boxed fallback
            // symbol changes — molt_inplace_div tries __itruediv__ before
            // the binary __truediv__/__rtruediv__ chain.
            let boxed_sym = if op.kind == "inplace_div" {
                "molt_inplace_div"
            } else {
                "molt_div"
            };
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Defer var_get per-branch: float path defers into cold
            // paths so Cranelift DCE can eliminate NaN-boxing.
            let res = if op_prefers_float_lane(op) {
                // Both operands known to be f64.  CPython raises
                // ZeroDivisionError for float division by zero, so
                // we must check before using fdiv (which produces
                // IEEE infinity/NaN instead of an exception).
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let out_is_float_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|o| representation_plan.is_float_unboxed(o));
                let lhs_f = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    representation_plan,
                    nbc,
                    lhs_name,
                );
                let rhs_f = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    representation_plan,
                    nbc,
                    rhs_name,
                );
                let zero_f = builder.ins().f64const(0.0);
                let is_zero = builder.ins().fcmp(FloatCC::Equal, rhs_f, zero_f);
                let ok_block = builder.create_block();
                let zero_block = builder.create_block();
                builder.set_cold_block(zero_block);
                let merge_block = builder.create_block();
                if !out_is_float_primary {
                    builder.append_block_param(merge_block, types::I64);
                } else {
                    builder.append_block_param(merge_block, types::F64);
                }
                builder.ins().brif(is_zero, zero_block, &[], ok_block, &[]);
                // Zero divisor -> call runtime for ZeroDivisionError.
                // Defer var_get to cold path -- only needed for runtime call.
                switch_to_block_materialized(&mut *builder, zero_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, zero_block);
                let lhs_boxed = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
                )
                .expect("LHS not found");
                let rhs_boxed = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[1],
                    representation_plan,
                )
                .expect("RHS not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs_boxed, *rhs_boxed]);
                let slow_res = builder.inst_results(call)[0];
                let slow_f = builder
                    .ins()
                    .bitcast(types::F64, MemFlagsData::new(), slow_res);
                if out_is_float_primary {
                    jump_block(&mut *builder, merge_block, &[slow_f]);
                } else {
                    jump_block(&mut *builder, merge_block, &[slow_res]);
                }
                // Non-zero -> fast fdiv.
                switch_to_block_materialized(&mut *builder, ok_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, ok_block);
                let result_f = builder.ins().fdiv(lhs_f, rhs_f);
                if out_is_float_primary {
                    jump_block(&mut *builder, merge_block, &[result_f]);
                } else {
                    let fast_res = box_float_value(&mut *builder, result_f, nbc);
                    jump_block(&mut *builder, merge_block, &[fast_res]);
                }
                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            } else if op_prefers_int_lane(op) {
                // Python true division: int / int always returns float.
                // Convert to f64 and do fdiv.
                let div_out_is_float_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|o| representation_plan.is_float_unboxed(o));
                let lhs = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
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
                    representation_plan,
                )
                .expect("RHS not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                if !div_out_is_float_primary {
                    builder.append_block_param(merge_block, types::I64);
                } else {
                    builder.append_block_param(merge_block, types::F64);
                }

                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                // Check for zero divisor using the NaN-boxed representation.
                // box_int(0) = QNAN | TAG_INT = 0x7ff9000000000000.
                let boxed_zero = builder.ins().iconst(types::I64, box_int(0));
                let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, *rhs, boxed_zero);
                let take_div = builder.ins().band(both_int, rhs_nonzero);
                builder
                    .ins()
                    .brif(take_div, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                // Python true division: int / int -> float.
                let lhs_f = builder.ins().fcvt_from_sint(types::F64, lhs_val);
                let rhs_f = builder.ins().fcvt_from_sint(types::F64, rhs_val);
                let result_f = builder.ins().fdiv(lhs_f, rhs_f);
                if div_out_is_float_primary {
                    jump_block(&mut *builder, merge_block, &[result_f]);
                } else {
                    let fast_res = box_float_value(&mut *builder, result_f, nbc);
                    jump_block(&mut *builder, merge_block, &[fast_res]);
                }

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                let slow_f = builder
                    .ins()
                    .bitcast(types::F64, MemFlagsData::new(), slow_res);
                if div_out_is_float_primary {
                    jump_block(&mut *builder, merge_block, &[slow_f]);
                } else {
                    jump_block(&mut *builder, merge_block, &[slow_res]);
                }

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            } else {
                let gen_div_out_fp = op
                    .out
                    .as_ref()
                    .is_some_and(|o| representation_plan.is_float_unboxed(o));
                let lhs = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
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
                    representation_plan,
                )
                .expect("RHS not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let int_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                if !gen_div_out_fp {
                    builder.append_block_param(merge_block, types::I64);
                } else {
                    builder.append_block_param(merge_block, types::F64);
                }
                builder
                    .ins()
                    .brif(both_int, int_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, int_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, int_block);
                let zero = builder.ins().iconst(types::I64, 0);
                let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                let fast_block = builder.create_block();
                builder
                    .ins()
                    .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                // Python true division: int / int -> float.
                let lhs_f = builder.ins().fcvt_from_sint(types::F64, lhs_val);
                let rhs_f = builder.ins().fcvt_from_sint(types::F64, rhs_val);
                let result_f = builder.ins().fdiv(lhs_f, rhs_f);
                if gen_div_out_fp {
                    jump_block(&mut *builder, merge_block, &[result_f]);
                } else {
                    let fast_res = box_float_value(&mut *builder, result_f, nbc);
                    jump_block(&mut *builder, merge_block, &[fast_res]);
                }

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                // Inline float fast path: if both operands are floats, do f64 div directly.
                let both_flt = both_float_check(&mut *builder, *lhs, *rhs, nbc);
                let float_block = builder.create_block();
                let call_block = builder.create_block();
                builder.set_cold_block(call_block);
                builder
                    .ins()
                    .brif(both_flt, float_block, &[], call_block, &[]);

                switch_to_block_materialized(&mut *builder, float_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, float_block);
                let lhs_ff = builder.ins().bitcast(types::F64, MemFlagsData::new(), *lhs);
                let rhs_ff = builder.ins().bitcast(types::F64, MemFlagsData::new(), *rhs);
                let flt_quot = builder.ins().fdiv(lhs_ff, rhs_ff);
                if gen_div_out_fp {
                    jump_block(&mut *builder, merge_block, &[flt_quot]);
                } else {
                    let flt_res = box_float_value(&mut *builder, flt_quot, nbc);
                    jump_block(&mut *builder, merge_block, &[flt_res]);
                }

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                let slow_f = builder
                    .ins()
                    .bitcast(types::F64, MemFlagsData::new(), slow_res);
                if gen_div_out_fp {
                    jump_block(&mut *builder, merge_block, &[slow_f]);
                } else {
                    jump_block(&mut *builder, merge_block, &[slow_res]);
                }

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "floordiv" | "inplace_floordiv" => {
            // `//` and `//=`.  The raw-i64 / float fast lanes are byte-
            // identical (builtin int/float have no __ifloordiv__); only
            // the boxed fallback symbol changes — molt_inplace_floordiv
            // tries __ifloordiv__ before the binary __floordiv__/
            // __rfloordiv__ chain.
            let boxed_sym = if op.kind == "inplace_floordiv" {
                "molt_inplace_floordiv"
            } else {
                "molt_floordiv"
            };
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                // Both-shadow raw-primary path: compute floordiv on raw
                // i64 values directly, store raw as primary.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_raw = int_raw_value(&mut *builder, vars, representation_plan, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, representation_plan, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| representation_plan.is_raw_int_carrier_name(out));

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);

                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                    // Raw i64 primary: compute floordiv directly.
                    // Division by zero falls to runtime.
                    let zero = builder.ins().iconst(types::I64, 0);
                    let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_raw, zero);
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64); // raw result
                    builder
                        .ins()
                        .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    let one = builder.ins().iconst(types::I64, 1);
                    let quot = builder.ins().sdiv(lhs_raw, rhs_raw);
                    let rem = builder.ins().srem(lhs_raw, rhs_raw);
                    let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                    let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_raw, zero);
                    let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_raw, zero);
                    let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                    let adjust = builder.ins().band(rem_nonzero, sign_diff);
                    let quot_minus_one = builder.ins().isub(quot, one);
                    let floor_quot = builder.ins().select(adjust, quot_minus_one, quot);
                    jump_block(&mut *builder, merge_block, &[floor_quot]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let lhs_boxed = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("LHS not found");
                    let rhs_boxed = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[1],
                        representation_plan,
                    )
                    .expect("RHS not found");
                    let call = builder.ins().call(local_callee, &[*lhs_boxed, *rhs_boxed]);
                    let slow_res = builder.inst_results(call)[0];
                    // Runtime returns boxed — unbox for raw-primary storage.
                    let slow_raw = unbox_int(&mut *builder, slow_res, nbc);
                    jump_block(&mut *builder, merge_block, &[slow_raw]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                    let result = builder.block_params(merge_block)[0];
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, result);
                    }
                    return OpFlow::Continue;
                } else {
                    let lhs = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
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
                        representation_plan,
                    )
                    .expect("RHS not found");
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);

                    let (lhs_xored, lhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                    let (rhs_xored, rhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                    let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let one = builder.ins().iconst(types::I64, 1);
                    let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                    let take_div = builder.ins().band(both_int, rhs_nonzero);
                    builder
                        .ins()
                        .brif(take_div, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    let quot = builder.ins().sdiv(lhs_val, rhs_val);
                    let rem = builder.ins().srem(lhs_val, rhs_val);
                    let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                    let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                    let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                    let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                    let adjust = builder.ins().band(rem_nonzero, sign_diff);
                    let quot_minus_one = builder.ins().isub(quot, one);
                    let floor_quot = builder.ins().select(adjust, quot_minus_one, quot);
                    let fast_res = box_int_value(&mut *builder, floor_quot, nbc);
                    let fits_inline = int_value_fits_inline(&mut *builder, floor_quot);
                    brif_block(
                        &mut *builder,
                        fits_inline,
                        merge_block,
                        &[fast_res],
                        slow_block,
                        &[],
                    );

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let slow_res = builder.inst_results(call)[0];
                    jump_block(&mut *builder, merge_block, &[slow_res]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                    builder.block_params(merge_block)[0]
                }
            } else {
                let lhs = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
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
                    representation_plan,
                )
                .expect("RHS not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let int_block = builder.create_block();
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                builder
                    .ins()
                    .brif(both_int, int_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, int_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, int_block);
                let zero = builder.ins().iconst(types::I64, 0);
                let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                builder
                    .ins()
                    .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let one = builder.ins().iconst(types::I64, 1);
                // SAFETY: Cranelift sdiv traps on INT_MIN/-1 (unlike x86 SIGFPE).
                // NaN-boxed ints are 47-bit (range [-(2^46), 2^46-1]), so INT64_MIN
                // cannot occur from unbox_int. If this invariant changes, add a guard:
                // rhs != -1 || lhs != INT64_MIN.
                let quot = builder.ins().sdiv(lhs_val, rhs_val);
                let rem = builder.ins().srem(lhs_val, rhs_val);
                let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                let adjust = builder.ins().band(rem_nonzero, sign_diff);
                let quot_minus_one = builder.ins().isub(quot, one);
                let floor_quot = builder.ins().select(adjust, quot_minus_one, quot);
                let fast_res = box_int_value(&mut *builder, floor_quot, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, floor_quot);
                brif_block(
                    &mut *builder,
                    fits_inline,
                    merge_block,
                    &[fast_res],
                    slow_block,
                    &[],
                );

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "mod" | "inplace_mod" => {
            // `%` and `%=`.  Int/float fast lanes byte-identical (builtin
            // numerics have no __imod__); boxed fallback symbol changes —
            // molt_inplace_mod tries __imod__ before the binary
            // __mod__/__rmod__ chain.
            let boxed_sym = if op.kind == "inplace_mod" {
                "molt_inplace_mod"
            } else {
                "molt_mod"
            };
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                // Both-shadow raw-primary path.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_raw = int_raw_value(&mut *builder, vars, representation_plan, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, representation_plan, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| representation_plan.is_raw_int_carrier_name(out));

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);

                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                    // Raw i64 primary: compute Python mod directly.
                    let zero = builder.ins().iconst(types::I64, 0);
                    let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_raw, zero);
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);
                    builder
                        .ins()
                        .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    let rem = builder.ins().srem(lhs_raw, rhs_raw);
                    let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                    let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_raw, zero);
                    let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_raw, zero);
                    let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                    let adjust = builder.ins().band(rem_nonzero, sign_diff);
                    let rem_adjusted = builder.ins().iadd(rem, rhs_raw);
                    let mod_val = builder.ins().select(adjust, rem_adjusted, rem);
                    jump_block(&mut *builder, merge_block, &[mod_val]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let lhs_boxed = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("LHS not found");
                    let rhs_boxed = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[1],
                        representation_plan,
                    )
                    .expect("RHS not found");
                    let call = builder.ins().call(local_callee, &[*lhs_boxed, *rhs_boxed]);
                    let slow_res = builder.inst_results(call)[0];
                    let slow_raw = unbox_int(&mut *builder, slow_res, nbc);
                    jump_block(&mut *builder, merge_block, &[slow_raw]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                    let result = builder.block_params(merge_block)[0];
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, result);
                    }
                    return OpFlow::Continue;
                } else {
                    let lhs = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
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
                        representation_plan,
                    )
                    .expect("RHS not found");
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);

                    let (lhs_xored, lhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                    let (rhs_xored, rhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                    let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                    let zero = builder.ins().iconst(types::I64, 0);
                    let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                    let take_div = builder.ins().band(both_int, rhs_nonzero);
                    builder
                        .ins()
                        .brif(take_div, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    let rem = builder.ins().srem(lhs_val, rhs_val);
                    let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                    let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                    let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                    let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                    let adjust = builder.ins().band(rem_nonzero, sign_diff);
                    let rem_adjusted = builder.ins().iadd(rem, rhs_val);
                    let mod_val = builder.ins().select(adjust, rem_adjusted, rem);
                    let fast_res = box_int_value(&mut *builder, mod_val, nbc);
                    let fits_inline = int_value_fits_inline(&mut *builder, mod_val);
                    brif_block(
                        &mut *builder,
                        fits_inline,
                        merge_block,
                        &[fast_res],
                        slow_block,
                        &[],
                    );

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let slow_res = builder.inst_results(call)[0];
                    jump_block(&mut *builder, merge_block, &[slow_res]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                    builder.block_params(merge_block)[0]
                }
            } else {
                let lhs = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
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
                    representation_plan,
                )
                .expect("RHS not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let int_block = builder.create_block();
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                builder
                    .ins()
                    .brif(both_int, int_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, int_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, int_block);
                let zero = builder.ins().iconst(types::I64, 0);
                let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                builder
                    .ins()
                    .brif(rhs_nonzero, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let rem = builder.ins().srem(lhs_val, rhs_val);
                let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                let adjust = builder.ins().band(rem_nonzero, sign_diff);
                let rem_adjusted = builder.ins().iadd(rem, rhs_val);
                let mod_val = builder.ins().select(adjust, rem_adjusted, rem);
                let fast_res = box_int_value(&mut *builder, mod_val, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, mod_val);
                brif_block(
                    &mut *builder,
                    fits_inline,
                    merge_block,
                    &[fast_res],
                    slow_block,
                    &[],
                );

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "floor_div" | "binop_floor_div" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let lhs = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
            )
            .expect("RHS not found");
            let res = if op_prefers_int_lane(op) {
                // Python floor_div: divide and floor towards negative infinity.
                // sdiv truncates towards zero; we adjust when signs differ and
                // there is a remainder.
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_floordiv",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);

                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let zero = builder.ins().iconst(types::I64, 0);
                let rhs_nonzero = builder.ins().icmp(IntCC::NotEqual, rhs_val, zero);
                let take_div = builder.ins().band(both_int, rhs_nonzero);
                builder
                    .ins()
                    .brif(take_div, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let quot = builder.ins().sdiv(lhs_val, rhs_val);
                let rem = builder.ins().srem(lhs_val, rhs_val);
                // Adjust: if rem != 0 and signs of lhs/rhs differ, subtract 1.
                let rem_nonzero = builder.ins().icmp(IntCC::NotEqual, rem, zero);
                let lhs_neg = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, zero);
                let rhs_neg = builder.ins().icmp(IntCC::SignedLessThan, rhs_val, zero);
                let sign_diff = builder.ins().bxor(lhs_neg, rhs_neg);
                let adjust = builder.ins().band(rem_nonzero, sign_diff);
                let one = builder.ins().iconst(types::I64, 1);
                let quot_adjusted = builder.ins().isub(quot, one);
                let floor_val = builder.ins().select(adjust, quot_adjusted, quot);
                let fast_res = box_int_value(&mut *builder, floor_val, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, floor_val);
                brif_block(
                    &mut *builder,
                    fits_inline,
                    merge_block,
                    &[fast_res],
                    slow_block,
                    &[],
                );

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_floordiv",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                builder.inst_results(call)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "pow" | "inplace_pow" => {
            // `**` and `**=`.  Int/float fast lanes byte-identical
            // (builtin numerics have no __ipow__); boxed fallback symbol
            // changes — molt_inplace_pow tries __ipow__ before the binary
            // __pow__/__rpow__ chain.
            let boxed_sym = if op.kind == "inplace_pow" {
                "molt_inplace_pow"
            } else {
                "molt_pow"
            };
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_raw = int_raw_value(&mut *builder, vars, representation_plan, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, representation_plan, rhs_name);

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);

                if let (Some(base_raw), Some(exp_raw)) = (lhs_raw, rhs_raw) {
                    // Raw i64 primary pow: inline for exp 0, 1, 2.
                    let exp0_block = builder.create_block();
                    let exp1_block = builder.create_block();
                    let exp2_block = builder.create_block();
                    let exp2_fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64);

                    let is_zero = builder.ins().icmp_imm(IntCC::Equal, exp_raw, 0);
                    builder
                        .ins()
                        .brif(is_zero, exp0_block, &[], exp1_block, &[]);

                    switch_to_block_materialized(&mut *builder, exp0_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, exp0_block);
                    let one = builder.ins().iconst(types::I64, 1);
                    jump_block(&mut *builder, merge_block, &[one]);

                    switch_to_block_materialized(&mut *builder, exp1_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, exp1_block);
                    let is_one = builder.ins().icmp_imm(IntCC::Equal, exp_raw, 1);
                    let exp1_ret_block = builder.create_block();
                    builder
                        .ins()
                        .brif(is_one, exp1_ret_block, &[], exp2_block, &[]);

                    switch_to_block_materialized(&mut *builder, exp1_ret_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, exp1_ret_block);
                    jump_block(&mut *builder, merge_block, &[base_raw]);

                    switch_to_block_materialized(&mut *builder, exp2_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, exp2_block);
                    let is_two = builder.ins().icmp_imm(IntCC::Equal, exp_raw, 2);
                    builder
                        .ins()
                        .brif(is_two, exp2_fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, exp2_fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, exp2_fast_block);
                    let sq = builder.ins().imul(base_raw, base_raw);
                    jump_block(&mut *builder, merge_block, &[sq]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let lhs_boxed = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("LHS not found");
                    let rhs_boxed = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        vars,
                        &args[1],
                        representation_plan,
                    )
                    .expect("RHS not found");
                    let call = builder.ins().call(local_callee, &[*lhs_boxed, *rhs_boxed]);
                    let slow_res = builder.inst_results(call)[0];
                    let slow_raw = unbox_int(&mut *builder, slow_res, nbc);
                    jump_block(&mut *builder, merge_block, &[slow_raw]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                    let result = builder.block_params(merge_block)[0];
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, result);
                    }
                    return OpFlow::Continue;
                }
                // No both-shadow: proven-int path with boxing.
                let lhs = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
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
                    representation_plan,
                )
                .expect("RHS not found");
                // Inline pow for small non-negative exponents (0, 1, 2).
                // Exponent >= 3 or negative falls back to runtime.
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);

                let exp0_block = builder.create_block();
                let exp1_block = builder.create_block();
                let exp2_block = builder.create_block();
                let exp2_fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);

                // op_prefers_int_lane proves both operands are Python
                // `int`-typed, which includes heap BigInts (TAG_PTR). The
                // inline exp==0/1/2 fast path shift-unboxes base/exp and is
                // only value-exact for inline TAG_INT; a BigInt operand
                // would be truncated. Guard the inline path on a runtime
                // inline-int tag check and route BigInt / float / mixed
                // operands to the boxed `molt_pow` slow path.
                let (lhs_xored, base_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, exp_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let pow_inline_block = builder.create_block();
                builder
                    .ins()
                    .brif(both_int, pow_inline_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, pow_inline_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, pow_inline_block);
                // Check exp == 0
                let is_zero = builder.ins().icmp_imm(IntCC::Equal, exp_val, 0);
                builder
                    .ins()
                    .brif(is_zero, exp0_block, &[], exp1_block, &[]);

                // exp == 0 → result is 1
                switch_to_block_materialized(&mut *builder, exp0_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, exp0_block);
                let one = builder.ins().iconst(types::I64, 1);
                let res_one = box_int_value(&mut *builder, one, nbc);
                jump_block(&mut *builder, merge_block, &[res_one]);

                // Check exp == 1 → result is base (return lhs as-is)
                switch_to_block_materialized(&mut *builder, exp1_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, exp1_block);
                let is_one = builder.ins().icmp_imm(IntCC::Equal, exp_val, 1);
                let exp1_ret_block = builder.create_block();
                builder
                    .ins()
                    .brif(is_one, exp1_ret_block, &[], exp2_block, &[]);

                switch_to_block_materialized(&mut *builder, exp1_ret_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, exp1_ret_block);
                jump_block(&mut *builder, merge_block, &[*lhs]);

                // Check exp == 2
                switch_to_block_materialized(&mut *builder, exp2_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, exp2_block);
                let is_two = builder.ins().icmp_imm(IntCC::Equal, exp_val, 2);
                builder
                    .ins()
                    .brif(is_two, exp2_fast_block, &[], slow_block, &[]);

                // exp == 2 → base * base with overflow check
                switch_to_block_materialized(&mut *builder, exp2_fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, exp2_fast_block);
                let (sq, fits) = imul_checked_inline(&mut *builder, base_val, base_val);
                let sq_res = box_int_value(&mut *builder, sq, nbc);
                brif_block(&mut *builder, fits, merge_block, &[sq_res], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            } else {
                let lhs = var_get_boxed_overflow_safe(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    &args[0],
                    representation_plan,
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
                    representation_plan,
                )
                .expect("RHS not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    boxed_sym,
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                builder.inst_results(call)[0]
            };
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "pow_mod" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let lhs = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[0],
                representation_plan,
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
                representation_plan,
            )
            .expect("RHS not found");
            let modulus = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Mod not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_pow_mod",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*lhs, *rhs, *modulus]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "round" => {
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
            .expect("Round arg not found");
            let ndigits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[1],
                representation_plan,
            )
            .expect("Round ndigits not found");
            let has_ndigits = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                &args[2],
                representation_plan,
            )
            .expect("Round ndigits flag not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_round",
                &[types::I64, types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder
                .ins()
                .call(local_callee, &[*val, *ndigits, *has_ndigits]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "trunc" => {
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
            .expect("Trunc arg not found");
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_trunc",
                &[types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*val]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("non-division arithmetic op routed to handle_arith_division_op"),
    }
    OpFlow::Proceed
}
