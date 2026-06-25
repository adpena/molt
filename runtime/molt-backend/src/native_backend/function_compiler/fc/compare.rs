use super::super::*;

/// Single-source kind authority for [`handle_compare_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] =
    &["lt", "le", "gt", "ge", "eq", "ne", "string_eq"];
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for comparison ops: numeric ordering, equality,
/// and string equality. Extracted from `compile_func_inner` as a move-only
/// function split; the arm bodies preserve the original scalar fast paths and
/// runtime fallback structure, with only split-borrow access paths changed.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_compare_op(
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
    float_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    loop_stack: &[LoopFrame],
    scalar_fast_paths_enabled: bool,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) {
    let name_is_numeric_scalar = |name: &str| {
        int_like_vars.contains(name)
            || bool_like_vars.contains(name)
            || float_like_vars.contains(name)
            || int_primary_vars.contains(name)
            || float_primary_vars.contains(name)
    };
    let op_prefers_integer_runtime_lane = |op: &OpIR| {
        scalar_fast_paths_enabled && representation_plan.op_prefers_integer_runtime_lane(op)
    };
    let op_prefers_float_numeric_lane = |op: &OpIR| {
        if !scalar_fast_paths_enabled || op_prefers_integer_runtime_lane(op) {
            return false;
        }
        let Some(args) = op.args.as_ref() else {
            return false;
        };
        let (Some(lhs), Some(rhs)) = (args.first(), args.get(1)) else {
            return false;
        };
        let has_float_operand = float_like_vars.contains(lhs)
            || float_like_vars.contains(rhs)
            || float_primary_vars.contains(lhs)
            || float_primary_vars.contains(rhs);
        has_float_operand && name_is_numeric_scalar(lhs) && name_is_numeric_scalar(rhs)
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
        "lt" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let in_active_loop = !loop_stack.is_empty();
            let lr = if in_active_loop {
                // Variable-backed shadows are phi-correct across loop
                // back-edges; Value-tier may be stale.
                int_raw_value(&mut *builder, vars, int_primary_vars, &args[0])
            } else {
                int_raw_value(&mut *builder, vars, int_primary_vars, &args[0])
            };
            let rr = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
            // Helper: propagate raw bool result from a Cranelift icmp/fcmp
            // result (i8) so downstream loop_break_if_true/false and `if`
            // ops branch directly on the raw value, eliminating NaN-box
            // round-trips (box_bool_value + band+icmp extraction).
            let mut lt_raw_bool: Option<Value> = None;
            let res = if op_prefers_float_numeric_lane(op) {
                let (boxed, raw) = emit_float_numeric_compare(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    bool_primary_vars,
                    nbc,
                    op.out.as_ref(),
                    &args[0],
                    &args[1],
                    FloatCC::LessThan,
                );
                lt_raw_bool = Some(raw);
                boxed
            } else if let (Some(lr), Some(rr)) = (lr, rr) {
                let cmp = builder.ins().icmp(IntCC::SignedLessThan, lr, rr);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                lt_raw_bool = Some(raw_bool);
                result
            } else if scalar_fast_paths_enabled
                && float_like_vars.contains(&args[0])
                && float_like_vars.contains(&args[1])
            {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[0],
                );
                let rf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[1],
                );
                let cmp = builder.ins().fcmp(FloatCC::LessThan, lf, rf);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                lt_raw_bool = Some(raw_bool);
                result
            } else {
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
                let cmp = builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val);
                let fast_res = box_bool_value(&mut *builder, cmp, nbc);
                jump_block(&mut *builder, merge_block, &[fast_res]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_lt",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let both_flt = both_float_check(&mut *builder, *lhs, *rhs, nbc);
                let float_block = builder.create_block();
                let call_block = builder.create_block();
                builder.set_cold_block(call_block);
                builder
                    .ins()
                    .brif(both_flt, float_block, &[], call_block, &[]);

                switch_to_block_materialized(&mut *builder, float_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, float_block);
                let lhs_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), *lhs);
                let rhs_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), *rhs);
                let fcmp = builder.ins().fcmp(FloatCC::LessThan, lhs_f, rhs_f);
                let flt_res = box_bool_value(&mut *builder, fcmp, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_bool_result(
                    &mut *builder,
                    vars,
                    bool_primary_vars,
                    out__,
                    res,
                    lt_raw_bool,
                );
            }
        }
        "le" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let lr = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]);
            let rr = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
            let mut le_raw_bool: Option<Value> = None;
            let res = if op_prefers_float_numeric_lane(op) {
                let (boxed, raw) = emit_float_numeric_compare(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    bool_primary_vars,
                    nbc,
                    op.out.as_ref(),
                    &args[0],
                    &args[1],
                    FloatCC::LessThanOrEqual,
                );
                le_raw_bool = Some(raw);
                boxed
            } else if let (Some(lr), Some(rr)) = (lr, rr) {
                let cmp = builder.ins().icmp(IntCC::SignedLessThanOrEqual, lr, rr);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                le_raw_bool = Some(raw_bool);
                result
            } else if scalar_fast_paths_enabled
                && float_like_vars.contains(&args[0])
                && float_like_vars.contains(&args[1])
            {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[0],
                );
                let rf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[1],
                );
                let cmp = builder.ins().fcmp(FloatCC::LessThanOrEqual, lf, rf);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                le_raw_bool = Some(raw_bool);
                result
            } else {
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
                let cmp = builder
                    .ins()
                    .icmp(IntCC::SignedLessThanOrEqual, lhs_val, rhs_val);
                let fast_res = box_bool_value(&mut *builder, cmp, nbc);
                jump_block(&mut *builder, merge_block, &[fast_res]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_le",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let both_flt = both_float_check(&mut *builder, *lhs, *rhs, nbc);
                let float_block = builder.create_block();
                let call_block = builder.create_block();
                builder.set_cold_block(call_block);
                builder
                    .ins()
                    .brif(both_flt, float_block, &[], call_block, &[]);

                switch_to_block_materialized(&mut *builder, float_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, float_block);
                let lhs_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), *lhs);
                let rhs_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), *rhs);
                let fcmp = builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs_f, rhs_f);
                let flt_res = box_bool_value(&mut *builder, fcmp, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_bool_result(
                    &mut *builder,
                    vars,
                    bool_primary_vars,
                    out__,
                    res,
                    le_raw_bool,
                );
            }
        }
        "gt" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let lhs_shadow = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]);
            let rhs_shadow = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
            let mut gt_raw_bool: Option<Value> = None;
            let res = if op_prefers_float_numeric_lane(op) {
                let (boxed, raw) = emit_float_numeric_compare(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    bool_primary_vars,
                    nbc,
                    op.out.as_ref(),
                    &args[0],
                    &args[1],
                    FloatCC::GreaterThan,
                );
                gt_raw_bool = Some(raw);
                boxed
            } else if let (Some(lr), Some(rr)) = (lhs_shadow, rhs_shadow) {
                let cmp = builder.ins().icmp(IntCC::SignedGreaterThan, lr, rr);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                gt_raw_bool = Some(raw_bool);
                result
            } else if scalar_fast_paths_enabled
                && float_like_vars.contains(&args[0])
                && float_like_vars.contains(&args[1])
            {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[0],
                );
                let rf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[1],
                );
                let cmp = builder.ins().fcmp(FloatCC::GreaterThan, lf, rf);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                gt_raw_bool = Some(raw_bool);
                result
            } else {
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
                let cmp = builder
                    .ins()
                    .icmp(IntCC::SignedGreaterThan, lhs_val, rhs_val);
                let fast_res = box_bool_value(&mut *builder, cmp, nbc);
                jump_block(&mut *builder, merge_block, &[fast_res]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_gt",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let both_flt = both_float_check(&mut *builder, *lhs, *rhs, nbc);
                let float_block = builder.create_block();
                let call_block = builder.create_block();
                builder.set_cold_block(call_block);
                builder
                    .ins()
                    .brif(both_flt, float_block, &[], call_block, &[]);

                switch_to_block_materialized(&mut *builder, float_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, float_block);
                let lhs_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), *lhs);
                let rhs_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), *rhs);
                let fcmp = builder.ins().fcmp(FloatCC::GreaterThan, lhs_f, rhs_f);
                let flt_res = box_bool_value(&mut *builder, fcmp, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_bool_result(
                    &mut *builder,
                    vars,
                    bool_primary_vars,
                    out__,
                    res,
                    gt_raw_bool,
                );
            }
        }
        "ge" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let lhs_shadow = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]);
            let rhs_shadow = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
            let mut ge_raw_bool: Option<Value> = None;
            let res = if op_prefers_float_numeric_lane(op) {
                let (boxed, raw) = emit_float_numeric_compare(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    bool_primary_vars,
                    nbc,
                    op.out.as_ref(),
                    &args[0],
                    &args[1],
                    FloatCC::GreaterThanOrEqual,
                );
                ge_raw_bool = Some(raw);
                boxed
            } else if let (Some(lr), Some(rr)) = (lhs_shadow, rhs_shadow) {
                let cmp = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, lr, rr);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                ge_raw_bool = Some(raw_bool);
                result
            } else if scalar_fast_paths_enabled
                && float_like_vars.contains(&args[0])
                && float_like_vars.contains(&args[1])
            {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[0],
                );
                let rf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[1],
                );
                let cmp = builder.ins().fcmp(FloatCC::GreaterThanOrEqual, lf, rf);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                ge_raw_bool = Some(raw_bool);
                result
            } else {
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
                let cmp = builder
                    .ins()
                    .icmp(IntCC::SignedGreaterThanOrEqual, lhs_val, rhs_val);
                let fast_res = box_bool_value(&mut *builder, cmp, nbc);
                jump_block(&mut *builder, merge_block, &[fast_res]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_ge",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let both_flt = both_float_check(&mut *builder, *lhs, *rhs, nbc);
                let float_block = builder.create_block();
                let call_block = builder.create_block();
                builder.set_cold_block(call_block);
                builder
                    .ins()
                    .brif(both_flt, float_block, &[], call_block, &[]);

                switch_to_block_materialized(&mut *builder, float_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, float_block);
                let lhs_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), *lhs);
                let rhs_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), *rhs);
                let fcmp = builder
                    .ins()
                    .fcmp(FloatCC::GreaterThanOrEqual, lhs_f, rhs_f);
                let flt_res = box_bool_value(&mut *builder, fcmp, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_bool_result(
                    &mut *builder,
                    vars,
                    bool_primary_vars,
                    out__,
                    res,
                    ge_raw_bool,
                );
            }
        }
        "eq" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let eq_lr = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]);
            let eq_rr = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
            let mut eq_raw_bool: Option<Value> = None;
            let res = if op_prefers_float_numeric_lane(op) {
                let (boxed, raw) = emit_float_numeric_compare(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    bool_primary_vars,
                    nbc,
                    op.out.as_ref(),
                    &args[0],
                    &args[1],
                    FloatCC::Equal,
                );
                eq_raw_bool = Some(raw);
                boxed
            } else if let (Some(lr), Some(rr)) = (eq_lr, eq_rr) {
                // Both operands have raw int shadows: direct icmp,
                // no unboxing needed.
                let cmp = builder.ins().icmp(IntCC::Equal, lr, rr);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                eq_raw_bool = Some(raw_bool);
                result
            } else if scalar_fast_paths_enabled
                && float_like_vars.contains(&args[0])
                && float_like_vars.contains(&args[1])
            {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[0],
                );
                let rf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[1],
                );
                let cmp = builder.ins().fcmp(FloatCC::Equal, lf, rf);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                eq_raw_bool = Some(raw_bool);
                result
            } else {
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
                let cmp = builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val);
                let fast_res = box_bool_value(&mut *builder, cmp, nbc);
                jump_block(&mut *builder, merge_block, &[fast_res]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_eq",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_bool_result(
                    &mut *builder,
                    vars,
                    bool_primary_vars,
                    out__,
                    res,
                    eq_raw_bool,
                );
            }
        }
        "ne" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let ne_lr = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]);
            let ne_rr = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
            let mut ne_raw_bool: Option<Value> = None;
            let res = if op_prefers_float_numeric_lane(op) {
                let (boxed, raw) = emit_float_numeric_compare(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    bool_primary_vars,
                    nbc,
                    op.out.as_ref(),
                    &args[0],
                    &args[1],
                    FloatCC::NotEqual,
                );
                ne_raw_bool = Some(raw);
                boxed
            } else if let (Some(lr), Some(rr)) = (ne_lr, ne_rr) {
                // Both operands have raw int shadows: direct icmp.
                let cmp = builder.ins().icmp(IntCC::NotEqual, lr, rr);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                ne_raw_bool = Some(raw_bool);
                result
            } else if scalar_fast_paths_enabled
                && float_like_vars.contains(&args[0])
                && float_like_vars.contains(&args[1])
            {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[0],
                );
                let rf = float_value_for_mixed(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    &mut *sealed_blocks,
                    vars,
                    float_primary_vars,
                    int_primary_vars,
                    int_like_vars,
                    bool_like_vars,
                    nbc,
                    &args[1],
                );
                let cmp = builder.ins().fcmp(FloatCC::NotEqual, lf, rf);
                let (result, raw_bool) = compare_bool_result_value(
                    &mut *builder,
                    bool_primary_vars,
                    op.out.as_ref(),
                    cmp,
                    nbc,
                );
                ne_raw_bool = Some(raw_bool);
                result
            } else {
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
                let cmp = builder.ins().icmp(IntCC::NotEqual, lhs_val, rhs_val);
                let fast_res = box_bool_value(&mut *builder, cmp, nbc);
                jump_block(&mut *builder, merge_block, &[fast_res]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_ne",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_bool_result(
                    &mut *builder,
                    vars,
                    bool_primary_vars,
                    out__,
                    res,
                    ne_raw_bool,
                );
            }
        }
        "string_eq" => {
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
            // Use the fast path: pointer-identity check before byte scan.
            let callee = SimpleBackend::import_func_id_split(
                &mut *module,
                &mut *import_ids,
                "molt_string_eq_fast",
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
        _ => unreachable!("non-compare op routed to handle_compare_op"),
    }
}
