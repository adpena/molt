use super::super::*;

/// Single-source kind authority for [`handle_loop_op`], consulted by
/// `op_family::FAMILY_DISPATCH_TABLE`. Mirror the `match op.kind.as_str()` arms below.
#[cfg(feature = "native-backend")]
pub(in crate::native_backend::function_compiler) const HANDLED_KINDS: &[&str] = &[
    "loop_start",
    "loop_index_start",
    "loop_break_if_exception",
    "loop_break_if_true",
    "loop_break_if_false",
    "loop_break",
    "loop_index_next",
    "loop_continue",
    "loop_end",
];
use super::OpFlow;
use super::list_index_fast_path::{
    ListIndexFastPathState, collect_pre_loop_defined_names, loop_start_has_index_prelude,
    scan_loop_hoistable_lists, scan_loop_int_sum_reduction,
};
use super::var_get_boxed_overflow_safe_fn;

/// Cranelift codegen handlers for structured loop lowering.
///
/// Extracted from compile_func_inner's per-op dispatch (M1.10). Loop
/// frame state, hoisted list caches, reduction skips, and cleanup-on-break
/// bookkeeping are threaded explicitly. Original outer op-loop `continue`
/// exits return `OpFlow::Continue` so the parent epilogue is skipped exactly
/// where the inline arms skipped it.
#[cfg(feature = "native-backend")]
#[allow(clippy::too_many_arguments)]
pub(in crate::native_backend::function_compiler) fn handle_loop_op(
    op: &OpIR,
    op_idx: usize,
    func_ir: &FunctionIR,
    module: &mut ObjectModule,
    import_ids: &mut BTreeMap<&'static str, (cranelift_module::FuncId, ImportSignatureShape)>,
    builder: &mut FunctionBuilder<'_>,
    import_refs: &mut BTreeMap<&'static str, FuncRef>,
    sealed_blocks: &mut BTreeSet<Block>,
    vars: &BTreeMap<String, Variable>,
    representation_plan: &ScalarRepresentationPlan,
    int_like_vars: &BTreeSet<String>,
    bool_like_vars: &BTreeSet<String>,
    last_use: &BTreeMap<String, usize>,
    alias_roots: &BTreeMap<String, String>,
    exception_label_ids: &BTreeSet<i64>,
    loop_body_init_vars: &BTreeMap<usize, Vec<String>>,
    list_index_fast_paths: &mut ListIndexFastPathState,
    block_tracked_obj: &mut BTreeMap<Block, Vec<String>>,
    block_tracked_ptr: &mut BTreeMap<Block, Vec<String>>,
    entry_vars: &BTreeMap<String, Value>,
    already_decrefed: &mut BTreeSet<String>,
    reachable_blocks: &mut BTreeSet<Block>,
    loop_stack: &mut Vec<LoopFrame>,
    skip_ops: &mut BTreeSet<usize>,
    loop_depth: &mut i32,
    is_block_filled: &mut bool,
    native_rc_tracking_enabled: bool,
    scalar_fast_paths_enabled: bool,
    debug_loop_cfg: Option<&str>,
    debug_block_origins: Option<&str>,
    maybe_debug_seal: &dyn Fn(&str, usize, Block),
    local_exc_pending_fast: FuncRef,
    exc_flag_ptr_slot: Option<cranelift_codegen::ir::StackSlot>,
    local_dec_ref_obj: FuncRef,
    nbc: &crate::NanBoxConsts,
) -> OpFlow {
    let ops = &func_ir.ops;
    let var_is_int = |name: &str| {
        scalar_fast_paths_enabled
            && (int_like_vars.contains(name) || representation_plan.is_raw_int_carrier_name(name))
    };
    let var_is_bool = |name: &str| scalar_fast_paths_enabled && bool_like_vars.contains(name);
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
        "loop_start" => {
            let indexed_loop_follows = loop_start_has_index_prelude(&func_ir.ops, op_idx);
            if indexed_loop_follows {
                // Indexed loops may carry a constant-materialization
                // prelude between LOOP_START and LOOP_INDEX_START.
                // LOOP_INDEX_START owns the real loop frame and IV.
                return OpFlow::Continue;
            }
            let loop_block = builder.create_block();
            let body_block = builder.create_block();
            let after_block = builder.create_block();
            if !*is_block_filled {
                // Initialize loop-body output variables to None (0)
                // before entering the loop header.  This ensures that
                // the SSA Variable has a valid reaching definition on
                // the first iteration so the per-iteration dec_ref at
                // the back-edge safely no-ops (molt_dec_ref_obj skips
                // non-pointer NaN-boxed values).
                if let Some(body_vars) = loop_body_init_vars.get(&op_idx) {
                    let none_val = builder.ins().iconst(types::I64, 0);
                    let none_f64 = builder.ins().f64const(0.0);
                    for name in body_vars {
                        if representation_plan.is_float_unboxed(name) {
                            def_var_named(&mut *builder, vars, name, none_f64);
                        } else {
                            def_var_named(&mut *builder, vars, name, none_val);
                        }
                    }
                }
                // ── Loop-invariant list pointer hoisting ──────────
                // Scan the loop body to find list variables that are
                // accessed (index/store_index) but never mutated
                // (append/pop/etc).  For those, emit the NaN-unbox +
                // data_ptr/len loads HERE (in the pre-loop block) so
                // the results live in Cranelift Variables that persist
                // across iterations via phi nodes.  The in-loop cache
                // lookup will then hit on every iteration.
                {
                    let mut pre_loop_defined = collect_pre_loop_defined_names(&func_ir.ops, op_idx);
                    for p in func_ir.params.iter().filter(|n| n.as_str() != "none") {
                        pre_loop_defined.insert(p.clone());
                    }
                    let (li_hoist, lg_hoist) = scan_loop_hoistable_lists(
                        &func_ir.ops,
                        op_idx,
                        &pre_loop_defined,
                        representation_plan,
                    );
                    for list_name in &li_hoist {
                        if list_index_fast_paths
                            .list_int_data_cache
                            .contains_key(list_name)
                        {
                            continue; // already cached from an outer scope
                        }
                        let Some(obj) = var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            sealed_blocks,
                            vars,
                            list_name,
                            representation_plan,
                        ) else {
                            continue;
                        };
                        let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                        let shifted = builder.ins().ishl_imm(masked, 16);
                        let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                        let storage_ptr =
                            builder
                                .ins()
                                .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
                        let dp = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            LIST_INT_STORAGE_DATA_OFFSET,
                        );
                        let len = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            LIST_INT_STORAGE_LEN_OFFSET,
                        );
                        let dvar = builder.declare_var(types::I64);
                        builder.def_var(dvar, dp);
                        list_index_fast_paths
                            .list_int_data_cache
                            .insert(list_name.clone(), dvar);
                        let lvar = builder.declare_var(types::I64);
                        builder.def_var(lvar, len);
                        list_index_fast_paths
                            .list_int_len_cache
                            .insert(list_name.clone(), lvar);
                    }
                    for list_name in &lg_hoist {
                        if list_index_fast_paths
                            .list_data_cache
                            .contains_key(list_name)
                        {
                            continue;
                        }
                        let Some(obj) = var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            sealed_blocks,
                            vars,
                            list_name,
                            representation_plan,
                        ) else {
                            continue;
                        };
                        let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                        let shifted = builder.ins().ishl_imm(masked, 16);
                        let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                        // Load type_id to distinguish list vs list_bool.
                        let tid = builder.ins().load(
                            types::I32,
                            MemFlagsData::trusted(),
                            obj_ptr,
                            HEADER_TYPE_ID_OFFSET,
                        );
                        let bool_tid = builder.ins().iconst(types::I32, JIT_TYPE_ID_LIST_BOOL);
                        let is_bool = builder.ins().icmp(IntCC::Equal, tid, bool_tid);
                        let ibvar = builder.declare_var(types::I8);
                        builder.def_var(ibvar, is_bool);
                        list_index_fast_paths
                            .list_is_bool_cache
                            .insert(list_name.clone(), ibvar);
                        let storage_ptr =
                            builder
                                .ins()
                                .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
                        let vec_layout = vec_u64_layout();
                        // ListBoolStorage (repr(C)): data@0, len@8
                        let dp_bool = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            0i32,
                        );
                        let len_bool = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            8i32,
                        );
                        // Vec<u64> (repr(Rust), probed offsets)
                        let dp_vec = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            vec_layout.data_offset,
                        );
                        let len_vec = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            vec_layout.len_offset,
                        );
                        let dp = builder.ins().select(is_bool, dp_bool, dp_vec);
                        let len = builder.ins().select(is_bool, len_bool, len_vec);
                        let dvar = builder.declare_var(types::I64);
                        builder.def_var(dvar, dp);
                        list_index_fast_paths
                            .list_data_cache
                            .insert(list_name.clone(), dvar);
                        let lvar = builder.declare_var(types::I64);
                        builder.def_var(lvar, len);
                        list_index_fast_paths
                            .list_len_cache
                            .insert(list_name.clone(), lvar);
                    }
                }

                ensure_block_in_layout(&mut *builder, loop_block);
                reachable_blocks.insert(loop_block);
                jump_block(&mut *builder, loop_block, &[]);
                switch_to_block_with_rebind(&mut *builder, loop_block, is_block_filled, false);
            } else {
                *is_block_filled = true;
            }
            loop_stack.push(LoopFrame {
                loop_block,
                body_block,
                after_block,
                index_name: None,
                next_index: None,
                linearized: false,
            });
            *loop_depth += 1;
        }
        "loop_index_start" => {
            let Some(out_name) = op.out.as_ref() else {
                return OpFlow::Continue;
            };
            if !*is_block_filled {
                let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);

                // Detect linearized TIR loops: the TIR optimizer may
                // replace structured loop_continue/loop_end with
                // store_var + jump to a state label.  In that case the
                // back-edge bypasses any dedicated loop block we create.
                // Scope-aware: skip over inner loops by tracking depth.
                let (has_structured_backedge, contains_nested_loop) = {
                    let mut depth = 0i32;
                    let mut found_backedge = false;
                    let mut nested_loop = false;
                    for i in (op_idx + 1)..ops.len() {
                        match ops[i].kind.as_str() {
                            "loop_start" => {
                                if depth == 0 {
                                    nested_loop = true;
                                }
                                depth += 1;
                            }
                            "loop_index_start" => {}
                            "loop_end" if depth > 0 => {
                                depth -= 1;
                            }
                            "loop_end" => {
                                // raw_int_shadow preserved across loop iterations (LuaJIT-style)
                                found_backedge = true;
                                break;
                            }
                            "loop_continue" if depth == 0 => {
                                found_backedge = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    (found_backedge, nested_loop)
                };

                // Try to find the phi variable for the counter via
                // the store_var/load_var pattern from TIR optimization.
                let phi_value: Option<Value> = 'find_phi: {
                    // Step 1: forward-scan for loop_index_next output
                    let mut next_out: Option<&str> = None;
                    for fwd in (op_idx + 1)..ops.len() {
                        if ops[fwd].kind == "loop_index_next" {
                            next_out = ops[fwd].out.as_deref();
                            break;
                        }
                        if ops[fwd].kind == "loop_end" {
                            break;
                        }
                    }
                    // Step 2: find store_var _bb*_arg* that stores it
                    let mut arg_name: Option<String> = None;
                    if let Some(next) = next_out {
                        for fwd in (op_idx + 1)..ops.len() {
                            let f = &ops[fwd];
                            if f.kind == "store_var"
                                && let (Some(v), Some(a)) = (&f.var, &f.args)
                                && v.starts_with("_bb")
                                && v.contains("_arg")
                                && a.first().map(|s| s.as_str()) == Some(next)
                            {
                                arg_name = Some(v.clone());
                                break;
                            }
                            if f.kind == "loop_end" {
                                break;
                            }
                        }
                    }
                    // Step 3: backward-scan for load_var of that arg
                    if let Some(ref an) = arg_name {
                        for bwd in (0..op_idx).rev() {
                            let b = &ops[bwd];
                            if b.kind == "load_var"
                                && b.var.as_deref() == Some(an.as_str())
                                && let Some(ref out) = b.out
                                && let Some(v) = var_get_boxed_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    out,
                                    representation_plan,
                                )
                            {
                                break 'find_phi Some(*v);
                            }
                        }
                    }
                    None
                };

                let allow_linearized_loop = !has_structured_backedge && phi_value.is_some();
                if debug_loop_cfg.is_some() {
                    eprintln!(
                        "LOOP_CFG {} op{} loop_index_start filled={} depth={} linearized={} structured_backedge={} nested_loop={} phi={}",
                        func_ir.name,
                        op_idx,
                        *is_block_filled,
                        *loop_depth,
                        allow_linearized_loop,
                        has_structured_backedge,
                        contains_nested_loop,
                        phi_value.is_some(),
                    );
                }
                if allow_linearized_loop {
                    // Linearized loop: define counter directly from the
                    // phi variable.  The back-edge flows through
                    // store_var/load_var on the state label block, so
                    // SSA resolution handles the counter update.
                    // Each loop level discovers its own phi variable
                    // independently, so this path is valid for nested
                    // loops too.  The LoopFrame is marked linearized
                    // so loop_end skips the loop_depth decrement.
                    def_var_named(&mut *builder, vars, out_name.clone(), phi_value.unwrap());
                    // Initialize loop-body output variables for
                    // linearized loops (same rationale as structured).
                    if let Some(body_vars) = loop_body_init_vars.get(&op_idx) {
                        let none_val = builder.ins().iconst(types::I64, 0);
                        let none_f64 = builder.ins().f64const(0.0);
                        for name in body_vars {
                            if representation_plan.is_float_unboxed(name) {
                                def_var_named(&mut *builder, vars, name, none_f64);
                            } else {
                                def_var_named(&mut *builder, vars, name, none_val);
                            }
                        }
                    }
                    let dummy = builder.create_block();
                    loop_stack.push(LoopFrame {
                        loop_block: dummy,
                        body_block: dummy,
                        after_block: dummy,
                        index_name: Some(out_name.clone()),
                        next_index: None,
                        linearized: true,
                    });
                    // Note: loop_depth NOT incremented for linearized loops;
                    // loop_end checks frame.linearized to skip the decrement.
                    return OpFlow::Continue;
                }

                // Structured loop: NO explicit block params for the
                // counter.  Cranelift's Variable SSA handles the phi.
                //
                // Why not explicit params?  When loop_index_next does
                // def_var(counter_var, new_val), and the loop header
                // also has def_var(counter_var, block_param), Cranelift's
                // seal_all_blocks adds a DUPLICATE implicit param for
                // counter_var alongside the explicit one.  This breaks
                // remove_constant_phis (assertion: 30 args vs 29 params).
                //
                // The correct Cranelift pattern for loops:
                // 1. def_var(V, initial) BEFORE the jump to loop header
                // 2. In the loop header, use_var(V) → SSA phi
                // 3. On the back-edge, def_var(V, incremented) then jump
                // Cranelift creates the phi param automatically.
                let loop_block = builder.create_block();
                let body_block = builder.create_block();
                let after_block = builder.create_block();
                let start = phi_value.unwrap_or_else(|| {
                    *var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("Loop index start not found")
                });
                let start = if representation_plan.is_raw_int_carrier_name(out_name.as_str()) {
                    args.first()
                        .and_then(|name| {
                            int_raw_value(&mut *builder, vars, representation_plan, name)
                        })
                        .unwrap_or_else(|| unbox_int_or_bool(&mut *builder, start, nbc))
                } else {
                    start
                };
                // Step 1: define counter Variable with initial value.
                // For representation_plan the main Variable is the raw i64
                // carrier; Cranelift SSA provides the loop phi.
                def_var_named(&mut *builder, vars, out_name.clone(), start);
                // Initialize loop-body output variables to None (0)
                // before entering the loop header -- see loop_start
                // for the full rationale.
                if let Some(body_vars) = loop_body_init_vars.get(&op_idx) {
                    let none_val = builder.ins().iconst(types::I64, 0);
                    let none_f64 = builder.ins().f64const(0.0);
                    for name in body_vars {
                        if representation_plan.is_float_unboxed(name) {
                            def_var_named(&mut *builder, vars, name, none_f64);
                        } else {
                            def_var_named(&mut *builder, vars, name, none_val);
                        }
                    }
                }

                // ── Loop-invariant list pointer hoisting (indexed) ──
                // Same as the loop_start hoisting — emit data_ptr/len
                // loads in the pre-loop block so the in-loop cache hits
                // on every iteration.
                {
                    let mut pre_loop_defined = collect_pre_loop_defined_names(&func_ir.ops, op_idx);
                    for p in func_ir.params.iter().filter(|n| n.as_str() != "none") {
                        pre_loop_defined.insert(p.clone());
                    }
                    let (li_hoist, lg_hoist) = scan_loop_hoistable_lists(
                        &func_ir.ops,
                        op_idx,
                        &pre_loop_defined,
                        representation_plan,
                    );
                    for list_name in &li_hoist {
                        if list_index_fast_paths
                            .list_int_data_cache
                            .contains_key(list_name)
                        {
                            continue;
                        }
                        let Some(obj) = var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            sealed_blocks,
                            vars,
                            list_name,
                            representation_plan,
                        ) else {
                            continue;
                        };
                        let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                        let shifted = builder.ins().ishl_imm(masked, 16);
                        let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                        let storage_ptr =
                            builder
                                .ins()
                                .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
                        let dp = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            LIST_INT_STORAGE_DATA_OFFSET,
                        );
                        let len = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            LIST_INT_STORAGE_LEN_OFFSET,
                        );
                        let dvar = builder.declare_var(types::I64);
                        builder.def_var(dvar, dp);
                        list_index_fast_paths
                            .list_int_data_cache
                            .insert(list_name.clone(), dvar);
                        let lvar = builder.declare_var(types::I64);
                        builder.def_var(lvar, len);
                        list_index_fast_paths
                            .list_int_len_cache
                            .insert(list_name.clone(), lvar);
                    }
                    for list_name in &lg_hoist {
                        if list_index_fast_paths
                            .list_data_cache
                            .contains_key(list_name)
                        {
                            continue;
                        }
                        let Some(obj) = var_get_boxed_overflow_safe(
                            &mut *module,
                            &mut *import_ids,
                            &mut *builder,
                            import_refs,
                            sealed_blocks,
                            vars,
                            list_name,
                            representation_plan,
                        ) else {
                            continue;
                        };
                        let masked = builder.ins().band_imm(*obj, POINTER_MASK as i64);
                        let shifted = builder.ins().ishl_imm(masked, 16);
                        let obj_ptr = builder.ins().sshr_imm(shifted, 16);
                        let tid = builder.ins().load(
                            types::I32,
                            MemFlagsData::trusted(),
                            obj_ptr,
                            HEADER_TYPE_ID_OFFSET,
                        );
                        let bool_tid = builder.ins().iconst(types::I32, JIT_TYPE_ID_LIST_BOOL);
                        let is_bool = builder.ins().icmp(IntCC::Equal, tid, bool_tid);
                        let ibvar = builder.declare_var(types::I8);
                        builder.def_var(ibvar, is_bool);
                        list_index_fast_paths
                            .list_is_bool_cache
                            .insert(list_name.clone(), ibvar);
                        let storage_ptr =
                            builder
                                .ins()
                                .load(types::I64, MemFlagsData::trusted(), obj_ptr, 0);
                        let vec_layout = vec_u64_layout();
                        let dp_bool = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            0i32,
                        );
                        let len_bool = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            8i32,
                        );
                        let dp_vec = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            vec_layout.data_offset,
                        );
                        let len_vec = builder.ins().load(
                            types::I64,
                            MemFlagsData::trusted(),
                            storage_ptr,
                            vec_layout.len_offset,
                        );
                        let dp = builder.ins().select(is_bool, dp_bool, dp_vec);
                        let len = builder.ins().select(is_bool, len_bool, len_vec);
                        let dvar = builder.declare_var(types::I64);
                        builder.def_var(dvar, dp);
                        list_index_fast_paths
                            .list_data_cache
                            .insert(list_name.clone(), dvar);
                        let lvar = builder.declare_var(types::I64);
                        builder.def_var(lvar, len);
                        list_index_fast_paths
                            .list_len_cache
                            .insert(list_name.clone(), lvar);
                    }
                }

                // ── 4x Loop Unrolling for sum reductions ─────────
                // Before emitting the standard structured loop, check
                // if the loop body is a simple sum reduction over a
                // structurally proven flat int list.  If so, emit a 4x-unrolled main loop +
                // scalar epilogue and skip all body ops.
                if let Some(reduction) =
                    scan_loop_int_sum_reduction(&func_ir.ops, op_idx, out_name, representation_plan)
                {
                    // We need:
                    //   - data_ptr from list_index_fast_paths.list_int_data_cache (hoisted above)
                    //   - len from list_index_fast_paths.list_int_len_cache (hoisted above)
                    //   - initial accumulator value from the acc_operand_name
                    //   - start index (already in out_name / start variable)
                    let data_ptr_var = list_index_fast_paths
                        .list_int_data_cache
                        .get(&reduction.list_name);
                    let len_var = list_index_fast_paths
                        .list_int_len_cache
                        .get(&reduction.list_name);
                    if let (Some(&dp_var), Some(&ln_var)) = (data_ptr_var, len_var) {
                        let data_ptr = builder.use_var(dp_var);
                        let len_val = builder.use_var(ln_var);

                        // Get the initial accumulator value (raw i64).
                        let init_acc = {
                            int_raw_value(
                                &mut *builder,
                                vars,
                                representation_plan,
                                &reduction.acc_operand_name,
                            )
                            .unwrap_or_else(|| {
                                let boxed = var_get_boxed_overflow_safe(
                                    &mut *module,
                                    &mut *import_ids,
                                    &mut *builder,
                                    import_refs,
                                    sealed_blocks,
                                    vars,
                                    &reduction.acc_operand_name,
                                    representation_plan,
                                )
                                .expect("Sum reduction accumulator not found");
                                unbox_int(&mut *builder, *boxed, nbc)
                            })
                        };

                        // Compute unrolled_len = len & ~3  (round down to multiple of 4)
                        let unrolled_len = builder.ins().band_imm(len_val, !3i64);

                        // Get raw start index (i64). For list iteration
                        // this is typically 0, produced by iter_devirt.
                        let raw_start_idx = args
                            .first()
                            .and_then(|name| {
                                int_raw_value(&mut *builder, vars, representation_plan, name)
                            })
                            .unwrap_or_else(|| {
                                if representation_plan.is_raw_int_carrier_name(out_name.as_str()) {
                                    start
                                } else {
                                    unbox_int(&mut *builder, start, nbc)
                                }
                            });

                        // Declare Cranelift Variables for the loop-carried state.
                        let idx_loop_var = builder.declare_var(types::I64);
                        let acc_loop_var = builder.declare_var(types::I64);
                        builder.def_var(idx_loop_var, raw_start_idx);
                        builder.def_var(acc_loop_var, init_acc);

                        // ── Unrolled main loop (4 elements per iteration) ──
                        let unroll_header = builder.create_block();
                        let unroll_body = builder.create_block();
                        let epilogue_header = builder.create_block();
                        let epilogue_body = builder.create_block();
                        let after_all = after_block; // reuse the after_block created above

                        ensure_block_in_layout(&mut *builder, unroll_header);
                        reachable_blocks.insert(unroll_header);
                        jump_block(&mut *builder, unroll_header, &[]);

                        // Unroll header: check idx < unrolled_len
                        switch_to_block_materialized(&mut *builder, unroll_header);
                        let idx_u = builder.use_var(idx_loop_var);
                        let cmp_u = builder
                            .ins()
                            .icmp(IntCC::SignedLessThan, idx_u, unrolled_len);
                        builder
                            .ins()
                            .brif(cmp_u, unroll_body, &[], epilogue_header, &[]);

                        // Unroll body: load 4 elements, add them to accumulator
                        ensure_block_in_layout(&mut *builder, unroll_body);
                        switch_to_block_materialized(&mut *builder, unroll_body);
                        seal_block_once(&mut *builder, sealed_blocks, unroll_body);
                        let cur_idx = builder.use_var(idx_loop_var);
                        let cur_acc = builder.use_var(acc_loop_var);

                        // Element 0: data_ptr[idx * 8]
                        let off0 = builder.ins().ishl_imm(cur_idx, 3);
                        let addr0 = builder.ins().iadd(data_ptr, off0);
                        let e0 = builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), addr0, 0);
                        let acc1 = builder.ins().iadd(cur_acc, e0);

                        // Element 1: data_ptr[(idx+1) * 8]
                        let e1 = builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), addr0, 8);
                        let acc2 = builder.ins().iadd(acc1, e1);

                        // Element 2: data_ptr[(idx+2) * 8]
                        let e2 = builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), addr0, 16);
                        let acc3 = builder.ins().iadd(acc2, e2);

                        // Element 3: data_ptr[(idx+3) * 8]
                        let e3 = builder
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), addr0, 24);
                        let acc4 = builder.ins().iadd(acc3, e3);

                        // Advance index by 4
                        let next_idx_u = builder.ins().iadd_imm(cur_idx, 4);
                        builder.def_var(idx_loop_var, next_idx_u);
                        builder.def_var(acc_loop_var, acc4);
                        jump_block(&mut *builder, unroll_header, &[]);

                        // ── Scalar epilogue (0-3 remaining elements) ──
                        // epilogue_header is a loop header (back-edge from
                        // epilogue_body) — do NOT seal it here; seal_all_blocks
                        // handles it after all predecessors are wired.
                        ensure_block_in_layout(&mut *builder, epilogue_header);
                        switch_to_block_materialized(&mut *builder, epilogue_header);
                        let idx_e = builder.use_var(idx_loop_var);
                        let cmp_e = builder.ins().icmp(IntCC::SignedLessThan, idx_e, len_val);
                        builder
                            .ins()
                            .brif(cmp_e, epilogue_body, &[], after_all, &[]);

                        // Epilogue body: load 1 element, add, advance by 1
                        ensure_block_in_layout(&mut *builder, epilogue_body);
                        switch_to_block_materialized(&mut *builder, epilogue_body);
                        seal_block_once(&mut *builder, sealed_blocks, epilogue_body);
                        let idx_eb = builder.use_var(idx_loop_var);
                        let acc_eb = builder.use_var(acc_loop_var);
                        let off_e = builder.ins().ishl_imm(idx_eb, 3);
                        let addr_e = builder.ins().iadd(data_ptr, off_e);
                        let elem_e =
                            builder
                                .ins()
                                .load(types::I64, MemFlagsData::trusted(), addr_e, 0);
                        let acc_e_next = builder.ins().iadd(acc_eb, elem_e);
                        let next_idx_e = builder.ins().iadd_imm(idx_eb, 1);
                        builder.def_var(idx_loop_var, next_idx_e);
                        builder.def_var(acc_loop_var, acc_e_next);
                        jump_block(&mut *builder, epilogue_header, &[]);

                        // ── After block: write final accumulator to variables ──
                        ensure_block_in_layout(&mut *builder, after_all);
                        switch_to_block_materialized(&mut *builder, after_all);
                        seal_block_once(&mut *builder, sealed_blocks, after_all);
                        let final_acc = builder.use_var(acc_loop_var);

                        // Update the accumulator variables as raw i64.
                        // The reduction scanner only accepts proven-int
                        // loop shapes; if the static fixpoint misses one
                        // of these names, the typed-IR invariant is too
                        // narrow and should fail during verification.
                        debug_assert!(
                            representation_plan
                                .is_raw_int_carrier_name(reduction.add_out_name.as_str())
                        );
                        debug_assert!(
                            representation_plan
                                .is_raw_int_carrier_name(reduction.acc_store_slot.as_str())
                        );
                        debug_assert!(
                            representation_plan
                                .is_raw_int_carrier_name(reduction.acc_operand_name.as_str())
                        );
                        def_var_named(&mut *builder, vars, &reduction.add_out_name, final_acc);
                        def_var_named(&mut *builder, vars, &reduction.acc_store_slot, final_acc);
                        def_var_named(&mut *builder, vars, &reduction.acc_operand_name, final_acc);
                        // Skip all ops from (op_idx+1) through loop_end_idx.
                        for skip_i in (op_idx + 1)..=reduction.loop_end_idx {
                            skip_ops.insert(skip_i);
                        }

                        *is_block_filled = false;

                        if debug_loop_cfg.is_some() {
                            eprintln!(
                                "LOOP_CFG {} op{} UNROLLED_4X list={} acc={}",
                                func_ir.name, op_idx, reduction.list_name, reduction.acc_store_slot,
                            );
                        }
                        return OpFlow::Continue;
                    }
                    // Fall through to standard structured loop if data_ptr/len
                    // are not in the cache (should not happen since we hoisted).
                }

                ensure_block_in_layout(&mut *builder, loop_block);
                reachable_blocks.insert(loop_block);
                jump_block(&mut *builder, loop_block, &[]);
                switch_to_block_with_rebind(&mut *builder, loop_block, is_block_filled, false);
                loop_stack.push(LoopFrame {
                    loop_block,
                    body_block,
                    after_block,
                    index_name: Some(out_name.clone()),
                    next_index: None,
                    linearized: false,
                });
                if debug_loop_cfg.is_some() {
                    eprintln!(
                        "LOOP_CFG {} op{} structured_loop loop={:?} body={:?} after={:?}",
                        func_ir.name, op_idx, loop_block, body_block, after_block
                    );
                }
                *loop_depth += 1;
            } else {
                let loop_block = builder.create_block();
                let body_block = builder.create_block();
                let after_block = builder.create_block();
                builder.append_block_param(loop_block, types::I64);
                *is_block_filled = true;
                loop_stack.push(LoopFrame {
                    loop_block,
                    body_block,
                    after_block,
                    index_name: Some(out_name.clone()),
                    next_index: None,
                    linearized: false,
                });
                if debug_loop_cfg.is_some() {
                    eprintln!(
                        "LOOP_CFG {} op{} fallback_loop filled={} loop={:?} body={:?} after={:?}",
                        func_ir.name, op_idx, *is_block_filled, loop_block, body_block, after_block
                    );
                }
                *loop_depth += 1;
            }
        }
        "loop_break_if_exception" => {
            // Control op (no value arg): break the loop when a runtime
            // exception is pending.  Emitted by the frontend after
            // ITER_NEXT in iterator-consumer loops compiled in a
            // function WITHOUT the exception stack (function_exception
            // _label == None), where the auto-CHECK_EXCEPTION machinery
            // is absent.  The consumption loop is driven off the done
            // flag alone; on a mid-iteration raise `molt_iter_next`
            // returns the None sentinel, `done` never becomes truthy,
            // and the loop would spin forever appending garbage (OOM).
            //
            // Gating the break on the sacrosanct
            // `molt_exception_pending_fast` flag (the same predicate
            // CHECK_EXCEPTION uses) makes the break un-foldable: no
            // SCCP/copy-prop can ever prove a constant for the runtime
            // exception flag, so the loop-exit edge always survives.
            // The still-pending exception then rides up the proven
            // lazy-return path to the caller's handler.
            //
            // Block bookkeeping mirrors `loop_break_if_true`: drain the
            // dead tracked temporaries on the current block, branch to a
            // cleanup block (which dec-refs them and jumps to the loop's
            // after_block) on a pending exception, else fall through to
            // the loop body.
            if loop_stack.is_empty() {
                *is_block_filled = true;
            } else {
                let frame = loop_stack.last().unwrap();
                let current_block = builder
                    .current_block()
                    .expect("loop_break_if_exception requires an active block");
                let mut carry_obj_lb = block_tracked_obj.remove(&current_block).unwrap_or_default();
                let tracked_obj_snapshot = drain_cleanup_tracked_dedup_with_authority(
                    native_rc_tracking_enabled,
                    &mut carry_obj_lb,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                let mut carry_ptr_lb = block_tracked_ptr.remove(&current_block).unwrap_or_default();
                let tracked_ptr_snapshot = drain_cleanup_tracked_dedup_with_authority(
                    native_rc_tracking_enabled,
                    &mut carry_ptr_lb,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                // Read the authoritative runtime exception-pending flag.
                // In needs-stack=False functions `exc_flag_ptr_slot` is
                // None, so this lowers to a `call molt_exception_pending
                // _fast` + `icmp != 0` — never a foldable value.
                let cond_bool = emit_exception_pending_condition(
                    &mut *builder,
                    local_exc_pending_fast,
                    exc_flag_ptr_slot,
                );
                let cleanup_block = builder.create_block();
                if debug_block_origins.is_some() {
                    eprintln!(
                        "BLOCK_ORIGIN {} op{} loop_break_if_exception cleanup={:?} body={:?} after={:?}",
                        func_ir.name, op_idx, cleanup_block, frame.body_block, frame.after_block
                    );
                }
                if let Some(current_block) = builder.current_block() {
                    builder.insert_block_after(cleanup_block, current_block);
                }
                reachable_blocks.insert(cleanup_block);
                reachable_blocks.insert(frame.body_block);
                builder
                    .ins()
                    .brif(cond_bool, cleanup_block, &[], frame.body_block, &[]);
                switch_to_block_with_rebind(&mut *builder, cleanup_block, is_block_filled, false);
                if exception_label_ids.is_empty() && sealed_blocks.insert(cleanup_block) {
                    maybe_debug_seal("loop_break_exception_cleanup", op_idx, cleanup_block);
                    seal_block_once(&mut *builder, sealed_blocks, cleanup_block);
                }
                for name in tracked_obj_snapshot {
                    let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                        .unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                for name in tracked_ptr_snapshot {
                    let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                        .unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                reachable_blocks.insert(frame.after_block);
                ensure_block_in_layout(&mut *builder, frame.after_block);
                jump_block(&mut *builder, frame.after_block, &[]);
                switch_to_block_with_rebind(
                    &mut *builder,
                    frame.body_block,
                    is_block_filled,
                    false,
                );
                // Seal body_block now — its only predecessor is the brif above.
                if exception_label_ids.is_empty() && sealed_blocks.insert(frame.body_block) {
                    maybe_debug_seal("loop_break_exception_body", op_idx, frame.body_block);
                    seal_block_once(&mut *builder, sealed_blocks, frame.body_block);
                }
                propagate_tracked_to_branches(
                    block_tracked_obj,
                    &[frame.body_block, frame.after_block],
                    carry_obj_lb,
                );
                propagate_tracked_to_branches(
                    block_tracked_ptr,
                    &[frame.body_block, frame.after_block],
                    carry_ptr_lb,
                );
            }
        }
        "loop_break_if_true" => {
            if loop_stack.is_empty() {
                *is_block_filled = true;
            } else {
                let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                let frame = loop_stack.last().unwrap();
                let current_block = builder
                    .current_block()
                    .expect("loop_break_if_true requires an active block");
                let mut carry_obj_lb = block_tracked_obj.remove(&current_block).unwrap_or_default();
                let tracked_obj_snapshot = drain_cleanup_tracked_dedup_with_authority(
                    native_rc_tracking_enabled,
                    &mut carry_obj_lb,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                let mut carry_ptr_lb = block_tracked_ptr.remove(&current_block).unwrap_or_default();
                let tracked_ptr_snapshot = drain_cleanup_tracked_dedup_with_authority(
                    native_rc_tracking_enabled,
                    &mut carry_ptr_lb,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                // Fast path: extract bool payload directly for NaN-boxed
                // booleans from fast_int comparisons or type hints.
                let cond_name = &args[0];
                let cond_is_bool_typed = var_is_bool(cond_name);
                let cond_is_int_typed = !cond_is_bool_typed && var_is_int(cond_name);
                let cond_bool = if let Some(raw_val) =
                    bool_raw_value(&mut *builder, vars, representation_plan, cond_name)
                {
                    // Raw bool from proven list_bool getitem or const_bool.
                    builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0)
                } else if cond_is_bool_typed {
                    // NaN-boxed bool: bit 0 is the boolean value.
                    let cond = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("Loop break cond not found");
                    let one = builder.ins().iconst(types::I64, 1);
                    let payload = builder.ins().band(*cond, one);
                    builder.ins().icmp_imm(IntCC::NotEqual, payload, 0)
                } else if let Some(raw_shadow) =
                    int_raw_value(&mut *builder, vars, representation_plan, &args[0])
                {
                    // Proven raw i64 carrier: truthiness is `value != 0`.
                    builder.ins().icmp_imm(IntCC::NotEqual, raw_shadow, 0)
                } else if cond_is_int_typed {
                    // `cond_is_int_typed` only proves Python-`int` type,
                    // which includes heap BigInts (TAG_PTR). The trusted
                    // unbox would truncate a BigInt pointer (e.g. `1 << 47`
                    // has low 47 bits zero and would be wrongly falsy).
                    // Guard on a runtime inline-int tag check: inline
                    // TAG_INT/TAG_BOOL use `unbox != 0`; any heap int
                    // (BigInt) is non-zero by construction, hence truthy.
                    let cond = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("Loop break cond not found");
                    let cond_val = unbox_int_or_bool(&mut *builder, *cond, nbc);
                    let is_inline_int = fused_is_int_or_bool(&mut *builder, *cond, nbc);
                    let inline_truthy = builder.ins().icmp_imm(IntCC::NotEqual, cond_val, 0);
                    let true_val = builder.ins().iconst(types::I8, 1);
                    builder.ins().select(is_inline_int, inline_truthy, true_val)
                } else {
                    let cond = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("Loop break cond not found");
                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_is_truthy",
                        &[types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0)
                };
                let cleanup_block = builder.create_block();
                if debug_block_origins.is_some() {
                    eprintln!(
                        "BLOCK_ORIGIN {} op{} loop_break_if_true cleanup={:?} body={:?} after={:?}",
                        func_ir.name, op_idx, cleanup_block, frame.body_block, frame.after_block
                    );
                }
                if let Some(current_block) = builder.current_block() {
                    builder.insert_block_after(cleanup_block, current_block);
                }
                reachable_blocks.insert(cleanup_block);
                reachable_blocks.insert(frame.body_block);
                builder
                    .ins()
                    .brif(cond_bool, cleanup_block, &[], frame.body_block, &[]);
                switch_to_block_with_rebind(&mut *builder, cleanup_block, is_block_filled, false);
                if exception_label_ids.is_empty() && sealed_blocks.insert(cleanup_block) {
                    maybe_debug_seal("loop_break_true_cleanup", op_idx, cleanup_block);
                    seal_block_once(&mut *builder, sealed_blocks, cleanup_block);
                }
                for name in tracked_obj_snapshot {
                    let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                        .unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                for name in tracked_ptr_snapshot {
                    let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                        .unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                reachable_blocks.insert(frame.after_block);
                ensure_block_in_layout(&mut *builder, frame.after_block);
                jump_block(&mut *builder, frame.after_block, &[]);
                switch_to_block_with_rebind(
                    &mut *builder,
                    frame.body_block,
                    is_block_filled,
                    false,
                );
                // Seal body_block now — its only predecessor is the brif above.
                if exception_label_ids.is_empty() && sealed_blocks.insert(frame.body_block) {
                    maybe_debug_seal("loop_break_true_body", op_idx, frame.body_block);
                    seal_block_once(&mut *builder, sealed_blocks, frame.body_block);
                }
                propagate_tracked_to_branches(
                    block_tracked_obj,
                    &[frame.body_block, frame.after_block],
                    carry_obj_lb,
                );
                propagate_tracked_to_branches(
                    block_tracked_ptr,
                    &[frame.body_block, frame.after_block],
                    carry_ptr_lb,
                );
            }
        }
        "loop_break_if_false" => {
            if loop_stack.is_empty() {
                *is_block_filled = true;
            } else {
                let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
                let frame = loop_stack.last().unwrap();
                if debug_loop_cfg.is_some() {
                    eprintln!(
                        "LOOP_CFG {} op{} loop_break_if_false loop={:?} body={:?} after={:?}",
                        func_ir.name, op_idx, frame.loop_block, frame.body_block, frame.after_block
                    );
                }
                let current_block = builder
                    .current_block()
                    .expect("loop_break_if_false requires an active block");
                let mut carry_obj_lb = block_tracked_obj.remove(&current_block).unwrap_or_default();
                let tracked_obj_snapshot = drain_cleanup_tracked_dedup_with_authority(
                    native_rc_tracking_enabled,
                    &mut carry_obj_lb,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                let mut carry_ptr_lb = block_tracked_ptr.remove(&current_block).unwrap_or_default();
                let tracked_ptr_snapshot = drain_cleanup_tracked_dedup_with_authority(
                    native_rc_tracking_enabled,
                    &mut carry_ptr_lb,
                    last_use,
                    alias_roots,
                    op_idx,
                    None,
                    Some(&mut *already_decrefed),
                );
                // Fast path: when the condition is a NaN-boxed bool from a
                // fast_int comparison (lt/le/gt/ge/eq/ne), extract the bool
                // payload directly instead of calling molt_is_truthy.  This
                // eliminates a runtime call per loop iteration AND avoids
                // inserting extra Cranelift blocks between the loop header
                // and body — keeping SSA variable propagation clean so the
                // loop induction variable is correctly threaded through the
                // back-edge.
                let cond_name = &args[0];
                let cond_is_bool_typed = var_is_bool(cond_name);
                let cond_is_int_typed = !cond_is_bool_typed && var_is_int(cond_name);
                let cond_bool = if let Some(raw_val) =
                    bool_raw_value(&mut *builder, vars, representation_plan, cond_name)
                {
                    // Raw bool from proven list_bool getitem or const_bool.
                    builder.ins().icmp_imm(IntCC::NotEqual, raw_val, 0)
                } else if cond_is_bool_typed {
                    // Condition is QNAN|TAG_BOOL|{0,1}: low bit is the bool.
                    let cond = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("Loop break cond not found");
                    let one = builder.ins().iconst(types::I64, 1);
                    let payload = builder.ins().band(*cond, one);
                    builder.ins().icmp_imm(IntCC::NotEqual, payload, 0)
                } else if let Some(raw_shadow) =
                    int_raw_value(&mut *builder, vars, representation_plan, &args[0])
                {
                    // Proven raw i64 carrier: truthiness is `value != 0`.
                    builder.ins().icmp_imm(IntCC::NotEqual, raw_shadow, 0)
                } else if cond_is_int_typed {
                    // `cond_is_int_typed` only proves Python-`int` type,
                    // which includes heap BigInts (TAG_PTR). The trusted
                    // unbox would truncate a BigInt pointer (e.g. `1 << 47`
                    // has low 47 bits zero and would be wrongly falsy).
                    // Guard on a runtime inline-int tag check: inline
                    // TAG_INT/TAG_BOOL use `unbox != 0`; any heap int
                    // (BigInt) is non-zero by construction, hence truthy.
                    let cond = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("Loop break cond not found");
                    let cond_val = unbox_int_or_bool(&mut *builder, *cond, nbc);
                    let is_inline_int = fused_is_int_or_bool(&mut *builder, *cond, nbc);
                    let inline_truthy = builder.ins().icmp_imm(IntCC::NotEqual, cond_val, 0);
                    let true_val = builder.ins().iconst(types::I8, 1);
                    builder.ins().select(is_inline_int, inline_truthy, true_val)
                } else {
                    let cond = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        &args[0],
                        representation_plan,
                    )
                    .expect("Loop break cond not found");
                    let callee = SimpleBackend::import_func_id_split(
                        &mut *module,
                        &mut *import_ids,
                        "molt_is_truthy",
                        &[types::I64],
                        &[types::I64],
                    );
                    let local_callee = module.declare_func_in_func(callee, builder.func);
                    let call = builder.ins().call(local_callee, &[*cond]);
                    let truthy = builder.inst_results(call)[0];
                    builder.ins().icmp_imm(IntCC::NotEqual, truthy, 0)
                };
                let cleanup_block = builder.create_block();
                if debug_block_origins.is_some() {
                    eprintln!(
                        "BLOCK_ORIGIN {} op{} loop_break_if_false cleanup={:?} body={:?} after={:?}",
                        func_ir.name, op_idx, cleanup_block, frame.body_block, frame.after_block
                    );
                }
                if let Some(current_block) = builder.current_block() {
                    builder.insert_block_after(cleanup_block, current_block);
                }
                reachable_blocks.insert(frame.body_block);
                reachable_blocks.insert(cleanup_block);
                builder
                    .ins()
                    .brif(cond_bool, frame.body_block, &[], cleanup_block, &[]);
                switch_to_block_with_rebind(&mut *builder, cleanup_block, is_block_filled, false);
                if exception_label_ids.is_empty() && sealed_blocks.insert(cleanup_block) {
                    maybe_debug_seal("loop_break_false_cleanup", op_idx, cleanup_block);
                    seal_block_once(&mut *builder, sealed_blocks, cleanup_block);
                }
                for name in tracked_obj_snapshot {
                    let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                        .unwrap_or_else(|| {
                            panic!(
                                "Tracked obj var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                for name in tracked_ptr_snapshot {
                    let val = resolve_cleanup_value(&mut *builder, vars, entry_vars, &name)
                        .unwrap_or_else(|| {
                            panic!(
                                "Tracked ptr var not found in {} op {}: {}",
                                func_ir.name, op_idx, name
                            )
                        });
                    builder.ins().call(local_dec_ref_obj, &[val]);
                }
                reachable_blocks.insert(frame.after_block);
                ensure_block_in_layout(&mut *builder, frame.after_block);
                jump_block(&mut *builder, frame.after_block, &[]);
                switch_to_block_with_rebind(
                    &mut *builder,
                    frame.body_block,
                    is_block_filled,
                    false,
                );
                // Seal body_block now — its only predecessor is the brif
                // above.  Early sealing helps Cranelift resolve SSA variables
                // (especially the loop induction variable) immediately.
                if exception_label_ids.is_empty() && sealed_blocks.insert(frame.body_block) {
                    maybe_debug_seal("loop_break_false_body", op_idx, frame.body_block);
                    seal_block_once(&mut *builder, sealed_blocks, frame.body_block);
                }
                propagate_tracked_to_branches(
                    block_tracked_obj,
                    &[frame.body_block, frame.after_block],
                    carry_obj_lb,
                );
                propagate_tracked_to_branches(
                    block_tracked_ptr,
                    &[frame.body_block, frame.after_block],
                    carry_ptr_lb,
                );
            }
        }
        "loop_break" => {
            if loop_stack.is_empty() {
                // break duplicated into an outer exception handler
                // that sits after the loop boundary — treat as dead.
                *is_block_filled = true;
            } else {
                let frame = loop_stack.last().unwrap();
                let current_block = builder
                    .current_block()
                    .expect("loop_break requires an active block");
                if let Some(names) = block_tracked_obj.get_mut(&current_block) {
                    let cleanup = drain_cleanup_tracked_dedup_with_authority(
                        native_rc_tracking_enabled,
                        names,
                        last_use,
                        alias_roots,
                        op_idx,
                        None,
                        Some(&mut *already_decrefed),
                    );
                    for name in cleanup {
                        // Use entry_vars (definition-time Value) for dec_ref,
                        // not var_get (current SSA Value). If the variable was
                        // redefined, var_get returns the WRONG object.
                        let val = entry_vars.get(&name).copied().or_else(|| {
                            var_get_boxed_overflow_safe(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                &name,
                                representation_plan,
                            )
                            .map(|v| *v)
                        });
                        let Some(val) = val else {
                            continue;
                        };
                        builder.ins().call(local_dec_ref_obj, &[val]);
                    }
                }
                if let Some(names) = block_tracked_ptr.get_mut(&current_block) {
                    let cleanup = drain_cleanup_tracked_dedup_with_authority(
                        native_rc_tracking_enabled,
                        names,
                        last_use,
                        alias_roots,
                        op_idx,
                        None,
                        Some(&mut *already_decrefed),
                    );
                    for name in cleanup {
                        let val = entry_vars.get(&name).copied().or_else(|| {
                            var_get_boxed_overflow_safe(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                &name,
                                representation_plan,
                            )
                            .map(|v| *v)
                        });
                        let Some(val) = val else {
                            continue;
                        };
                        builder.ins().call(local_dec_ref_obj, &[val]);
                    }
                }
                reachable_blocks.insert(frame.after_block);
                ensure_block_in_layout(&mut *builder, frame.after_block);
                jump_block(&mut *builder, frame.after_block, &[]);
                *is_block_filled = true;
            }
        }
        "loop_index_next" => {
            let args = op.args.as_ref().unwrap_or(&EMPTY_VEC_STRING);
            let next_idx = var_get_boxed_overflow_safe(
                &mut *module,
                &mut *import_ids,
                &mut *builder,
                import_refs,
                sealed_blocks,
                vars,
                &args[0],
                representation_plan,
            )
            .expect("Loop index next not found");
            if loop_stack.is_empty() {
                let Some(out_name) = op.out.as_ref() else {
                    return OpFlow::Continue;
                };
                def_var_named(&mut *builder, vars, out_name, *next_idx);
            } else {
                let frame = loop_stack.last_mut().unwrap();
                let next_raw = args
                    .first()
                    .and_then(|name| int_raw_value(&mut *builder, vars, representation_plan, name));
                let next_value = if frame
                    .index_name
                    .as_deref()
                    .is_some_and(|name| representation_plan.is_raw_int_carrier_name(name))
                {
                    next_raw.unwrap_or_else(|| unbox_int_or_bool(&mut *builder, *next_idx, nbc))
                } else {
                    *next_idx
                };
                frame.next_index = Some(next_value);
                if let Some(out_name) = op.out.as_ref()
                    && frame.index_name.as_ref() != Some(out_name)
                {
                    let out_value =
                        if representation_plan.is_raw_int_carrier_name(out_name.as_str()) {
                            next_raw.unwrap_or(next_value)
                        } else {
                            *next_idx
                        };
                    def_var_named(&mut *builder, vars, out_name.clone(), out_value);
                }
            }
        }
        "loop_continue" => {
            if loop_stack.is_empty() {
                // Same as loop_index_next: the continue was
                // duplicated into an outer exception handler that
                // sits after the loop's END_LOOP.  Mark the block
                // as filled so subsequent ops are dead code.
                *is_block_filled = true;
            } else {
                let frame = loop_stack.last_mut().unwrap();
                if frame.next_index.is_none()
                    && let Some(index_name) = frame.index_name.as_ref()
                {
                    let mut depth = 0i32;
                    for scan in (0..op_idx).rev() {
                        let scan_op = &func_ir.ops[scan];
                        match scan_op.kind.as_str() {
                            "loop_end" => {
                                depth += 1;
                            }
                            "loop_start" | "loop_index_start" => {
                                if depth == 0 {
                                    break;
                                }
                                depth -= 1;
                            }
                            "store_var" if depth == 0 => {
                                if scan_op.var.as_deref() == Some(index_name.as_str())
                                    && let Some(src_name) =
                                        scan_op.args.as_ref().and_then(|args| args.first())
                                    && let Some(next_idx) = var_get_boxed_overflow_safe(
                                        &mut *module,
                                        &mut *import_ids,
                                        &mut *builder,
                                        import_refs,
                                        sealed_blocks,
                                        vars,
                                        src_name,
                                        representation_plan,
                                    )
                                {
                                    frame.next_index = Some(*next_idx);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                let current_block = builder
                    .current_block()
                    .expect("loop_continue requires an active block");
                if let Some(names) = block_tracked_obj.get_mut(&current_block) {
                    let cleanup = drain_cleanup_tracked_dedup_with_authority(
                        native_rc_tracking_enabled,
                        names,
                        last_use,
                        alias_roots,
                        op_idx,
                        None,
                        Some(&mut *already_decrefed),
                    );
                    for name in cleanup {
                        // Use entry_vars (definition-time Value) for dec_ref,
                        // not var_get (current SSA Value). If the variable was
                        // redefined, var_get returns the WRONG object.
                        let val = entry_vars.get(&name).copied().or_else(|| {
                            var_get_boxed_overflow_safe(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                &name,
                                representation_plan,
                            )
                            .map(|v| *v)
                        });
                        let Some(val) = val else {
                            continue;
                        };
                        builder.ins().call(local_dec_ref_obj, &[val]);
                    }
                }
                if let Some(names) = block_tracked_ptr.get_mut(&current_block) {
                    let cleanup = drain_cleanup_tracked_dedup_with_authority(
                        native_rc_tracking_enabled,
                        names,
                        last_use,
                        alias_roots,
                        op_idx,
                        None,
                        Some(&mut *already_decrefed),
                    );
                    for name in cleanup {
                        let val = entry_vars.get(&name).copied().or_else(|| {
                            var_get_boxed_overflow_safe(
                                &mut *module,
                                &mut *import_ids,
                                &mut *builder,
                                import_refs,
                                sealed_blocks,
                                vars,
                                &name,
                                representation_plan,
                            )
                            .map(|v| *v)
                        });
                        let Some(val) = val else {
                            continue;
                        };
                        builder.ins().call(local_dec_ref_obj, &[val]);
                    }
                }
                reachable_blocks.insert(frame.loop_block);
                // Step 3: def_var the counter with incremented value,
                // then jump with no explicit args.  SSA carries the phi.
                if let Some(next_idx) = frame.next_index.take()
                    && let Some(name) = frame.index_name.as_ref()
                {
                    def_var_named(&mut *builder, vars, name, next_idx);
                } else if let Some(name) = frame.index_name.as_ref()
                    && let Some(current) = var_get_boxed_overflow_safe(
                        &mut *module,
                        &mut *import_ids,
                        &mut *builder,
                        import_refs,
                        sealed_blocks,
                        vars,
                        name,
                        representation_plan,
                    )
                {
                    let current = if representation_plan.is_raw_int_carrier_name(name.as_str()) {
                        int_raw_value(&mut *builder, vars, representation_plan, name)
                            .unwrap_or_else(|| unbox_int_or_bool(&mut *builder, *current, nbc))
                    } else {
                        *current
                    };
                    def_var_named(&mut *builder, vars, name, current);
                }
                jump_block(&mut *builder, frame.loop_block, &[]);
                *is_block_filled = true;
            }
        }
        "loop_end" => {
            if loop_stack.is_empty() {
                // Orphan loop_end from a duplicated exception
                // handler path — skip silently.
            } else {
                let mut frame = loop_stack.pop().unwrap();
                if debug_loop_cfg.is_some() {
                    eprintln!(
                        "LOOP_CFG {} op{} loop_end loop={:?} body={:?} after={:?} reachable_after={} filled={}",
                        func_ir.name,
                        op_idx,
                        frame.loop_block,
                        frame.body_block,
                        frame.after_block,
                        reachable_blocks.contains(&frame.after_block),
                        *is_block_filled,
                    );
                }
                if !frame.linearized {
                    *loop_depth -= 1;
                }
                if !*is_block_filled {
                    ensure_block_in_layout(&mut *builder, frame.loop_block);
                    reachable_blocks.insert(frame.loop_block);
                    if let Some(next_idx) = frame.next_index.take()
                        && let Some(name) = frame.index_name.as_ref()
                    {
                        def_var_named(&mut *builder, vars, name, next_idx);
                    }
                    jump_block(&mut *builder, frame.loop_block, &[]);
                }
                if builder.func.layout.is_block_inserted(frame.loop_block) {
                    seal_block_once(&mut *builder, sealed_blocks, frame.loop_block);
                }
                if reachable_blocks.contains(&frame.after_block) {
                    ensure_block_in_layout(&mut *builder, frame.after_block);
                    if debug_loop_cfg.is_some() {
                        eprintln!(
                            "LOOP_CFG {} op{} switch_after {:?}",
                            func_ir.name, op_idx, frame.after_block
                        );
                    }
                    switch_to_block_with_rebind(
                        &mut *builder,
                        frame.after_block,
                        is_block_filled,
                        false,
                    );
                    if exception_label_ids.is_empty()
                        && builder.func.layout.is_block_inserted(frame.after_block)
                        && sealed_blocks.insert(frame.after_block)
                    {
                        maybe_debug_seal("loop_end_after", op_idx, frame.after_block);
                        seal_block_once(&mut *builder, sealed_blocks, frame.after_block);
                    }
                } else {
                    *is_block_filled = true;
                }
            }
        }
        _ => unreachable!("non-loop op dispatched to handle_loop_op: {}", op.kind),
    }
    OpFlow::Proceed
}
