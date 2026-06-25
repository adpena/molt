use super::super::*;
use super::OpFlow;
use super::var_get_boxed_overflow_safe_fn;

/// Scalar arithmetic kind authority for [`handle_arith_op`]. The delegated
/// `vec_*` reductions live in [`super::vec_reductions::HANDLED_KINDS`], not
/// here; `op_family::FAMILY_DISPATCH_TABLE` routes both slices to
/// `NativeOpFamily::Arith`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "add",
    "checked_add",
    "checked_mul",
    "inplace_add",
    "sub",
    "inplace_sub",
    "mul",
    "inplace_mul",
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
    "matmul",
    "inplace_matmul",
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

/// Cranelift codegen handlers for arithmetic ops: numeric add/sub/mul,
/// bitwise and shifts, division/modulo, power, rounding, and truncation.
/// Extracted from `compile_func_inner` as a move-only function split; the arm
/// bodies preserve the original scalar fast paths and runtime fallback
/// structure, with only split-borrow access paths and outer-loop flow signals
/// changed.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments, clippy::manual_map)]
pub(in crate::native_backend::function_compiler) fn handle_arith_op(
    op: &OpIR,
    op_idx: usize,
    func_name: &str,
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
    loop_stack: &[LoopFrame],
    scalar_fast_paths_enabled: bool,
    representation_plan: &ScalarRepresentationPlan,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
    let op_prefers_int_lane = |op: &OpIR| {
        super::op_prefers_int_lane(
            scalar_fast_paths_enabled,
            representation_plan,
            op,
            int_like_vars,
            bool_like_vars,
            int_primary_vars,
            bool_primary_vars,
        )
    };
    let op_prefers_integer_runtime_lane = |op: &OpIR| {
        scalar_fast_paths_enabled && representation_plan.op_prefers_integer_runtime_lane(op)
    };
    let op_prefers_float_lane = |op: &OpIR| {
        scalar_fast_paths_enabled
            && !op_prefers_integer_runtime_lane(op)
            && representation_plan.op_scalar_lane(op) == Some(ScalarKind::Float)
    };
    let op_prefers_str_lane = |op: &OpIR| {
        scalar_fast_paths_enabled && representation_plan.op_scalar_lane(op) == Some(ScalarKind::Str)
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
        "add" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Defer var_get: NaN-boxed operands are only read on paths
            // that actually need them.  On the both-shadow fast path the
            // raw i64 values are used directly, so never calling use_var
            // on the NaN-boxed Variable allows Cranelift DCE to eliminate
            // the upstream boxing (band+bor) when all consumers also use
            // the shadow path.
            let res = if op_prefers_str_lane(op) {
                // Both operands known to be strings — direct concat,
                // skips the 8-branch dispatch in molt_add.
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
                    "molt_str_concat",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                builder.inst_results(call)[0]
            } else if op_prefers_float_lane(op) {
                // Both operands known to be f64 — direct float arithmetic.
                // Float-primary operands are read as raw F64; other
                // float-like operands recover F64 by bitcasting their
                // main boxed I64 value.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_f = float_value_for_mixed(
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
                    lhs_name,
                );
                let rhs_f = float_value_for_mixed(
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
                    rhs_name,
                );
                let result_f = builder.ins().fadd(lhs_f, rhs_f);
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| float_primary_vars.contains(o))
                {
                    result_f
                } else {
                    box_float_value(&mut *builder, result_f, nbc)
                }
            } else if op_prefers_int_lane(op) {
                // LuaJIT-style unboxed arithmetic chain with overflow guard.
                // If both operands have raw i64 shadows, skip tag check + unbox.
                // If result overflows 47-bit inline range, fall to slow path.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                // Phase 1b: inside loops, accept either a Variable-tier
                // shadow (phi-correct across back-edges) OR a
                // int_primary_vars main Variable (loop-invariant
                // constants and non-phi raw values). This widens fast
                // path eligibility for `i + 1` patterns where the
                // const is in int_primary_vars but never shadowed.
                let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_add",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);

                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                    // Typed IR: raw i64 is PRIMARY.  Branchless iadd
                    // with deferred overflow — the 47-bit inline range
                    // check is deferred to boxing escape points
                    // (return_value, call args) via var_get_boxed /
                    // ensure_boxed_overflow_safe.  No boxing instruction
                    // is emitted here; the raw sum flows through
                    // Cranelift Variables directly.
                    let sum = builder.ins().iadd(lhs_raw, rhs_raw);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, sum);
                    }
                    return OpFlow::Continue;
                } else {
                    // op_prefers_int_lane proves both operands are Python
                    // `int`-typed, but that includes heap BigInts carried
                    // as TAG_PTR NaN-boxes. The raw shift-unbox is only
                    // value-exact for inline TAG_INT (and TAG_BOOL); a
                    // BigInt pointer would be truncated to garbage. Guard
                    // the raw lane on a runtime inline-int tag check and
                    // route BigInt / float / mixed operands to the boxed
                    // runtime helper, which is value-correct for all of
                    // them. The 47-bit inline overflow guard is retained.
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
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64); // boxed
                    builder.append_block_param(merge_block, types::I64); // raw shadow
                    let (lhs_xored, lhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                    let (rhs_xored, rhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                    let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                    let sum = builder.ins().iadd(lhs_val, rhs_val);
                    let fast_res = box_int_value(&mut *builder, sum, nbc);
                    let fits_inline = int_value_fits_inline(&mut *builder, sum);
                    let take_fast = builder.ins().band(both_int, fits_inline);
                    builder
                        .ins()
                        .brif(take_fast, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    jump_block(&mut *builder, merge_block, &[fast_res, sum]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let slow_res = builder.inst_results(call)[0];
                    let zero = builder.ins().iconst(types::I64, 0);
                    jump_block(&mut *builder, merge_block, &[slow_res, zero]);

                    switch_to_block_materialized(&mut *builder, merge_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);

                    builder.block_params(merge_block)[0]
                }
            } else if op_prefers_integer_runtime_lane(op) {
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
                    "molt_add",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                builder.inst_results(call)[0]
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
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_add",
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
                let sum = builder.ins().iadd(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, sum, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, sum);
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
                // Inline float fast path: if both operands are floats, do f64 add directly.
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
                let flt_sum = builder.ins().fadd(lhs_f, rhs_f);
                let flt_res = box_float_value(&mut *builder, flt_sum, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                emit_mixed_int_float_op(&mut *builder, *lhs, *rhs, nbc, 0, merge_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_var_from_numeric_result(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    int_primary_vars,
                    bool_primary_vars,
                    float_primary_vars,
                    nbc,
                    out__,
                    res,
                );
                // raw_int_shadow propagation is handled inside the
                // both-shadow path above (via merge phi).  Other paths
                // (tag-check, generic) don't shadow because the output
                // representation is unknown.
            }
        }
        "checked_add" => {
            // CheckedAdd from the overflow_peel transform. op.args =
            // [lhs, rhs], op.var = wrapping-sum output, op.out =
            // overflow-flag output. A TOTAL function with two lanes:
            //
            // RAW lane (both operands int-primary): hardware-exact
            // signed-overflow detection via Cranelift `sadd_overflow`
            // (a single instruction pair on x64/aarch64). When the
            // flag is set, the sum holds the mathematically WRAPPED
            // value — consumers may only observe it on the flag=0
            // branch (the peel's CFG enforces this; the slow loop is
            // seeded from the PRE-iteration values).
            //
            // BOXED lane (any operand unproven): the carrier chain
            // refused the raw promotion, so the values are NaN-boxed
            // ints/BigInts. Dispatch through `molt_add` — BigInt-exact
            // by construction, so the sum can NEVER silently wrap and
            // the overflow flag is CONSTANT FALSE (the peel's slow
            // path is correctly dead; the "fast" loop IS the boxed
            // loop — same semantics, no speedup). This mirrors the
            // Luau lowering exactly.
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]);
            let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
            if let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                let (sum, of) = builder.ins().sadd_overflow(lhs_raw, rhs_raw);
                if let Some(ref sum_name) = op.var {
                    // The chain can only admit the sum if it admitted
                    // the operands feeding it — a raw sum slot with a
                    // boxed def would truncate. Enforced here because
                    // this IS the trusted-unbox boundary.
                    assert!(
                        int_primary_vars.contains(sum_name),
                        "checked_add: raw operands but non-raw sum slot '{sum_name}' (carrier chain inconsistency)",
                    );
                    def_var_named(&mut *builder, vars, sum_name, sum);
                }
                if let Some(ref flag_name) = op.out {
                    // `of` is an i8 0/1; widen to the I64 raw-bool
                    // carrier convention.
                    let of_wide = builder.ins().uextend(types::I64, of);
                    def_raw_bool_value(
                        &mut *builder,
                        vars,
                        bool_primary_vars,
                        flag_name,
                        of_wide,
                        nbc,
                    );
                }
            } else {
                assert!(
                    op.var
                        .as_ref()
                        .is_none_or(|sum| !int_primary_vars.contains(sum)),
                    "checked_add: boxed operands but raw sum slot (carrier chain inconsistency)",
                );
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
                .expect("checked_add: LHS not found");
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
                .expect("checked_add: RHS not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_add",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let sum_boxed = builder.inst_results(call)[0];
                if let Some(ref sum_name) = op.var {
                    def_var_named(&mut *builder, vars, sum_name, sum_boxed);
                }
                if let Some(ref flag_name) = op.out {
                    let zero = builder.ins().iconst(types::I64, 0);
                    def_raw_bool_value(
                        &mut *builder,
                        vars,
                        bool_primary_vars,
                        flag_name,
                        zero,
                        nbc,
                    );
                }
            }
        }
        "checked_mul" => {
            // CheckedMul from the overflow_peel transform. op.args =
            // [lhs, rhs], op.var = wrapping-product output, op.out =
            // overflow-flag output. A TOTAL function with two lanes,
            // mirroring `checked_add` exactly.
            //
            // RAW lane (both operands int-primary): hardware-exact
            // signed-overflow detection via the `smulhi` pattern
            // (`imul_overflow64`). Cranelift 0.131 has NO `smul_overflow`
            // (unlike `sadd_overflow`), so overflow is witnessed by
            // `smulhi(lhs,rhs) != (prod >> 63)` — a FULL 64-bit-exact flag,
            // NOT the 47-bit `imul_checked_inline` inline-window test (which
            // would deopt the full-range accumulator 2^16x too early). When
            // the flag is set, the product holds the mathematically WRAPPED
            // value — consumers may only observe it on the flag=0 branch (the
            // peel's CFG enforces this; the slow loop is seeded from the
            // PRE-iteration values).
            //
            // BOXED lane (any operand unproven): the carrier chain refused
            // the raw promotion, so the values are NaN-boxed ints/BigInts.
            // Dispatch through `molt_mul` — BigInt-exact by construction, so
            // the product can NEVER silently wrap and the overflow flag is
            // CONSTANT FALSE (the peel's slow path is correctly dead; the
            // "fast" loop IS the boxed loop — same semantics, no speedup).
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]);
            let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
            if let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                let (prod, of) = imul_overflow64(&mut *builder, lhs_raw, rhs_raw);
                if let Some(ref prod_name) = op.var {
                    // The chain can only admit the product if it admitted the
                    // operands feeding it — a raw product slot with a boxed
                    // def would truncate. Enforced here because this IS the
                    // trusted-unbox boundary.
                    assert!(
                        int_primary_vars.contains(prod_name),
                        "checked_mul: raw operands but non-raw product slot '{prod_name}' (carrier chain inconsistency)",
                    );
                    def_var_named(&mut *builder, vars, prod_name, prod);
                }
                if let Some(ref flag_name) = op.out {
                    // `of` is an i8 0/1; widen to the I64 raw-bool carrier
                    // convention.
                    let of_wide = builder.ins().uextend(types::I64, of);
                    def_raw_bool_value(
                        &mut *builder,
                        vars,
                        bool_primary_vars,
                        flag_name,
                        of_wide,
                        nbc,
                    );
                }
            } else {
                assert!(
                    op.var
                        .as_ref()
                        .is_none_or(|prod| !int_primary_vars.contains(prod)),
                    "checked_mul: boxed operands but raw product slot (carrier chain inconsistency)",
                );
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
                .expect("checked_mul: LHS not found");
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
                .expect("checked_mul: RHS not found");
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_mul",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let prod_boxed = builder.inst_results(call)[0];
                if let Some(ref prod_name) = op.var {
                    def_var_named(&mut *builder, vars, prod_name, prod_boxed);
                }
                if let Some(ref flag_name) = op.out {
                    let zero = builder.ins().iconst(types::I64, 0);
                    def_raw_bool_value(
                        &mut *builder,
                        vars,
                        bool_primary_vars,
                        flag_name,
                        zero,
                        nbc,
                    );
                }
            }
        }
        "inplace_add" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Defer var_get: see "add" handler comment.
            let res = if op_prefers_str_lane(op) {
                // Both operands known to be strings — direct concat.
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
                    "molt_str_concat",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                builder.inst_results(call)[0]
            } else if op_prefers_float_lane(op) {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_f = float_value_for_mixed(
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
                    lhs_name,
                );
                let rhs_f = float_value_for_mixed(
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
                    rhs_name,
                );
                let result_f = builder.ins().fadd(lhs_f, rhs_f);
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| float_primary_vars.contains(o))
                {
                    result_f
                } else {
                    box_float_value(&mut *builder, result_f, nbc)
                }
            } else if op_prefers_int_lane(op) {
                // Raw chain: both operands already unboxed + overflow guard.
                // Propagate raw shadow via second merge phi.
                // Inside loops, use Variable-only shadows (phi-correct).
                // Use Option-based lookup (matching the `add` handler) so
                // that when inside a loop and only Value-tier shadows exist
                // (no Variable-tier), we fall through to the proven-int path
                // instead of panicking on unwrap.
                let lhs_val = int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]);
                let rhs_val = int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));
                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_val, rhs_val) {
                    // Typed IR: raw i64 is PRIMARY.  Branchless iadd
                    // with deferred overflow — no boxing emitted here.
                    let raw_result = builder.ins().iadd(lhs_raw, rhs_raw);
                    if let Some(ref out_name) = op.out {
                        def_var_named(&mut *builder, vars, out_name, raw_result);
                    }
                    return OpFlow::Continue;
                }
                // Proven-int fallback: shadows unavailable (e.g. inside loop
                // with only Value-tier shadows). Box both operands and use the
                // unbox-add-rebox path with overflow guard.
                // Propagate raw shadow so downstream ops skip unbox.
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
                    "molt_inplace_add",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64); // boxed
                builder.append_block_param(merge_block, types::I64); // raw shadow
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let sum = builder.ins().iadd(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, sum, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, sum);
                let take_fast = builder.ins().band(both_int, fits_inline);
                builder
                    .ins()
                    .brif(take_fast, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                jump_block(&mut *builder, merge_block, &[fast_res, sum]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                let zero = builder.ins().iconst(types::I64, 0);
                jump_block(&mut *builder, merge_block, &[slow_res, zero]);

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
                    "molt_inplace_add",
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
                let sum = builder.ins().iadd(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, sum, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, sum);
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
                let flt_sum = builder.ins().fadd(lhs_f, rhs_f);
                let flt_res = box_float_value(&mut *builder, flt_sum, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                emit_mixed_int_float_op(&mut *builder, *lhs, *rhs, nbc, 0, merge_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_var_from_numeric_result(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    int_primary_vars,
                    bool_primary_vars,
                    float_primary_vars,
                    nbc,
                    out__,
                    res,
                );
                // In-place addition can merge inline-int and boxed-bigint
                // results, so do not record a raw shadow for the merged value.
            }
        }
        // ── vec_* reduction family ──────────────────────────────
        // sum/prod/min/max over int and float sequences, plus the
        // _trusted / _range / _range_iter variants. Extracted to
        // fc::vec_reductions (M1 phase 1) so the handler is its own
        // codegen unit lifted out of this monolith.
        "vec_sum_int"
        | "vec_sum_int_trusted"
        | "vec_sum_int_range"
        | "vec_sum_int_range_trusted"
        | "vec_sum_int_range_iter"
        | "vec_sum_int_range_iter_trusted"
        | "vec_sum_float"
        | "vec_sum_float_trusted"
        | "vec_sum_float_range"
        | "vec_sum_float_range_trusted"
        | "vec_sum_float_range_iter"
        | "vec_sum_float_range_iter_trusted"
        | "vec_prod_int"
        | "vec_prod_int_trusted"
        | "vec_prod_int_range"
        | "vec_prod_int_range_trusted"
        | "vec_min_int"
        | "vec_min_int_trusted"
        | "vec_min_int_range"
        | "vec_min_int_range_trusted"
        | "vec_max_int"
        | "vec_max_int_trusted"
        | "vec_max_int_range"
        | "vec_max_int_range_trusted" => {
            fc::vec_reductions::handle_vec_reduction(
                op,
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                &mut *import_refs,
                &mut *sealed_blocks,
                vars,
                int_primary_vars,
                float_primary_vars,
                bool_primary_vars,
                nbc,
            );
        }
        "sub" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Defer var_get: see "add" handler comment.
            let res = if op_prefers_float_lane(op) {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_f = float_value_for_mixed(
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
                    lhs_name,
                );
                let rhs_f = float_value_for_mixed(
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
                    rhs_name,
                );
                let result_f = builder.ins().fsub(lhs_f, rhs_f);
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| float_primary_vars.contains(o))
                {
                    result_f
                } else {
                    box_float_value(&mut *builder, result_f, nbc)
                }
            } else if op_prefers_int_lane(op) {
                // LuaJIT-style unboxed arithmetic chain with overflow guard.
                // If both operands have raw i64 shadows, skip tag check + unbox.
                // If result overflows 47-bit inline range, fall to slow path.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let in_active_loop = !loop_stack.is_empty();
                let lhs_raw = if in_active_loop {
                    // Inside loops, only use Variable-backed shadows
                    // (phi-correct across back-edges). Value-tier
                    // shadows may hold stale SSA values from a
                    // previous block/iteration.
                    int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name)
                } else {
                    int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name)
                };
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_sub",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);

                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                    // Typed IR: raw i64 is PRIMARY.  Branchless isub
                    // with deferred overflow — the 47-bit inline range
                    // check is deferred to boxing escape points
                    // (return_value, call args) via var_get_boxed /
                    // ensure_boxed_overflow_safe.  No boxing instruction
                    // is emitted here; the raw difference flows through
                    // Cranelift Variables directly.
                    let diff = builder.ins().isub(lhs_raw, rhs_raw);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, diff);
                    }
                    return OpFlow::Continue;
                } else {
                    // Proven-int path: op_prefers_int_lane guarantees both
                    // operands are int-like. Skip tag check, unbox directly.
                    // Overflow guard retained for BigInt fallback.
                    // Propagate raw shadow so downstream ops skip unbox.
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
                    .unwrap_or_else(|| panic!("LHS not found in {} op {}", func_name, op_idx));
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
                    .unwrap_or_else(|| panic!("RHS not found in {} op {}", func_name, op_idx));
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64); // boxed
                    builder.append_block_param(merge_block, types::I64); // raw shadow
                    let (lhs_xored, lhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                    let (rhs_xored, rhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                    let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                    let diff = builder.ins().isub(lhs_val, rhs_val);
                    let fast_res = box_int_value(&mut *builder, diff, nbc);
                    let fits_inline = int_value_fits_inline(&mut *builder, diff);
                    let take_fast = builder.ins().band(both_int, fits_inline);
                    builder
                        .ins()
                        .brif(take_fast, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    jump_block(&mut *builder, merge_block, &[fast_res, diff]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let slow_res = builder.inst_results(call)[0];
                    let zero = builder.ins().iconst(types::I64, 0);
                    jump_block(&mut *builder, merge_block, &[slow_res, zero]);

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
                let diff = builder.ins().isub(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, diff, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, diff);
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
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_sub",
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
                let flt_diff = builder.ins().fsub(lhs_f, rhs_f);
                let flt_res = box_float_value(&mut *builder, flt_diff, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                emit_mixed_int_float_op(&mut *builder, *lhs, *rhs, nbc, 1, merge_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_var_from_numeric_result(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    int_primary_vars,
                    bool_primary_vars,
                    float_primary_vars,
                    nbc,
                    out__,
                    res,
                );
                // raw_int_shadow propagation is handled inside the
                // both-shadow path above (via merge phi).  Other paths
                // (tag-check, generic) don't shadow because the output
                // representation is unknown.
            }
        }
        "inplace_sub" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Defer var_get: see "add" handler comment.
            let res = if op_prefers_float_lane(op) {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_f = float_value_for_mixed(
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
                    lhs_name,
                );
                let rhs_f = float_value_for_mixed(
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
                    rhs_name,
                );
                let result_f = builder.ins().fsub(lhs_f, rhs_f);
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| float_primary_vars.contains(o))
                {
                    result_f
                } else {
                    box_float_value(&mut *builder, result_f, nbc)
                }
            } else if op
                .out
                .as_ref()
                .is_some_and(|out| int_primary_vars.contains(out))
                && (int_primary_vars.contains(args[0].as_str()))
                && (int_primary_vars.contains(args[1].as_str()))
                && op_prefers_int_lane(op)
            {
                // Raw chain: both operands already unboxed + overflow guard.
                // Propagate raw shadow via second merge phi.
                // Inside loops, use Variable-only shadows (phi-correct).
                let lhs_val =
                    int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]).unwrap();
                let rhs_val =
                    int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]).unwrap();
                // Typed IR: raw i64 is PRIMARY.  Branchless isub
                // with deferred overflow — no boxing emitted here.
                let raw_result = builder.ins().isub(lhs_val, rhs_val);
                if let Some(ref out_name) = op.out {
                    def_var_named(&mut *builder, vars, out_name, raw_result);
                }
                return OpFlow::Continue;
            } else if op_prefers_int_lane(op) {
                // Propagate raw shadow so downstream ops skip unbox.
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
                .unwrap_or_else(|| panic!("LHS not found in {} op {}", func_name, op_idx));
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
                .unwrap_or_else(|| panic!("RHS not found in {} op {}", func_name, op_idx));
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_inplace_sub",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64); // boxed
                builder.append_block_param(merge_block, types::I64); // raw shadow
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let diff = builder.ins().isub(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, diff, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, diff);
                let take_fast = builder.ins().band(both_int, fits_inline);
                builder
                    .ins()
                    .brif(take_fast, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                jump_block(&mut *builder, merge_block, &[fast_res, diff]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                let zero = builder.ins().iconst(types::I64, 0);
                jump_block(&mut *builder, merge_block, &[slow_res, zero]);

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
                let diff = builder.ins().isub(lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, diff, nbc);
                let fits_inline = int_value_fits_inline(&mut *builder, diff);
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
                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_inplace_sub",
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
                let flt_diff = builder.ins().fsub(lhs_f, rhs_f);
                let flt_res = box_float_value(&mut *builder, flt_diff, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                emit_mixed_int_float_op(&mut *builder, *lhs, *rhs, nbc, 1, merge_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_var_from_numeric_result(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    int_primary_vars,
                    bool_primary_vars,
                    float_primary_vars,
                    nbc,
                    out__,
                    res,
                );
                // In-place subtraction can merge inline-int and boxed-bigint
                // results, so do not record a raw shadow for the merged value.
            }
        }
        "mul" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Defer var_get: see "add" handler comment.
            let res = if op_prefers_float_lane(op) {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_f = float_value_for_mixed(
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
                    lhs_name,
                );
                let rhs_f = float_value_for_mixed(
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
                    rhs_name,
                );
                let result_f = builder.ins().fmul(lhs_f, rhs_f);
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| float_primary_vars.contains(o))
                {
                    result_f
                } else {
                    box_float_value(&mut *builder, result_f, nbc)
                }
            } else if op_prefers_int_lane(op) {
                // LuaJIT-style unboxed arithmetic chain with overflow guard.
                // If both operands have raw i64 shadows, skip tag check + unbox.
                // If result overflows 47-bit inline range, fall to slow path.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let in_active_loop = !loop_stack.is_empty();
                let lhs_raw = if in_active_loop {
                    // Inside loops, only use Variable-backed shadows
                    // (phi-correct across back-edges). Value-tier
                    // shadows may hold stale SSA values from a
                    // previous block/iteration.
                    int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name)
                } else {
                    int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name)
                };
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));

                let callee = SimpleBackend::import_func_id_split(
                    &mut *module,
                    &mut *import_ids,
                    "molt_mul",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);

                if out_is_int_primary && let (Some(lhs_raw), Some(rhs_raw)) = (lhs_raw, rhs_raw) {
                    // Typed IR: raw i64 is PRIMARY.  Branchless imul
                    // with deferred overflow — the 47-bit inline range
                    // check is deferred to boxing escape points
                    // (return_value, call args) via var_get_boxed /
                    // ensure_boxed_overflow_safe.  No boxing instruction
                    // is emitted here; the raw product flows through
                    // Cranelift Variables directly.
                    let prod = builder.ins().imul(lhs_raw, rhs_raw);
                    if let Some(ref out__) = op.out {
                        def_var_named(&mut *builder, vars, out__, prod);
                    }
                    return OpFlow::Continue;
                } else {
                    // Proven-int path: op_prefers_int_lane guarantees both
                    // operands are int-like. Skip tag check, unbox directly.
                    // Overflow guard retained for BigInt fallback.
                    // Propagate raw shadow so downstream ops skip unbox.
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
                    let fast_block = builder.create_block();
                    let slow_block = builder.create_block();
                    builder.set_cold_block(slow_block);
                    let merge_block = builder.create_block();
                    builder.append_block_param(merge_block, types::I64); // boxed
                    builder.append_block_param(merge_block, types::I64); // raw shadow
                    let (lhs_xored, lhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                    let (rhs_xored, rhs_val) =
                        fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                    let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                    let (prod, fits) = imul_checked_inline(&mut *builder, lhs_val, rhs_val);
                    let fast_res = box_int_value(&mut *builder, prod, nbc);
                    let take_fast = builder.ins().band(both_int, fits);
                    builder
                        .ins()
                        .brif(take_fast, fast_block, &[], slow_block, &[]);

                    switch_to_block_materialized(&mut *builder, fast_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                    jump_block(&mut *builder, merge_block, &[fast_res, prod]);

                    switch_to_block_materialized(&mut *builder, slow_block);
                    seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                    let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                    let slow_res = builder.inst_results(call)[0];
                    let zero = builder.ins().iconst(types::I64, 0);
                    jump_block(&mut *builder, merge_block, &[slow_res, zero]);

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
                    "molt_mul",
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
                let (prod, fits) = imul_checked_inline(&mut *builder, lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, prod, nbc);
                brif_block(
                    &mut *builder,
                    fits,
                    merge_block,
                    &[fast_res],
                    slow_block,
                    &[],
                );

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
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
                let flt_prod = builder.ins().fmul(lhs_f, rhs_f);
                let flt_res = box_float_value(&mut *builder, flt_prod, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                emit_mixed_int_float_op(&mut *builder, *lhs, *rhs, nbc, 2, merge_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_var_from_numeric_result(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    int_primary_vars,
                    bool_primary_vars,
                    float_primary_vars,
                    nbc,
                    out__,
                    res,
                );
                // raw_int_shadow propagation is handled inside the
                // both-shadow path above (via merge phi).  Other paths
                // (tag-check, generic) don't shadow because the output
                // representation is unknown.
            }
        }
        "inplace_mul" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            // Defer var_get: see "add" handler comment.
            let res = if op_prefers_float_lane(op) {
                // Float-primary operands are raw f64; boxed floats are extracted explicitly.
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_f = float_value_for_mixed(
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
                    lhs_name,
                );
                let rhs_f = float_value_for_mixed(
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
                    rhs_name,
                );
                let result_f = builder.ins().fmul(lhs_f, rhs_f);
                if op
                    .out
                    .as_ref()
                    .is_some_and(|o| float_primary_vars.contains(o))
                {
                    result_f
                } else {
                    box_float_value(&mut *builder, result_f, nbc)
                }
            } else if op
                .out
                .as_ref()
                .is_some_and(|out| int_primary_vars.contains(out))
                && (int_primary_vars.contains(args[0].as_str()))
                && (int_primary_vars.contains(args[1].as_str()))
                && op_prefers_int_lane(op)
            {
                // Raw chain: both operands already unboxed + overflow guard.
                // Inside loops, use Variable-only shadows (phi-correct).
                let lhs_val =
                    int_raw_value(&mut *builder, vars, int_primary_vars, &args[0]).unwrap();
                let rhs_val =
                    int_raw_value(&mut *builder, vars, int_primary_vars, &args[1]).unwrap();
                // Typed IR: raw i64 is PRIMARY.  Branchless imul
                // with deferred overflow — no boxing emitted here.
                let raw_result = builder.ins().imul(lhs_val, rhs_val);
                if let Some(ref out_name) = op.out {
                    def_var_named(&mut *builder, vars, out_name, raw_result);
                }
                return OpFlow::Continue;
            } else if op_prefers_int_lane(op) {
                // Propagate raw shadow so downstream ops skip unbox.
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
                    "molt_inplace_mul",
                    &[types::I64, types::I64],
                    &[types::I64],
                );
                let local_callee = module.declare_func_in_func(callee, builder.func);
                let fast_block = builder.create_block();
                let slow_block = builder.create_block();
                builder.set_cold_block(slow_block);
                let merge_block = builder.create_block();
                builder.append_block_param(merge_block, types::I64); // boxed
                builder.append_block_param(merge_block, types::I64); // raw shadow
                let (lhs_xored, lhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *lhs, nbc);
                let (rhs_xored, rhs_val) = fused_tag_check_and_unbox_int(&mut *builder, *rhs, nbc);
                let both_int = fused_both_int_check(&mut *builder, lhs_xored, rhs_xored, nbc);
                let (prod, fits) = imul_checked_inline(&mut *builder, lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, prod, nbc);
                let take_fast = builder.ins().band(both_int, fits);
                builder
                    .ins()
                    .brif(take_fast, fast_block, &[], slow_block, &[]);

                switch_to_block_materialized(&mut *builder, fast_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, fast_block);
                jump_block(&mut *builder, merge_block, &[fast_res, prod]);

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                let zero = builder.ins().iconst(types::I64, 0);
                jump_block(&mut *builder, merge_block, &[slow_res, zero]);

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
                    "molt_inplace_mul",
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
                let (prod, fits) = imul_checked_inline(&mut *builder, lhs_val, rhs_val);
                let fast_res = box_int_value(&mut *builder, prod, nbc);
                brif_block(
                    &mut *builder,
                    fits,
                    merge_block,
                    &[fast_res],
                    slow_block,
                    &[],
                );

                switch_to_block_materialized(&mut *builder, slow_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, slow_block);
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
                let flt_prod = builder.ins().fmul(lhs_f, rhs_f);
                let flt_res = box_float_value(&mut *builder, flt_prod, nbc);
                jump_block(&mut *builder, merge_block, &[flt_res]);

                switch_to_block_materialized(&mut *builder, call_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, call_block);
                emit_mixed_int_float_op(&mut *builder, *lhs, *rhs, nbc, 2, merge_block);
                let call = builder.ins().call(local_callee, &[*lhs, *rhs]);
                let slow_res = builder.inst_results(call)[0];
                jump_block(&mut *builder, merge_block, &[slow_res]);

                switch_to_block_materialized(&mut *builder, merge_block);
                seal_block_once(&mut *builder, &mut *sealed_blocks, merge_block);
                builder.block_params(merge_block)[0]
            };
            if let Some(ref out__) = op.out {
                def_var_from_numeric_result(
                    &mut *module,
                    &mut *import_ids,
                    &mut *builder,
                    &mut *import_refs,
                    vars,
                    int_primary_vars,
                    bool_primary_vars,
                    float_primary_vars,
                    nbc,
                    out__,
                    res,
                );
                // In-place multiplication can merge inline-int and boxed-bigint
                // results, so do not record a raw shadow for the merged value.
            }
        }
        "bit_or" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let res = if op_prefers_int_lane(op) {
                let lhs_name = &args[0];
                let rhs_name = &args[1];
                let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));

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
                let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));

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
                let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));

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
        "matmul" | "inplace_matmul" => {
            // `@` and `@=`.  Inplace variant: molt_inplace_matmul tries
            // __imatmul__ before the binary __matmul__/__rmatmul__ chain.
            let boxed_sym = if op.kind == "inplace_matmul" {
                "molt_inplace_matmul"
            } else {
                "molt_matmul"
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
                    .is_some_and(|o| float_primary_vars.contains(o));
                let lhs_f = float_value_for_mixed(
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
                    lhs_name,
                );
                let rhs_f = float_value_for_mixed(
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
                    int_primary_vars,
                    float_primary_vars,
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
                    int_primary_vars,
                    float_primary_vars,
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
                let slow_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), slow_res);
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
                    .is_some_and(|o| float_primary_vars.contains(o));
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
                let slow_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), slow_res);
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
                    .is_some_and(|o| float_primary_vars.contains(o));
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
                let slow_f = builder.ins().bitcast(types::F64, MemFlagsData::new(), slow_res);
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
                let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));

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
                        int_primary_vars,
                        float_primary_vars,
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
                        int_primary_vars,
                        float_primary_vars,
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
                let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);
                let out_is_int_primary = op
                    .out
                    .as_ref()
                    .is_some_and(|out| int_primary_vars.contains(out));

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
                        int_primary_vars,
                        float_primary_vars,
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
                        int_primary_vars,
                        float_primary_vars,
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
                let lhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, lhs_name);
                let rhs_raw = int_raw_value(&mut *builder, vars, int_primary_vars, rhs_name);

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
                        int_primary_vars,
                        float_primary_vars,
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
                        int_primary_vars,
                        float_primary_vars,
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
            let modulus = var_get_boxed_overflow_safe(
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
                int_primary_vars,
                float_primary_vars,
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
        _ => unreachable!("non-arithmetic op routed to handle_arith_op"),
    }
    OpFlow::Proceed
}
