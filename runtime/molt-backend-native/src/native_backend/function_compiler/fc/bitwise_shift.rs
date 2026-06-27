use super::super::*;
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Bitwise and shift kind authority for [`handle_bitwise_shift_op`]. Keep this
/// disjoint from [`super::arith::HANDLED_KINDS`] so scalar arithmetic does not
/// grow back into a monolithic operator bucket.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "bit_or",
    "inplace_bit_or",
    "bit_and",
    "inplace_bit_and",
    "bit_xor",
    "inplace_bit_xor",
    "lshift",
    "shl",
    "inplace_lshift",
    "rshift",
    "shr",
    "inplace_rshift",
];

/// Cranelift codegen handlers for bitwise and shift ops.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_bitwise_shift_op(
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
        "bit_or" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_raw = int_raw_value(&mut *builder, vars, representation_plan, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, representation_plan, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| representation_plan.is_raw_int_carrier_name(out));

                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                    // Bitwise OR on raw i64: branchless, no overflow
                    // possible (|a|b| <= max(|a|,|b|)).
                    let raw = builder.ins().bor(lhs_raw, rhs_raw);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, raw);
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
                    emit_guarded_boxed_bitwise(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        *lhs,
                        *rhs,
                        "molt_bit_or",
                        BoxedBitwiseOp::Or,
                        nbc,
                    )
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
                    "molt_bit_or",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                builder
                    .ins()
                    .brif(both_int, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let raw = builder.ins().bor(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, raw, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, raw);
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
        "inplace_bit_or" => {
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
                emit_guarded_boxed_bitwise(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    *lhs,
                    *rhs,
                    "molt_inplace_bit_or",
                    BoxedBitwiseOp::Or,
                    nbc,
                )
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_inplace_bit_or",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                builder
                    .ins()
                    .brif(both_int, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let raw = builder.ins().bor(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, raw, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, raw);
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
        "bit_and" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_raw = int_raw_value(&mut *builder, vars, representation_plan, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, representation_plan, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| representation_plan.is_raw_int_carrier_name(out));

                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                    // Bitwise AND on raw i64: branchless, no overflow.
                    let raw = builder.ins().band(lhs_raw, rhs_raw);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, raw);
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
                    emit_guarded_boxed_bitwise(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        *lhs,
                        *rhs,
                        "molt_bit_and",
                        BoxedBitwiseOp::And,
                        nbc,
                    )
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
                    "molt_bit_and",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                builder
                    .ins()
                    .brif(both_int, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let raw = builder.ins().band(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, raw, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, raw);
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
        "inplace_bit_and" => {
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
                emit_guarded_boxed_bitwise(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    *lhs,
                    *rhs,
                    "molt_inplace_bit_and",
                    BoxedBitwiseOp::And,
                    nbc,
                )
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_inplace_bit_and",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                builder
                    .ins()
                    .brif(both_int, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let raw = builder.ins().band(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, raw, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, raw);
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
        "bit_xor" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_raw = int_raw_value(&mut *builder, vars, representation_plan, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, representation_plan, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| representation_plan.is_raw_int_carrier_name(out));

                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                    // Bitwise XOR on raw i64: branchless, no overflow.
                    let raw = builder.ins().bxor(lhs_raw, rhs_raw);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, raw);
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
                    emit_guarded_boxed_bitwise(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        &mut *import_refs,
                        &mut *sealed_blocks,
                        *lhs,
                        *rhs,
                        "molt_bit_xor",
                        BoxedBitwiseOp::Xor,
                        nbc,
                    )
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
                    "molt_bit_xor",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                builder
                    .ins()
                    .brif(both_int, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let raw = builder.ins().bxor(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, raw, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, raw);
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
        "inplace_bit_xor" => {
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
                emit_guarded_boxed_bitwise(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    *lhs,
                    *rhs,
                    "molt_inplace_bit_xor",
                    BoxedBitwiseOp::Xor,
                    nbc,
                )
            } else {
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_inplace_bit_xor",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64);
                builder
                    .ins()
                    .brif(both_int, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                let raw = builder.ins().bxor(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, raw, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, raw);
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
        "lshift" | "shl" | "inplace_lshift" => {
            // `<<` and `<<=`.  The inplace variant differs ONLY in the
            // boxed runtime symbol (molt_inplace_lshift tries __ilshift__
            // before the binary __lshift__/__rlshift__ chain); builtin int
            // has no in-place dunder so there is no fast-lane divergence.
            let boxed_sym = if op.kind == "inplace_lshift" {
                "molt_inplace_lshift"
            } else {
                "molt_lshift"
            };
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
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                boxed_sym,
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        "rshift" | "shr" | "inplace_rshift" => {
            // `>>` and `>>=`.  Inplace variant: molt_inplace_rshift tries
            // __irshift__ before the binary chain.
            let boxed_sym = if op.kind == "inplace_rshift" {
                "molt_inplace_rshift"
            } else {
                "molt_rshift"
            };
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
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                boxed_sym,
                &[types::I64, types::I64],
                &[types::I64],
            );
            let local_callee = module.declare_func_in_func(callee, builder.func);
            let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
            let res = builder.inst_results(call)[0];
            if let Some(out__) = op.out.as_ref() {
                def_var_named(&mut *builder, vars, out__, res);
            }
        }
        _ => unreachable!("non-bitwise/shift op routed to handle_bitwise_shift_op"),
    }
    OpFlow::Proceed
}
